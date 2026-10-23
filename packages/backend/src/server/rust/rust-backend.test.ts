/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { fileURLToPath } from 'node:url';
import { expect, test, vi } from 'vitest';
import { decodePing, decodeTest, decodeEndpoint, decodeEndpoints, encodeUtf16 } from './protocol.js';

test('ping rejects unsafe numbers and wrong envelopes', () => {
 expect(decodePing({ v: 1, op: 'ping', pong: 42 })).toEqual({ pong: 42 });
 for (const value of [{ v: 2, op: 'ping', pong: 42 }, { v: 1, op: 'test', pong: 42 }, { v: 1, op: 'ping', pong: 1.5 }, { v: 1, op: 'ping', pong: 9007199254740992 }]) expect(() => decodePing(value)).toThrow();
});
test('test restores exact opaque JSON including surrogates and extras', () => {
 const json = '{"required":true,"nullableDefault":"hello","extra":{"x":"\\ud800"},"n":1e+30}';
 expect(decodeTest({ v: 1, op: 'test', resultJson: json }, json)).toEqual({ required: true, nullableDefault: 'hello', extra: { x: '\ud800' }, n: 1e30 });
 expect(() => decodeTest({ v: 1, op: 'test', resultJson: '{}' }, json)).toThrow();
 expect(() => decodeTest({ v: 1, op: 'test', resultJson: '[]' }, '[]')).toThrow();
});
test('endpoint preserves UTF16 code units and rejects invalid values', () => {
 expect(encodeUtf16('😀\ud800')).toEqual([55357, 56832, 55296]);
 expect(decodeEndpoint({ v: 1, op: 'endpoint', result: { params: [{ nameUtf16: [55296], type: 'string' }] } })).toEqual({ params: [{ name: '\ud800', type: 'string' }] });
 expect(decodeEndpoint({ v: 1, op: 'endpoint', result: null })).toBeNull();
 expect(() => decodeEndpoint({ v: 1, op: 'endpoint', result: { params: [{ nameUtf16: [65536], type: 'String' }] } })).toThrow();
});

test('endpoints decoder preserves empty names, order, duplicates and exact UTF16 units', () => {
	const names = ['z', '', '\ud800', '\udc00', '😀\u0000\uffff', 'a', 'z'];
	expect(decodeEndpoints({ v: 1, op: 'endpoints', endpointsUtf16: names.map(encodeUtf16) })).toEqual(names);
	expect(decodeEndpoints({ v: 1, op: 'endpoints', endpointsUtf16: [] })).toEqual([]);
});

test('endpoints decoder rejects malformed envelopes, lists and UTF16 units', () => {
	for (const value of [
		null, [], {},
		{ v: 2, op: 'endpoints', endpointsUtf16: [] },
		{ v: 1, op: 'endpoint', endpointsUtf16: [] },
		{ v: 1, op: 'endpoints' },
		{ v: 1, op: 'endpoints', endpointsUtf16: null },
		{ v: 1, op: 'endpoints', endpointsUtf16: 'ping' },
		{ v: 1, op: 'endpoints', endpointsUtf16: [null] },
		{ v: 1, op: 'endpoints', endpointsUtf16: ['ping'] },
		{ v: 1, op: 'endpoints', endpointsUtf16: new Array(1) },
		{ v: 1, op: 'endpoints', endpointsUtf16: [new Array(1)] },
	]) expect(() => decodeEndpoints(value)).toThrow();
	for (const unit of [-1, 65536, 0.5, NaN, Infinity, '97', null, {}]) {
		expect(() => decodeEndpoints({ v: 1, op: 'endpoints', endpointsUtf16: [[unit]] })).toThrow();
	}
});

test('endpoints service maps invalid RPC data and transport errors to503 without retry', async () => {
	const { RustBackendService, RustBackendTransport } = await import('./RustBackendService.js');
	const rpc = vi.spyOn(RustBackendTransport.prototype, 'rpc');
	const service = new RustBackendService();
	try {
		for (const value of [
			{ v: 2, op: 'endpoints', endpointsUtf16: [] },
			{ v: 1, op: 'endpoint', endpointsUtf16: [] },
			{ v: 1, op: 'endpoints', endpointsUtf16: null },
			{ v: 1, op: 'endpoints', endpointsUtf16: [[65536]] },
		]) {
			rpc.mockResolvedValueOnce(value);
			await expect(service.endpoints()).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503, kind: 'server', id: '91600ba8-92ad-48fa-9afd-d6c615f06946' });
			expect(rpc).toHaveBeenCalledExactlyOnceWith({ v: 1, op: 'endpoints' });
			rpc.mockClear();
		}
		rpc.mockRejectedValueOnce(new Error('private transport detail'));
		await expect(service.endpoints()).rejects.toMatchObject({ message: 'Rust backend unavailable.', code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503 });
		expect(rpc).toHaveBeenCalledExactlyOnceWith({ v: 1, op: 'endpoints' });
	} finally { rpc.mockRestore(); await service.stop(); }
});

