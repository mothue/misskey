/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { join } from 'node:path';
import { Inject, Injectable, Optional, type OnApplicationShutdown } from '@nestjs/common';
import type { Config } from '@/config.js';
import { DI } from '@/di-symbols.js';
import { ApiError } from '@/server/api/error.js';
import { decodeEndpoint, decodeEndpoints, decodeOnlineUsersCount, decodePing, decodeReady, decodeTest, encodeUtf16 } from './protocol.js';
import type { CatalogEndpoint, TestResult } from './protocol.js';
import { findCodeRoot } from './source-root.js';
import { resolveRustPostgresConfig, type RustPostgresConfig } from './postgres-config.js';

const MAX_BYTES = 16 * 1024 * 1024;
const unavailable = () => new ApiError({ message: 'Rust backend unavailable.', code: 'RUST_BACKEND_UNAVAILABLE', id: '91600ba8-92ad-48fa-9afd-d6c615f06946', kind: 'server', httpStatusCode: 503 });

/** One transport instance owns one child lifetime. No restart or fallback. */
export class RustBackendTransport {
	private child?: ChildProcessWithoutNullStreams;
	private startPromise?: Promise<void>;
	private stopPromise?: Promise<void>;
	private stopped = false;
	private ready = false;
	private port = 0;
	private nativeEndpoints: ReadonlySet<string> = new Set();
	private readonly token = randomBytes(32).toString('hex');
	private readonly pending = new Set<AbortController>();
	constructor(private readonly executable: string, private readonly args: string[] = []) {}
	isReady(): boolean { return this.ready; }
	isNativeEndpoint(name: string): boolean {
		if (!this.ready) throw unavailable();
		return this.nativeEndpoints.has(name);
	}
	start(endpoints: readonly CatalogEndpoint[], postgres: RustPostgresConfig | null = null, api?: unknown): Promise<void> {
		if (this.stopped) return Promise.reject(unavailable());
		return this.startPromise ??= this.launch(endpoints, postgres, api).catch(async () => { await this.stop(); throw unavailable(); });
	}
	private async launch(endpoints: readonly CatalogEndpoint[], postgres: RustPostgresConfig | null, api?: unknown): Promise<void> {
		const bootstrap = Buffer.from(`${JSON.stringify({ v: 1, token: this.token, endpoints, postgres, ...(api === undefined ? {} : { api }) })}\n`);
		if (bootstrap.length > MAX_BYTES) throw unavailable();
		// UTF-16 catalog names conservatively bound their JSON text in readiness;
		// retain fixed envelope overhead without an endpoint-count constant.
		const readyLimit = Math.min(MAX_BYTES, 4096 + Buffer.byteLength(JSON.stringify(endpoints.map(endpoint => endpoint.nameUtf16))));
		const child = spawn(this.executable, this.args, { stdio: ['pipe', 'pipe', 'pipe'] });
		this.child = child;
		child.stderr.pipe(process.stderr, { end: false });
		child.stdin.on('error', () => {});
		child.on('exit', () => { this.ready = false; for (const pending of this.pending) pending.abort(); });
		await new Promise<void>((resolve, reject) => {
			let bytes = Buffer.alloc(0);
			let received = false;
			const timer = setTimeout(() => reject(unavailable()), 10_000);
			const finish = (error?: Error) => { clearTimeout(timer); if (error) reject(error); else resolve(); };
			child.once('error', () => finish(unavailable()));
			child.once('exit', () => finish(unavailable()));
			child.stdout.on('data', (chunk: Buffer) => {
				if (received || bytes.length + chunk.length > readyLimit) { this.ready = false; void this.stop(); finish(unavailable()); return; }
				bytes = Buffer.concat([bytes, chunk]);
				const newline = bytes.indexOf(10);
				if (newline < 0) return;
				try {
					const frame: unknown = JSON.parse(bytes.subarray(0, newline).toString('utf8'));
					if (newline !== bytes.length - 1 || this.stopped) throw unavailable();
					const ready = decodeReady(frame, endpoints);
					this.port = ready.port;
					this.nativeEndpoints = ready.nativeEndpoints;
					this.ready = true;
					received = true;
					finish();
				} catch { finish(unavailable()); }
			});
			child.stdin.write(bootstrap, error => { if (error) finish(unavailable()); });
		});
	}
	async rpc(request: Record<string, unknown>): Promise<unknown> {
		if (!this.ready) throw unavailable();
		const controller = new AbortController();
		this.pending.add(controller);
		const timer = setTimeout(() => controller.abort(), 5000);
		try {
			const body = JSON.stringify(request);
			if (Buffer.byteLength(body) > MAX_BYTES) throw unavailable();
			const response = await fetch(`http://127.0.0.1:${this.port}/rpc`, { method: 'POST', headers: { authorization: `Bearer ${this.token}`, 'content-type': 'application/json' }, body, signal: controller.signal, redirect: 'error' });
			if (!response.ok || !response.body) { await response.body?.cancel(); throw unavailable(); }
			const reader = response.body.getReader();
			const chunks: Uint8Array[] = [];
			let size = 0;
			for (;;) {
				const { done, value } = await reader.read();
				if (done) break;
				size += value.byteLength;
				if (size > MAX_BYTES) { await reader.cancel(); throw unavailable(); }
				chunks.push(value);
			}
			return JSON.parse(Buffer.concat(chunks).toString('utf8'));
		} catch { throw unavailable(); } finally { clearTimeout(timer); this.pending.delete(controller); }
	}
	async api(context: Record<string, unknown>, body: ReadableStream<Uint8Array> | null, signal?: AbortSignal): Promise<Response> {
		if (!this.ready) throw unavailable();
		const controller = new AbortController();
		this.pending.add(controller);
		const cleanup = () => this.pending.delete(controller);
		try {
			const response = await fetch(`http://127.0.0.1:${this.port}/api-request`, {
				method: 'POST',
				headers: { authorization: `Bearer ${this.token}`, 'x-misskey-api-context': Buffer.from(JSON.stringify(context)).toString('base64') },
				body,
				signal: signal ? AbortSignal.any([signal, controller.signal]) : controller.signal,
				redirect: 'error',
				// Node and Bun require this for streamed HTTP request bodies.
				duplex: 'half',
			} as RequestInit);
			if (!response.body) { cleanup(); return response; }
			const reader = response.body.getReader();
			const stream = new ReadableStream<Uint8Array>({
				async pull(target) {
					try {
						const { done, value } = await reader.read();
						if (done) { cleanup(); target.close(); } else target.enqueue(value);
					} catch (error) { cleanup(); target.error(error); }
				},
				async cancel(reason) { controller.abort(); cleanup(); await reader.cancel(reason); },
			});
			return new Response(stream, { status: response.status, headers: response.headers });
		} catch { cleanup(); throw unavailable(); }
	}
	stop(): Promise<void> {
		this.stopped = true;
		this.ready = false;
		for (const pending of this.pending) pending.abort();
		return this.stopPromise ??= (async () => {
			const child = this.child;
			if (!child || child.exitCode !== null || child.signalCode !== null) return;
			const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
			const wait = async (ms: number) => {
				let timer: NodeJS.Timeout | undefined;
				try { await Promise.race([exited, new Promise<void>(resolve => { timer = setTimeout(resolve, ms); })]); } finally { clearTimeout(timer); }
			};
			child.stdin.end();
			await wait(2000);
			if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await wait(1000); }
		})();
	}
}

