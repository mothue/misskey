/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import Fastify from 'fastify';
import { expect, test, vi } from 'vitest';
import { ApiServerService } from './ApiServerService.js';
import type { RustBackendService } from '../rust/RustBackendService.js';

vi.mock('@/core/RoleService.js', () => ({ DEFAULT_POLICIES: {} }));
vi.mock('@/core/entities/UserEntityService.js', () => ({ UserEntityService: class {} }));
vi.mock('@/server/rust/RustBackendService.js', () => ({ RustBackendService: class {} }));
vi.mock('@/server/rust/postgres-config.js', () => ({ resolveRustPrimaryTarget: vi.fn() }));
vi.mock('@/server/rust/redis-config.js', () => ({ resolveRustRedisConfig: vi.fn() }));
vi.mock('./ApiCallService.js', () => ({ ApiCallService: class {} }));
vi.mock('./SignupApiService.js', () => ({ SignupApiService: class {} }));
vi.mock('./SigninApiService.js', () => ({ SigninApiService: class {} }));
vi.mock('./SigninWithPasskeyApiService.js', () => ({ SigninWithPasskeyApiService: class {} }));
vi.mock('./AuthenticateService.js', () => ({ AuthenticationError: class AuthenticationError extends Error {} }));
vi.mock('./endpoints.js', () => ({ default: [] }));

test('public Fastify transport forwards raw invalid JSON, multipart and OPTIONS before TS parsers', async () => {
	const api = vi.fn(async (context: { method: string }, body: ReadableStream<Uint8Array> | null) => {
		const bytes = body === null ? '' : await new Response(body).text();
		if (context.method === 'OPTIONS') return new Response(null, { status: 204, headers: { 'access-control-allow-origin': 'rust-origin' } });
		return new Response(bytes, { status: 202, headers: { 'content-type': 'application/octet-stream', 'cache-control': 'rust-cache' } });
	});
	const span = { withApiSpan: (_name: string, callback: () => unknown) => callback() };
	const service = new ApiServerService(...[
		{}, {}, {}, {}, {}, {}, {}, span, {}, {}, {}, { api } as unknown as RustBackendService,
	] as unknown as ConstructorParameters<typeof ApiServerService>);
	const server = Fastify();
	try {
		await server.register(service.createServer, { prefix: '/api' });
		const invalid = await server.inject({ method: 'POST', url: '/api/test', headers: { 'content-type': 'application/json' }, payload: '{invalid' });
		expect(invalid.statusCode).toBe(202);
		expect(invalid.body).toBe('{invalid');
		expect(invalid.headers['cache-control']).toBe('rust-cache');
		const multipart = '--boundary\r\nContent-Disposition: form-data; name="file"; filename="x"\r\n\r\nraw\r\n--boundary--\r\n';
		const upload = await server.inject({ method: 'POST', url: '/api/drive/files/create', headers: { 'content-type': 'multipart/form-data; boundary=boundary' }, payload: multipart });
		expect(upload.body).toBe(multipart);
		const options = await server.inject({ method: 'OPTIONS', url: '/api/test', headers: { origin: 'public-origin', 'access-control-request-method': 'POST' } });
		expect(options.statusCode).toBe(204);
		expect(options.headers['access-control-allow-origin']).toBe('rust-origin');
		expect(api).toHaveBeenCalledTimes(3);
	} finally { await server.close(); await service.stop(); }
});

