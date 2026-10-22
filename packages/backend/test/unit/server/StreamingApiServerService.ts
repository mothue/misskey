/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { EventEmitter } from 'node:events';
import { connect, type Socket } from 'node:net';
import type { Duplex } from 'node:stream';
import { setTimeout as delay } from 'node:timers/promises';
import Fastify from 'fastify';
import { describe, expect, test } from 'vitest';
import { mock } from 'vitest-mock-extended';
import { AuthenticationError, type AuthenticateService } from '@/server/api/AuthenticateService.js';
import { StreamingApiServerService } from '@/server/api/StreamingApiServerService.js';
import type { ModuleRef } from '@nestjs/core';
import type * as Redis from 'ioredis';
import type { UserService } from '@/core/UserService.js';
import type { MiLocalUser } from '@/models/User.js';

describe('Streaming upgrade rejection', () => {
	test.each([401, 403, 500])('flushes HTTP %i without blocking server shutdown', async status => {
		const service = new StreamingApiServerService(
			new EventEmitter() as Redis.Redis,
			{} as ModuleRef,
			mock<AuthenticateService>({
				async authenticate(): ReturnType<AuthenticateService['authenticate']> {
					if (status === 401) throw new AuthenticationError('Invalid token');
					if (status === 500) throw new Error('Authentication unavailable');
					return [{ isSuspended: true } as MiLocalUser, null];
				},
			}),
			{} as UserService,
		);
		const fastify = Fastify();
		const sockets: Duplex[] = [];
		let client: Socket | undefined;
		service.attach(fastify.server);
		fastify.server.on('upgrade', (_request, socket) => sockets.push(socket));
		try {
			await fastify.listen({ host: '127.0.0.1', port: 0 });
			const address = fastify.server.address();
			if (address == null || typeof address === 'string') throw new Error('Missing loopback address');
			const response = await new Promise<string>((resolve, reject) => {
				const socket = client = connect(address.port, '127.0.0.1');
				let received = '';
				socket.setEncoding('utf8');
				socket.setTimeout(2000, () => socket.destroy(new Error('Upgrade response timed out')));
				socket.on('error', reject);
				socket.on('close', () => {
					if (!received.includes('\r\n\r\n')) reject(new Error('Upgrade closed before response headers'));
				});
				socket.on('data', chunk => {
					received += chunk;
					if (received.includes('\r\n\r\n')) {
						socket.destroy();
						resolve(received);
					}
				});
				socket.on('connect', () => socket.write([
					'GET /streaming HTTP/1.1',
					`Host: 127.0.0.1:${address.port}`,
					'Connection: Upgrade',
					'Upgrade: websocket',
					'Sec-WebSocket-Version: 13',
					'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==',
					'Authorization: Bearer invalid',
					'', '',
				].join('\r\n')));
			});
			expect(response).toMatch(new RegExp(`^HTTP/1\\.1 ${status} `));
			expect(response).toMatch(/\r\nConnection: close\r\n/i);
			expect(response).toMatch(/\r\nContent-Length: 0\r\n/i);
			if (status === 401) {
				expect(response).toMatch(/\r\nWWW-Authenticate: Bearer realm="Misskey", error="invalid_token", error_description="Failed to authenticate"\r\n/i);
			}
			await service.detach();
			const closed = await Promise.race([
				fastify.close().then(() => true),
				delay(500).then(() => false),
			]);
			expect(closed, 'Rejected upgrades must not retain a connection during shutdown').toBe(true);
		} finally {
			client?.destroy();
			for (const socket of sockets) socket.destroy();
			await service.detach();
			await Promise.race([fastify.close(), delay(500)]);
		}
	});
});