@Injectable()
export class RustBackendService implements OnApplicationShutdown {
	private readonly transport = new RustBackendTransport(join(findCodeRoot(), 'packages/backend-rust/built/bin', `misskey-backend-rust${process.platform === 'win32' ? '.exe' : ''}`));
	constructor(@Optional() @Inject(DI.config) private readonly config?: Pick<Config, 'db' | 'dbReplications' | 'dbSlaves'>) {}
	private apiConfig?: unknown;
	configureApi(config: unknown): void { this.apiConfig = config; }
	async start(): Promise<void> {
		try {
			const { default: endpoints } = await import('../api/endpoints.js');
			const postgres = await resolveRustPostgresConfig(this.config);
			await this.transport.start(endpoints.map(endpoint => ({ nameUtf16: encodeUtf16(endpoint.name), properties: Object.entries(endpoint.params.properties ?? {}).map(([name, schema]) => ({ nameUtf16: encodeUtf16(name), type: schema.type ?? null })) })), postgres, this.apiConfig);
		} catch { throw unavailable(); }
	}
	stop(): Promise<void> { return this.transport.stop(); }
	onApplicationShutdown(): Promise<void> { return this.stop(); }
	isReady(): boolean { return this.transport.isReady(); }
	isNativeEndpoint(name: string): boolean { return this.transport.isNativeEndpoint(name); }
	async api(context: { v: 1; requestId: string; path: string; method: string; ip: string; headers: Record<string, string | string[] | undefined> }, body: ReadableStream<Uint8Array> | null, signal?: AbortSignal): Promise<Response> {
		try { return await this.transport.api(context, body, signal); } catch {
			const error = unavailable();
			return new Response(JSON.stringify({ error: { message: error.message, code: error.code, id: error.id, kind: error.kind } }), { status: 503, headers: { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'private, max-age=0, must-revalidate', 'access-control-allow-origin': '*' } });
		}
	}
	async ping(): Promise<{ pong: number }> {
		try { return decodePing(await this.transport.rpc({ v: 1, op: 'ping' })); } catch { throw unavailable(); }
	}
	async test(params: Record<string, unknown>): Promise<TestResult> {
		const paramsJson = JSON.stringify(params);
		try { return decodeTest(await this.transport.rpc({ v: 1, op: 'test', paramsJson }), paramsJson); } catch { throw unavailable(); }
	}
	async endpoint(name: string): Promise<{ params: { name: string; type: string }[] } | null> {
		try { return decodeEndpoint(await this.transport.rpc({ v: 1, op: 'endpoint', endpointUtf16: encodeUtf16(name) })); } catch { throw unavailable(); }
	}
	async endpoints(): Promise<string[]> {
		try { return decodeEndpoints(await this.transport.rpc({ v: 1, op: 'endpoints' })); } catch { throw unavailable(); }
	}
	async getOnlineUsersCount(): Promise<{ count: number }> {
		try { return decodeOnlineUsersCount(await this.transport.rpc({ v: 1, op: 'get-online-users-count' })); } catch { throw unavailable(); }
	}
}
