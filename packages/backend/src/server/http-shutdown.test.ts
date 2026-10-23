/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { once } from 'node:events';
import Fastify from 'fastify';
import { WebSocket, WebSocketServer } from 'ws';
import { expect, test } from 'vitest';
import { RustBackendTransport } from './rust/RustBackendService.js';
import { findCodeRoot } from './rust/source-root.js';
import { join } from 'node:path';
import { HttpServerShutdown } from './http-shutdown.js';

test('open upgraded connection cannot keep HTTP accepting after bounded disposal', async () => {
	const fastify = Fastify();
	fastify.get('/', () => 'alive');
	const shutdown = new HttpServerShutdown(fastify);
	const wss = new WebSocketServer({ server: fastify.server });
	await fastify.listen({ host: '127.0.0.1', port: 0 });
	const address = fastify.server.address();
	if (!address || typeof address === 'string') throw new Error('Expected TCP address');
	const url = `http://127.0.0.1:${address.port}`;
	const socket = new WebSocket(url.replace('http:', 'ws:'));
	await once(socket, 'open');
	const child = new RustBackendTransport(join(findCodeRoot(), 'packages/backend-rust/built/bin', `misskey-backend-rust${process.platform === 'win32' ? '.exe' : ''}`));
	await child.start([]);
	try {
		const disposal = shutdown.close(() => new Promise<void>(resolve => wss.close(() => resolve())));
		await expect.poll(() => fastify.server.listening, { timeout: 1000 }).toBe(false);
		await disposal;
		await child.stop();
		expect(fastify.server.listening).toBe(false);
		await expect(fetch(url)).rejects.toThrow();
		await expect.poll(() => socket.readyState).toBe(WebSocket.CLOSED);
		expect(child.isReady()).toBe(false);
	} finally {
		await child.stop();
		socket.terminate();
		for (const client of wss.clients) client.terminate();
		await fastify.close();
	}
});

test('failure before websocket attach still closes the HTTP listener', async () => {
	const fastify = Fastify();
	const shutdown = new HttpServerShutdown(fastify);
	await fastify.listen({ host: '127.0.0.1', port: 0 });
	await expect(shutdown.close(() => { throw new Error('Not attached'); })).rejects.toThrow('Not attached');
	expect(fastify.server.listening).toBe(false);
});
