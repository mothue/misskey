/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { mkdtemp, chmod, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { Readable } from 'node:stream';
import { Inject, Injectable, type OnApplicationShutdown } from '@nestjs/common';
import { ModuleRef } from '@nestjs/core';
import type { DataSource } from 'typeorm';
import type { Config } from '@/config.js';
import type { InstancesRepository, AccessTokensRepository, MiMeta } from '@/models/_.js';
import type { MiLocalUser } from '@/models/User.js';
import type { MiAccessToken } from '@/models/AccessToken.js';
import { DI } from '@/di-symbols.js';
import { UserEntityService } from '@/core/entities/UserEntityService.js';
import { DEFAULT_POLICIES } from '@/core/RoleService.js';
import { bindThis } from '@/decorators.js';
import { RustBackendService } from '@/server/rust/RustBackendService.js';
import { ApiCallbackServer, encodeApiResult, type ApiCallbackInvocation, type ApiCallbackResult } from '@/server/rust/api-callback.js';
import { apiEntityLayout, hydrateApiEntity } from '@/server/rust/api-entity.js';
import { apiRequestBody } from '@/server/rust/api-body.js';
import { resolveRustPrimaryTarget } from '@/server/rust/postgres-config.js';
import { resolveRustRedisConfig } from '@/server/rust/redis-config.js';
import endpoints from './endpoints.js';
import { ApiCallService, type ValidatedLegacyEndpoint } from './ApiCallService.js';
import { SignupApiService } from './SignupApiService.js';
import { SigninApiService } from './SigninApiService.js';
import { SigninWithPasskeyApiService } from './SigninWithPasskeyApiService.js';
import type { FastifyInstance, FastifyPluginOptions, FastifyRequest, FastifyReply } from 'fastify';

class LegacyReply {
	statusCode = 200;
	readonly headers: Record<string, string> = {};
	payload: unknown;
	sent = false;
	code(status: number): this { this.statusCode = status; return this; }
	status(status: number): this { return this.code(status); }
	header(name: string, value: unknown): this { this.headers[name.toLowerCase()] = String(value); return this; }
	send(payload?: unknown): this { this.payload = payload; this.sent = true; return this; }
}

@Injectable()
export class ApiServerService implements OnApplicationShutdown {
	private readonly callbacks = new ApiCallbackServer((invocation, request) => this.executeLegacy(invocation, request));
	private uploadDirectory?: string;
	private preparePromise?: Promise<void>;
	private stopPromise?: Promise<void>;

	constructor(
		private moduleRef: ModuleRef,
		@Inject(DI.config) private config: Config,
		@Inject(DI.meta) private meta: MiMeta,
		@Inject(DI.db) private db: DataSource,
		@Inject(DI.instancesRepository) private instancesRepository: InstancesRepository,
		@Inject(DI.accessTokensRepository) private accessTokensRepository: AccessTokensRepository,
		private userEntityService: UserEntityService,
		private apiCallService: ApiCallService,
		private signupApiService: SignupApiService,
		private signinApiService: SigninApiService,
		private signinWithPasskeyApiService: SigninWithPasskeyApiService,
		private rustBackendService: RustBackendService,
	) {}

	public prepareRustApi(): Promise<void> {
		return this.preparePromise ??= (async () => {
			try {
				this.uploadDirectory = await mkdtemp(join(tmpdir(), 'misskey-rust-api-'));
				await chmod(this.uploadDirectory, 0o700);
				const callback = await this.callbacks.start();
				const tables = ['user', 'access_token', 'app', 'role', 'role_assignment', 'antenna', 'clip', 'gallery_post', 'gallery_like'];
				const entityLayouts = Object.fromEntries(tables.map(table => [table, apiEntityLayout(this.db.getMetadata(table))]));
				this.rustBackendService.configureApi({
					callback,
					endpoints: endpoints.map(endpoint => ({ name: endpoint.name, meta: endpoint.meta, params: endpoint.params })),
					maxFileSize: this.config.maxFileSize,
					enableIpRateLimit: this.config.enableIpRateLimit ?? false,
					environment: process.env.NODE_ENV ?? '',
					effectiveTimeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					uploadDirectory: this.uploadDirectory,
					host: this.config.host,
					postgresPrimary: await resolveRustPrimaryTarget(this.config),
					redis: resolveRustRedisConfig(this.config.redis),
					redisPubsub: resolveRustRedisConfig(this.config.redisForPubsub),
					redisReactions: resolveRustRedisConfig(this.config.redisForReactions),
					notes: {
						url: this.config.url,
						host: this.config.host,
						mediaProxy: this.config.mediaProxy,
						externalMediaProxyEnabled: this.config.externalMediaProxyEnabled,
						videoThumbnailGenerator: this.config.videoThumbnailGenerator,
					},
					auth: {
						idMethod: this.config.id,
						maxFileSize: this.config.maxFileSize,
						defaultPolicies: DEFAULT_POLICIES,
						meta: {
							rootUserId: this.meta.rootUserId,
							policies: this.meta.policies,
							enableIpLogging: this.meta.enableIpLogging,
							preservedUsernames: this.meta.preservedUsernames,
							ugcVisibilityForVisitor: this.meta.ugcVisibilityForVisitor,
							enableReactionsBuffering: this.meta.enableReactionsBuffering,
							showRoleBadgesOfRemoteUsers: this.meta.showRoleBadgesOfRemoteUsers,
							proxyRemoteFiles: this.meta.proxyRemoteFiles,
							iconUrl: this.meta.iconUrl,
						},
						entityLayouts,
					},
				});
			} catch (error) {
				await this.stop();
				throw error;
			}
		})();
	}

	@bindThis
	public createServer(fastify: FastifyInstance, options: FastifyPluginOptions, done: (err?: Error) => void): void {
		// Proxy during onRequest, before Fastify parses a body or generates CORS responses.
		fastify.addHook('onRequest', async (request, reply) => {
			const pending = this.callbacks.register(request);
			const controller = new AbortController();
			let stopWaiting: () => void = () => {};
			const disconnected = new Promise<void>(resolve => { stopWaiting = resolve; });
			const aborted = () => { pending.release(); controller.abort(); stopWaiting(); };
			const closed = () => { if (!reply.raw.writableFinished) aborted(); };
			request.raw.once('aborted', aborted);
			reply.raw.once('close', closed);
			const body = apiRequestBody(request.raw);
			try {
				const context = { v: 1 as const, requestId: pending.requestId, path: request.raw.url ?? request.url, method: request.method, ip: request.ip, headers: request.headers };
				const response = await this.apiCallService.withApiSpan(request.url.split('?')[0].replace(/^\/api\//, ''), () => this.rustBackendService.api(context, body.stream, controller.signal));
				body.drain();
				if (controller.signal.aborted) { await response.body?.cancel(); return; }
				reply.code(response.status);
				for (const [name, value] of response.headers) {
					if (!['connection', 'keep-alive', 'transfer-encoding', 'upgrade'].includes(name)) reply.header(name, value);
				}
				if (response.body === null) reply.send();
				else reply.send(Readable.fromWeb(response.body as import('node:stream/web').ReadableStream<Uint8Array>));
				await Promise.race([reply, disconnected]);
			} finally {
				body.drain();
				pending.release();
				request.raw.off('aborted', aborted);
				reply.raw.off('close', closed);
			}
		});
		fastify.all('/', () => {});
		fastify.all('/*', () => {});
		done();
	}

	private async executeLegacy(invocation: ApiCallbackInvocation, original: FastifyRequest): Promise<ApiCallbackResult> {
		const path = new URL(original.raw.url ?? original.url, 'http://localhost').pathname;
		const endpointPath = path.startsWith('/api/') ? path.slice(5) : '';
		if (decodeURIComponent(endpointPath) !== invocation.endpoint) throw new Error('API callback endpoint mismatch');
		if (invocation.kind === 'special') return this.executeSpecial(invocation, original);
		if (!endpoints.some(endpoint => endpoint.name === invocation.endpoint)) throw new Error('Unknown legacy callback');
		if (this.rustBackendService.isNativeEndpoint(invocation.endpoint)) throw new Error('Native API cannot execute through TS');
		const userRecord = invocation.user === null ? null : hydrateApiEntity(this.db.getMetadata('user'), invocation.user);
		if (userRecord !== null && typeof userRecord.id !== 'string') throw new Error('Invalid API user');
		let tokenRecord: Record<string, unknown> | null = null;
		if (invocation.token !== null) {
			const reduced = typeof invocation.token === 'object' && Object.keys(invocation.token).length === 2;
			tokenRecord = hydrateApiEntity(this.db.getMetadata('access_token'), invocation.token, reduced);
			if (typeof tokenRecord.id !== 'string' || !Array.isArray(tokenRecord.permission) || tokenRecord.permission.some(permission => typeof permission !== 'string')) throw new Error('Invalid API token');
		}
		if (invocation.file && (!this.uploadDirectory || dirname(invocation.file.path) !== this.uploadDirectory || !basename(invocation.file.path).startsWith('upload-'))) throw new Error('Invalid API upload path');
		const endpoint: ValidatedLegacyEndpoint = this.moduleRef.get('ep:' + invocation.endpoint, { strict: false });
		if (typeof endpoint.invokeValidated !== 'function') throw new Error('Invalid legacy executor');
		return await this.apiCallService.executeLegacy(invocation.endpoint, endpoint, invocation.params, userRecord as MiLocalUser | null, tokenRecord as MiAccessToken | null, invocation.file, original.ip, original.headers as Record<string, string>, encodeApiResult) as ApiCallbackResult;
	}

	private async executeSpecial(invocation: ApiCallbackInvocation, original: FastifyRequest): Promise<ApiCallbackResult> {
		const request = Object.create(original) as FastifyRequest;
		Object.defineProperties(request, {
			body: { value: invocation.params, enumerable: true },
			params: { value: invocation.routeParams ?? {}, enumerable: true },
		});
		const recorder = new LegacyReply();
		const reply = recorder as unknown as FastifyReply;
		let result: unknown;
		try {
			switch (invocation.endpoint) {
				case 'signup': result = await this.signupApiService.signup(request as Parameters<SignupApiService['signup']>[0], reply); break;
				case 'signin-flow': result = await this.signinApiService.signin(request as Parameters<SigninApiService['signin']>[0], reply); break;
				case 'signin-with-passkey': result = await this.signinWithPasskeyApiService.signin(request as Parameters<SigninWithPasskeyApiService['signin']>[0], reply); break;
				case 'signup-pending': result = await this.signupApiService.signupPending(request as Parameters<SignupApiService['signupPending']>[0], reply); break;
				case 'v1/instance/peers': result = (await this.instancesRepository.find({ select: { host: true }, where: { suspensionState: 'none' } })).map(instance => instance.host); break;
				default: {
					if (!invocation.routeParams || invocation.endpoint !== `miauth/${invocation.routeParams.session}/check`) throw new Error('Unknown API special callback');
					const token = await this.accessTokensRepository.findOneBy({ session: invocation.routeParams.session });
					if (token && token.session != null && !token.fetched) {
						void this.accessTokensRepository.update(token.id, { fetched: true });
						result = { ok: true, token: token.token, user: await this.userEntityService.pack(token.userId, null, { schema: 'UserDetailedNotMe' }) };
					} else result = { ok: false };
				}
			}
			if (recorder.sent) result = recorder.payload;
		} catch (error) {
			const candidate = error as { statusCode?: unknown; message?: unknown; code?: unknown };
			const status = typeof candidate.statusCode === 'number' && candidate.statusCode >= 400 && candidate.statusCode < 600 ? candidate.statusCode : 500;
			return { v: 1, kind: 'framework-error', status, headers: recorder.headers, message: typeof candidate.message === 'string' ? candidate.message : 'Internal Server Error', ...(typeof candidate.code === 'string' ? { code: candidate.code } : {}) };
		}
		if (result !== undefined && !recorder.headers['content-type']) recorder.header('content-type', typeof result === 'string' ? 'text/plain; charset=utf-8' : 'application/json; charset=utf-8');
		return { v: 1, kind: 'http', status: recorder.statusCode, headers: recorder.headers, ...(result === undefined ? {} : { bodyJson: typeof result === 'string' ? result : JSON.stringify(result) }) };
	}

	public stop(): Promise<void> {
		return this.stopPromise ??= (async () => {
			await this.callbacks.stop();
			if (this.uploadDirectory) await rm(this.uploadDirectory, { recursive: true, force: true });
		})();
	}

	public onApplicationShutdown(): Promise<void> { return this.stop(); }
}
