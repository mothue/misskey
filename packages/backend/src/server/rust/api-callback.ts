/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { ApiError } from '@/server/api/error.js';
import { AuthenticationError } from '@/server/api/AuthenticateService.js';
import { apiDecodeValue, apiEncodeValue, apiStringifyFrame, type ApiWireValue } from './api-wire.js';
import type { FastifyRequest } from 'fastify';
import type { Server } from 'node:http';

const MAX_CALLBACK_BYTES = 64 * 1024 * 1024;

export type ApiCallbackInvocation = {
	v: 1;
	requestId: string;
	kind: 'endpoint' | 'special';
	endpoint: string;
	params: unknown;
	user: unknown;
	token: unknown;
	file?: { name?: string | null; path: string };
	routeParams?: { session: string };
};

export type ApiCallbackResult =
	| { v: 1; kind: 'value'; value: ApiWireValue }
	| { v: 1; kind: 'json'; resultJson?: string }
	| { v: 1; kind: 'bytes'; base64: string }
	| { v: 1; kind: 'api-error'; message: string; code: string; id: string; errorKind: string; httpStatusCode?: number; info: ApiWireValue }
	| { v: 1; kind: 'authentication-error' }
	| { v: 1; kind: 'framework-error'; status: number; headers: Record<string, string>; message: string; code?: string }
	| { v: 1; kind: 'http'; status: number; headers: Record<string, string>; bodyJson?: string };

/** Preserve the legacy JS serializer at the temporary business-result boundary. */
export function encodeApiResult(value: unknown): ApiCallbackResult {
	if (Buffer.isBuffer(value)) return { v: 1, kind: 'bytes', base64: value.toString('base64') };
	if (value !== null && typeof value === 'object') return { v: 1, kind: 'json', resultJson: JSON.stringify(value) };
	return { v: 1, kind: 'value', value: apiEncodeValue(value) };
}

type PendingRequest = { request: FastifyRequest; claimed: boolean };

function object(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function decodeApiInvocation(value: unknown): ApiCallbackInvocation {
	if (!object(value) || value.v !== 1 || typeof value.requestId !== 'string' || !/^[a-f0-9]{64}$/.test(value.requestId)
		|| (value.kind !== 'endpoint' && value.kind !== 'special') || typeof value.endpoint !== 'string'
		|| !Object.hasOwn(value, 'params') || !Object.hasOwn(value, 'user') || !Object.hasOwn(value, 'token')
		|| Object.keys(value).some(key => !['v', 'requestId', 'kind', 'endpoint', 'params', 'user', 'token', 'file', 'routeParams'].includes(key))) {
		throw new Error('Invalid API callback');
	}
	let file: ApiCallbackInvocation['file'];
	if (value.file !== undefined && value.file !== null) {
		if (!object(value.file) || typeof value.file.path !== 'string' || value.file.path.length === 0
			|| (Object.hasOwn(value.file, 'name') && value.file.name !== null && typeof value.file.name !== 'string')
			|| Object.keys(value.file).some(key => key !== 'path' && key !== 'name')) throw new Error('Invalid API callback file');
		file = { name: typeof value.file.name === 'string' || value.file.name === null ? value.file.name : undefined, path: value.file.path };
	}
	let routeParams: ApiCallbackInvocation['routeParams'];
	if (value.routeParams !== undefined && value.routeParams !== null) {
		if (!object(value.routeParams) || typeof value.routeParams.session !== 'string' || Object.keys(value.routeParams).length !== 1) throw new Error('Invalid API callback route parameters');
		routeParams = { session: value.routeParams.session };
	}
	return {
		v: 1,
		requestId: value.requestId,
		kind: value.kind,
		endpoint: value.endpoint,
		params: apiDecodeValue(value.params),
		user: value.user,
		token: value.token,
		...(file ? { file } : {}),
		...(routeParams ? { routeParams } : {}),
	};
}

/** Private, authenticated, request-bound callback executor. It has no public route. */
export class ApiCallbackServer {
	private readonly token = randomBytes(32).toString('hex');
	private readonly pending = new Map<string, PendingRequest>();
	private server?: Server;
	private startPromise?: Promise<{ url: string; token: string }>;
	private stopPromise?: Promise<void>;

	constructor(private readonly execute: (invocation: ApiCallbackInvocation, request: FastifyRequest) => Promise<ApiCallbackResult>) {}

	register(request: FastifyRequest): { requestId: string; release: () => void } {
		const requestId = randomBytes(32).toString('hex');
		this.pending.set(requestId, { request, claimed: false });
		return { requestId, release: () => this.pending.delete(requestId) };
	}

	start(): Promise<{ url: string; token: string }> {
		return this.startPromise ??= new Promise((resolve, reject) => {
			const server = createServer(async (request, response) => {
				const supplied = request.headers.authorization;
				const expected = `Bearer ${this.token}`;
				if (request.method !== 'POST' || request.url !== '/execute' || typeof supplied !== 'string'
					|| Buffer.byteLength(supplied) !== Buffer.byteLength(expected)
					|| !timingSafeEqual(Buffer.from(supplied), Buffer.from(expected))) {
					response.writeHead(401).end();
					request.resume();
					return;
				}
				try {
					const chunks: Buffer[] = [];
					let size = 0;
					for await (const chunk of request) {
						const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
						size += bytes.length;
						if (size > MAX_CALLBACK_BYTES) { response.writeHead(413).end(); request.resume(); return; }
						chunks.push(bytes);
					}
					const invocation = decodeApiInvocation(JSON.parse(Buffer.concat(chunks).toString('utf8')));
					const pending = this.pending.get(invocation.requestId);
					if (!pending || pending.claimed) { response.writeHead(409).end(); return; }
					pending.claimed = true;
					let result: ApiCallbackResult;
					try {
						result = await this.execute(invocation, pending.request);
					} catch (error) {
						if (error instanceof ApiError) {
							result = { v: 1, kind: 'api-error', message: error.message, code: error.code, id: error.id, errorKind: error.kind, httpStatusCode: error.httpStatusCode, info: apiEncodeValue(error.info) };
						} else if (error instanceof AuthenticationError) {
							result = { v: 1, kind: 'authentication-error' };
						} else {
							// Business exceptions are logged and wrapped by ApiCallService. Never expose transport failures.
							const generic = new ApiError();
							result = { v: 1, kind: 'api-error', message: generic.message, code: generic.code, id: generic.id, errorKind: generic.kind, httpStatusCode: 500, info: apiEncodeValue(undefined) };
						}
					}
					response.writeHead(200, { 'content-type': 'application/json' }).end(apiStringifyFrame(result));
				} catch {
					if (!response.headersSent) response.writeHead(400);
					response.end();
				}
			});
			this.server = server;
			server.requestTimeout = 0;
			server.once('error', reject);
			server.listen(0, '127.0.0.1', () => {
				const address = server.address();
				if (address === null || typeof address === 'string') { reject(new Error('API callback unavailable')); return; }
				resolve({ url: `http://127.0.0.1:${address.port}/execute`, token: this.token });
			});
		});
	}

	stop(): Promise<void> {
		return this.stopPromise ??= new Promise(resolve => {
			this.pending.clear();
			if (!this.server) { resolve(); return; }
			this.server.close(() => resolve());
			this.server.closeAllConnections();
		});
	}
}
