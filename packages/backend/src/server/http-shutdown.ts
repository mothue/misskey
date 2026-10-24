/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import type { Socket } from 'node:net';
import type { FastifyInstance } from 'fastify';

/** Track raw connections too: Fastify cannot account for manually upgraded sockets. */
export class HttpServerShutdown {
	private readonly sockets = new Set<Socket>();
	private readonly connected = (socket: Socket): void => {
		this.sockets.add(socket);
		socket.once('close', () => this.sockets.delete(socket));
	};
	constructor(private readonly fastify: FastifyInstance) {
		fastify.server.on('connection', this.connected);
	}
	async close(detach: () => Promise<void>): Promise<void> {
		let timer: NodeJS.Timeout | undefined;
		try {
			await Promise.race([
				Promise.allSettled([
					Promise.resolve().then(detach),
					this.fastify.close(),
				]).then(results => {
					for (const result of results) if (result.status === 'rejected') throw result.reason;
				}),
				new Promise<void>(resolve => {
					timer = setTimeout(() => {
						for (const socket of this.sockets) socket.destroy();
						resolve();
					}, 5000);
				}),
			]);
		} finally {
			clearTimeout(timer);
			// Even a failing pre-close hook must not leave the old listener accepting.
			if (this.fastify.server.listening) this.fastify.server.close();
			for (const socket of this.sockets) socket.destroy();
			this.fastify.server.off('connection', this.connected);
		}
	}
}
