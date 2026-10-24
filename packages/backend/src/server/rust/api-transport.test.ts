/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test, vi } from 'vitest';
import { RustBackendService, RustBackendTransport } from './RustBackendService.js';

const context = { v: 1 as const, requestId: 'a'.repeat(64), path: '/api/ping', method: 'POST', ip: '127.0.0.1', headers: {} };

test('public API reports fixed 503 when Rust fails without exposing private transport errors or retrying', async () => {
	const api = vi.spyOn(RustBackendTransport.prototype, 'api').mockRejectedValue(new Error('private connection password'));
	const service = new RustBackendService();
	try {
		const response = await service.api(context, null);
		expect(response.status).toBe(503);
		expect(response.headers.get('cache-control')).toBe('private, max-age=0, must-revalidate');
		expect(response.headers.get('access-control-allow-origin')).toBe('*');
		expect(await response.json()).toEqual({ error: { message: 'Rust backend unavailable.', code: 'RUST_BACKEND_UNAVAILABLE', id: '91600ba8-92ad-48fa-9afd-d6c615f06946', kind: 'server' } });
		expect(api).toHaveBeenCalledExactlyOnceWith(context, null, undefined);
	} finally { api.mockRestore(); await service.stop(); }
});

test('API transport streams raw body/status/headers and is not bounded by the old five-second operation deadline', async () => {
	const source = `let frame='';process.stdin.on('data',chunk=>{frame+=chunk;if(!frame.includes('\\n'))return;const b=JSON.parse(frame);const s=require('node:http').createServer(async(q,r)=>{if(q.url!=='/api-request'||q.headers.authorization!=='Bearer '+b.token){r.writeHead(401).end();return;}const c=JSON.parse(Buffer.from(q.headers['x-misskey-api-context'],'base64').toString());let body='';for await(const chunk of q)body+=chunk;setTimeout(()=>{r.writeHead(418,{'content-type':'application/json','x-api-test':'streamed'});r.end(JSON.stringify({method:c.method,body}));},5100);});s.listen(0,'127.0.0.1',()=>console.log(JSON.stringify({v:1,port:s.address().port})));process.stdin.removeAllListeners('data');process.stdin.on('end',()=>{s.closeAllConnections();s.close();});});`;
	const transport = new RustBackendTransport(process.execPath, ['-e', source]);
	try {
		await transport.start([]);
		const body = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{raw')); controller.close(); } });
		const response = await transport.api(context, body);
		expect(response.status).toBe(418);
		expect(response.headers.get('x-api-test')).toBe('streamed');
		expect(await response.json()).toEqual({ method: 'POST', body: '{raw' });
	} finally { await transport.stop(); }
});