test('endpoints service uses the real child catalog and does not fall back after stop', async () => {
	const { RustBackendService } = await import('./RustBackendService.js');
	const names = ['z', '', '\ud800', '😀', 'a', 'z'];
	vi.doMock('../api/endpoints.js', () => ({ default: names.map(name => ({ name, params: { properties: {} } })) }));
	const service = new RustBackendService();
	try {
		await service.start();
		expect(await service.endpoints()).toEqual(names);
		await service.stop();
		await expect(service.endpoints()).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503 });
		expect(service.isReady()).toBe(false);
	} finally { await service.stop(); vi.doUnmock('../api/endpoints.js'); }
});

test('transport starts one child and stop is terminal', async () => {
 const { RustBackendTransport } = await import('./RustBackendService.js');
 const source = `const http=require('node:http');let bytes='';process.stdin.on('data',c=>{bytes+=c;if(!bytes.includes('\\n'))return;const b=JSON.parse(bytes);if(b.v!==1||!/^[a-f0-9]{64}$/.test(b.token)||!Array.isArray(b.endpoints))process.exit(8);const s=http.createServer((q,r)=>{if(q.headers.authorization!=='Bearer '+b.token){r.statusCode=401;r.end();return;}let t='';q.on('data',c=>t+=c);q.on('end',()=>{const x=JSON.parse(t);r.end(JSON.stringify({v:1,op:x.op,pong:123}));});});s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));process.stdin.removeAllListeners('data');process.stdin.on('end',()=>s.close());});`;
 const transport = new RustBackendTransport(process.execPath, ['-e', source]);
 try {
  await Promise.all([transport.start([]), transport.start([])]);
  expect(transport.isReady()).toBe(true);
  expect(decodePing(await transport.rpc({ v: 1, op: 'ping' }))).toEqual({ pong: 123 });
 } finally { await Promise.all([transport.stop(), transport.stop()]); }
 expect(transport.isReady()).toBe(false);
 await expect(transport.start([])).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503 });
 await expect(transport.rpc({ v: 1, op: 'ping' })).rejects.toMatchObject({ kind: 'server' });
});
test('transport cleans malformed startup and cannot restart', async () => {
 const { RustBackendTransport } = await import('./RustBackendService.js');
 const transport = new RustBackendTransport(process.execPath, ['-e', `process.stdin.resume();console.log('{"v":2,"port":12}');`]);
 await expect(transport.start([])).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE' });
 expect(transport.isReady()).toBe(false);
 await transport.stop();
 await expect(transport.start([])).rejects.toThrow();
});
test('source root is independent of current working directory and bundle depth', async () => {
 const { findCodeRoot } = await import('./source-root.js');
 expect(findCodeRoot(new URL('../../../../built/entry.js', import.meta.url).href)).toBe(fileURLToPath(new URL('../../../../../', import.meta.url)).replace(/\/$/, ''));
});

test('child exit removes readiness and aborts selected API calls', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const source = `process.stdin.once('data',()=>{const s=require('node:http').createServer(()=>process.exit(7));s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));});`;
	const transport = new RustBackendTransport(process.execPath, ['-e', source]);
	try {
		await transport.start([]);
		await expect(transport.rpc({ v: 1, op: 'ping' })).rejects.toMatchObject({ httpStatusCode: 503 });
		await expect.poll(() => transport.isReady()).toBe(false);
	} finally { await transport.stop(); }
});
test('call timeout leaves a live child ready', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const source = `process.stdin.once('data',()=>{let count=0;const s=require('node:http').createServer((q,r)=>{count++;if(count>1)r.end(JSON.stringify({v:1,op:'ping',pong:count}));});s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));});process.stdin.on('end',()=>process.exit(0));`;
	const transport = new RustBackendTransport(process.execPath, ['-e', source]);
	try {
		await transport.start([]);
		await expect(transport.rpc({ v: 1, op: 'ping' })).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE' });
		expect(transport.isReady()).toBe(true);
		expect(decodePing(await transport.rpc({ v: 1, op: 'ping' }))).toEqual({ pong: 2 });
	} finally { await transport.stop(); }
});
test('oversized ready frame is rejected and stop during startup is safe', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const oversized = new RustBackendTransport(process.execPath, ['-e', `process.stdin.resume();process.stdout.write('x'.repeat(4097));`]);
	await expect(oversized.start([])).rejects.toMatchObject({ httpStatusCode: 503 });
	await oversized.stop();
	const waiting = new RustBackendTransport(process.execPath, ['-e', `process.stdin.resume();`]);
	const startup = waiting.start([]);
	const failure = expect(startup).rejects.toMatchObject({ httpStatusCode: 503 });
	await waiting.stop();
	await failure;
	expect(waiting.isReady()).toBe(false);
});