test('request-bound legacy callback uses invokeValidated only and preserves special business replies', async () => {
	const { apiEncodeValue } = await import('../rust/api-wire.js');
	let callback: { url: string; token: string };
	const invokeValidated = vi.fn(async (..._args: unknown[]) => ({ at: new Date(0), omitted: undefined }));
	const legacyExec = vi.fn();
	const moduleRef = { get: vi.fn(() => ({ invokeValidated, exec: legacyExec })) };
	const businessSignin = vi.fn(async (request: { body: unknown }, reply: { code: (status: number) => unknown; header: (key: string, value: string) => unknown }) => {
		reply.header('Access-Control-Allow-Origin', 'https://configured.test');
		reply.code(401);
		return { error: { id: 'business-denied' }, echoed: request.body };
	});
	const configureApi = vi.fn((config: { callback: { url: string; token: string } }) => { callback = config.callback; });
	const observed: unknown[] = [];
	const findToken = vi.fn(async () => null);
	const api = vi.fn(async (context: { method: string; path: string; requestId: string }, body: ReadableStream<Uint8Array> | null) => {
		const endpoint = decodeURIComponent(context.path.replace('/api/', ''));
		const params = body ? JSON.parse(await new Response(body).text()) : {};
		const routeParams = endpoint.startsWith('miauth/') ? { session: endpoint.slice('miauth/'.length, -'/check'.length) } : undefined;
		const response = await fetch(callback.url, { method: 'POST', headers: { authorization: `Bearer ${callback.token}`, 'content-type': 'application/json' }, body: JSON.stringify({ v: 1, requestId: context.requestId, kind: endpoint === 'signin-flow' || routeParams ? 'special' : 'endpoint', endpoint, params: apiEncodeValue(params), user: null, token: null, routeParams }) });
		expect(response.status).toBe(200);
		const result = await response.json() as { kind: string; resultJson?: string; status?: number; headers?: Record<string, string>; bodyJson?: string };
		observed.push(result);
		if (result.kind === 'http') return new Response(result.bodyJson, { status: result.status, headers: result.headers });
		return new Response(result.resultJson, { status: 200, headers: { 'content-type': 'application/json' } });
	});
	const span = {
		withApiSpan: (_name: string, callback: () => unknown) => callback(),
		executeLegacy: async (_name: string, endpoint: { invokeValidated: (...args: unknown[]) => Promise<unknown> }, params: unknown, user: unknown, token: unknown, file: unknown, ip: unknown, headers: unknown, serialize: (value: unknown) => unknown) => serialize(await endpoint.invokeValidated(params, user, token, file, ip, headers)),
	};
	const metadata = { columns: [], create: () => ({}) };
	const service = new ApiServerService(...[
		moduleRef, { maxFileSize: 1024, id: 'aidx', host: 'local.test' }, { policies: {}, enableIpLogging: false }, { getMetadata: () => metadata }, {}, { findOneBy: findToken }, {}, span, {}, { signin: businessSignin }, {}, { api, configureApi },
	] as unknown as ConstructorParameters<typeof ApiServerService>);
	const server = Fastify();
	try {
		const endpoints = (await import('./endpoints.js')).default;
		endpoints.push({ name: 'emoji', meta: {}, params: { type: 'object' } });
		await service.prepareRustApi();
		await server.register(service.createServer, { prefix: '/api' });
		const ordinary = await server.inject({ method: 'POST', url: '/api/emoji', payload: { name: 'passed-by-rust' } });
		expect(ordinary.statusCode).toBe(200);
		expect(ordinary.json()).toEqual({ at: '1970-01-01T00:00:00.000Z' });
		expect(invokeValidated).toHaveBeenCalledOnce();
		expect(invokeValidated.mock.calls[0][0]).toEqual({ name: 'passed-by-rust' });
		expect(legacyExec).not.toHaveBeenCalled();
		const special = await server.inject({ method: 'POST', url: '/api/signin-flow', payload: { username: 'from-rust' } });
		expect(special.statusCode).toBe(401);
		expect(special.headers['access-control-allow-origin']).toBe('https://configured.test');
		expect(special.json()).toEqual({ error: { id: 'business-denied' }, echoed: { username: 'from-rust' } });
		expect(businessSignin).toHaveBeenCalledOnce();
		const encodedSession = await server.inject({ method: 'POST', url: '/api/miauth/a%2Fb/check', payload: {} });
		expect(encodedSession.statusCode).toBe(200);
		expect(encodedSession.json()).toEqual({ ok: false });
		expect(findToken).toHaveBeenCalledExactlyOnceWith({ session: 'a/b' });
		expect(observed).toHaveLength(3);
	} finally { await server.close(); await service.stop(); }
});

test.each([false, true])('an early response reaches an incomplete upload and retains its socket (local 503: %s)', async localUnavailable => {
	const { createServer, request: httpRequest, Agent } = await import('node:http');
	const upstream = createServer((_request, response) => { response.writeHead(413, { 'content-type': 'text/plain' }).end('limit'); });
	await new Promise<void>(resolve => upstream.listen(0, '127.0.0.1', resolve));
	const upstreamAddress = upstream.address();
	if (!upstreamAddress || typeof upstreamAddress === 'string') throw new Error('Expected upstream TCP address');
	const api = async (_context: unknown, body: ReadableStream<Uint8Array> | null, signal: AbortSignal) => localUnavailable ? new Response('offline', { status: 503 }) : fetch(`http://127.0.0.1:${upstreamAddress.port}`, { method: 'POST', body, signal, duplex: 'half' } as RequestInit);
	const span = { withApiSpan: (_name: string, callback: () => unknown) => callback() };
	const service = new ApiServerService(...[{}, {}, {}, {}, {}, {}, {}, span, {}, {}, {}, { api }] as unknown as ConstructorParameters<typeof ApiServerService>);
	const server = Fastify();
	let incoming: import('node:http').IncomingMessage | undefined;
	server.addHook('onRequest', async request => { if (request.url.startsWith('/api/')) incoming = request.raw; });
	server.get('/healthy', () => 'alive');
	await server.register(service.createServer, { prefix: '/api' });
	await server.listen({ host: '127.0.0.1', port: 0 });
	const address = server.server.address();
	if (!address || typeof address === 'string') throw new Error('Expected public TCP address');
	const agent = new Agent({ keepAlive: true, maxSockets: 1 });
	let upload: import('node:http').ClientRequest | undefined;
	try {
		const fullSize = 2 * 1024 * 1024;
		const firstSize = 64 * 1024;
		const response = new Promise<{ status?: number; body: string }>((resolve, reject) => {
			upload = httpRequest({ host: '127.0.0.1', port: address.port, method: 'POST', path: '/api/drive/files/create', agent, signal: AbortSignal.timeout(5000), headers: { 'content-type': 'multipart/form-data; boundary=slow', 'content-length': fullSize } }, reply => {
				const chunks: Buffer[] = [];
				reply.on('data', chunk => chunks.push(Buffer.from(chunk)));
				reply.once('end', () => resolve({ status: reply.statusCode, body: Buffer.concat(chunks).toString() }));
				reply.once('error', reject);
			});
			upload.once('error', reject);
			upload.write(Buffer.alloc(firstSize, 0x61));
		});
		expect(await response).toEqual(localUnavailable ? { status: 503, body: 'offline' } : { status: 413, body: 'limit' });
		expect(incoming?.complete).toBe(false);
		expect(incoming?.destroyed).toBe(false);
		const firstSocket = upload!.socket;
		const ended = new Promise<void>(resolve => incoming!.once('end', resolve));
		upload!.end(Buffer.alloc(fullSize - firstSize, 0x62));
		await ended;
		const healthy = await new Promise<{ body: string; socket: import('node:net').Socket | null }>((resolve, reject) => {
			const request = httpRequest({ host: '127.0.0.1', port: address.port, method: 'GET', path: '/healthy', agent, signal: AbortSignal.timeout(5000) }, reply => {
				const socket = request.socket;
				const chunks: Buffer[] = [];
				reply.on('data', chunk => chunks.push(Buffer.from(chunk)));
				reply.once('end', () => resolve({ body: Buffer.concat(chunks).toString(), socket }));
			});
			request.once('error', reject);
			request.end();
		});
		expect(healthy.body).toBe('alive');
		expect(healthy.socket).toBe(firstSocket);
	} finally {
		upload?.destroy();
		agent.destroy();
		await server.close();
		await service.stop();
		upstream.closeAllConnections();
		await new Promise<void>(resolve => upstream.close(() => resolve()));
	}
});

test('disconnect releases the callback capability before an outstanding Rust response settles', async () => {
	const { request: httpRequest } = await import('node:http');
	const { apiEncodeValue } = await import('../rust/api-wire.js');
	let callback: { url: string; token: string };
	let requestId = '';
	let signal: AbortSignal | undefined;
	let entered: () => void = () => {};
	const apiEntered = new Promise<void>(resolve => { entered = resolve; });
	let finish: (response: Response) => void = () => {};
	const waiting = new Promise<Response>(resolve => { finish = resolve; });
	const api = async (context: { requestId: string }, _body: unknown, requestSignal: AbortSignal) => { requestId = context.requestId; signal = requestSignal; entered(); return waiting; };
	const configureApi = (config: { callback: { url: string; token: string } }) => { callback = config.callback; };
	const span = { withApiSpan: (_name: string, callback: () => unknown) => callback() };
	const metadata = { columns: [], create: () => ({}) };
	const service = new ApiServerService(...[{}, { maxFileSize: 1024, id: 'aidx', host: 'local.test' }, { policies: {} }, { getMetadata: () => metadata }, {}, {}, {}, span, {}, {}, {}, { api, configureApi }] as unknown as ConstructorParameters<typeof ApiServerService>);
	const server = Fastify();
	await service.prepareRustApi();
	await server.register(service.createServer, { prefix: '/api' });
	await server.listen({ host: '127.0.0.1', port: 0 });
	const address = server.server.address();
	if (!address || typeof address === 'string') throw new Error('Expected public TCP address');
	const request = httpRequest({ host: '127.0.0.1', port: address.port, method: 'POST', path: '/api/emoji', headers: { 'content-length': 1024 * 1024 } });
	request.on('error', () => {});
	try {
		request.write(Buffer.alloc(64 * 1024));
		await apiEntered;
		request.destroy();
		await expect.poll(() => signal?.aborted).toBe(true);
		const response = await fetch(callback!.url, { method: 'POST', headers: { authorization: `Bearer ${callback!.token}`, 'content-type': 'application/json' }, body: JSON.stringify({ v: 1, requestId, kind: 'endpoint', endpoint: 'emoji', params: apiEncodeValue({}), user: null, token: null }) });
		expect(response.status).toBe(409);
	} finally {
		request.destroy();
		finish(new Response('discarded'));
		await server.close();
		await service.stop();
	}
});