test('Nest context close stops the shared child without constructor startup', async () => {
	const { NestFactory } = await import('@nestjs/core');
	const { RustBackendModule } = await import('./RustBackendModule.js');
	const { RustBackendService } = await import('./RustBackendService.js');
	const app = await NestFactory.createApplicationContext(RustBackendModule, { logger: false });
	const service = app.get(RustBackendService);
	expect(service.isReady()).toBe(false);
	// The real registry imports Bun-only AP modules; Node's Vitest runtime cannot load those.
	vi.doMock('../api/endpoints.js', () => ({ default: [] }));
	await service.start();
	expect(service.isReady()).toBe(true);
	expect(await service.ping()).toEqual({ pong: expect.any(Number) });
	await app.close();
	vi.doUnmock('../api/endpoints.js');
	expect(service.isReady()).toBe(false);
	await expect(service.start()).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE' });
});

test('missing executable fails startup with fixed unavailable error and terminal stop', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const transport = new RustBackendTransport(fileURLToPath(new URL('./not-a-real-executable', import.meta.url)));
	await expect(transport.start([])).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503, kind: 'server', id: '91600ba8-92ad-48fa-9afd-d6c615f06946' });
	expect(transport.isReady()).toBe(false);
	await transport.stop();
	await expect(transport.start([])).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE' });
});

test('startup times out a live silent child and cleans its process', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const { mkdtemp, readFile, rm } = await import('node:fs/promises');
	const { tmpdir } = await import('node:os');
	const { join } = await import('node:path');
	const directory = await mkdtemp(join(tmpdir(), 'misskey-rust-startup-'));
	const pidPath = join(directory, 'pid');
	const source = `require('node:fs').writeFileSync(process.argv[1],String(process.pid));process.stdin.resume();process.stdin.on('end',()=>process.exit(0));setInterval(()=>{},1000);`;
	const transport = new RustBackendTransport(process.execPath, ['-e', source, pidPath]);
	const startedAt = performance.now();
	try {
		await expect(transport.start([])).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE' });
		const elapsed = performance.now() - startedAt;
		expect(elapsed).toBeGreaterThanOrEqual(9500);
		expect(elapsed).toBeLessThan(15000);
		const pid = Number(await readFile(pidPath, 'utf8'));
		expect(transport.isReady()).toBe(false);
		await expect.poll(() => { try { process.kill(pid, 0); return false; } catch { return true; } }).toBe(true);
	} finally { await transport.stop(); await rm(directory, { recursive: true, force: true }); }
});

for (const mode of ['malformed JSON', 'oversized streamed JSON'] as const) {
	test(`${mode} RPC fails without retry and leaves live child ready`, async () => {
		const { RustBackendTransport } = await import('./RustBackendService.js');
		const firstResponse = mode === 'malformed JSON' ? "r.end('{');" : "r.write('\"');for(let i=0;i<15;i++)r.write('x'.repeat(1024*1024));r.write('x'.repeat(1024*1024-1));r.end('\"');";
		const source = `process.stdin.once('data',()=>{let count=0;const s=require('node:http').createServer((q,r)=>{count++;if(count===1){${firstResponse}}else r.end(JSON.stringify({v:1,op:'ping',pong:count}));});s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));});process.stdin.on('end',()=>process.exit(0));`;
		const transport = new RustBackendTransport(process.execPath, ['-e', source]);
		try {
			await transport.start([]);
			await expect(transport.rpc({ v: 1, op: 'ping' })).rejects.toMatchObject({ code: 'RUST_BACKEND_UNAVAILABLE', httpStatusCode: 503 });
			expect(transport.isReady()).toBe(true);
			expect(decodePing(await transport.rpc({ v: 1, op: 'ping' }))).toEqual({ pong: 2 });
		} finally { await transport.stop(); }
	});
}

test('RPC accepts a streamed JSON response exactly16MiB before rejecting larger responses', async () => {
	const { RustBackendTransport } = await import('./RustBackendService.js');
	const source = `process.stdin.once('data',()=>{const s=require('node:http').createServer((q,r)=>{r.write('"');for(let i=0;i<15;i++)r.write('x'.repeat(1024*1024));r.write('x'.repeat(1024*1024-2));r.end('"');});s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));});process.stdin.on('end',()=>process.exit(0));`;
	const transport = new RustBackendTransport(process.execPath, ['-e', source]);
	try {
		await transport.start([]);
		const result = await transport.rpc({ v: 1, op: 'ping' });
		expect(typeof result).toBe('string');
		if (typeof result !== 'string') throw new Error('Expected opaque parsed string');
		expect(result.length).toBe(16 * 1024 * 1024 - 2);
		expect(transport.isReady()).toBe(true);
	} finally { await transport.stop(); }
});
