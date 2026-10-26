/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import type { Readable } from 'node:stream';

/**
 * Forward an HTTP request body without giving fetch ownership of the public
 * socket. An early upstream response cancels its request stream; toWeb would
 * destroy IncomingMessage here and prevent sending that response to the client.
 */
export function apiRequestBody(source: Readable): { stream: ReadableStream<Uint8Array>; drain: () => void } {
	let active = true;
	let detach = () => {};
	let target: ReadableStreamDefaultController<Uint8Array>;
	const drainSource = () => {
		if (source.readableEnded || source.closed) return;
		// An aborted IncomingMessage emits 'aborted', then 'error', then 'close'.
		// Keep an error listener during draining even after forwarding detaches.
		const ignore = () => {};
		const finished = () => {
			source.off('error', ignore);
			source.off('end', finished);
			source.off('close', finished);
		};
		source.on('error', ignore);
		source.once('end', finished);
		source.once('close', finished);
		if (!source.destroyed) source.resume();
	};
	const stream = new ReadableStream<Uint8Array>({
		start(controller) {
			target = controller;
			source.pause();
			const data = (chunk: Buffer | Uint8Array | string) => {
				if (!active) return;
				controller.enqueue(typeof chunk === 'string' ? Buffer.from(chunk) : chunk);
				if (controller.desiredSize !== null && controller.desiredSize <= 0) source.pause();
			};
			const end = () => {
				if (!active) return;
				active = false;
				detach();
				controller.close();
			};
			const error = (reason: unknown) => {
				if (!active) return;
				active = false;
				detach();
				drainSource();
				controller.error(reason instanceof Error ? reason : new Error('API request body failed'));
			};
			const aborted = () => error(new Error('API request body aborted'));
			const close = () => { if (!source.readableEnded) error(new Error('API request body closed')); };
			detach = () => {
				source.off('data', data);
				source.off('end', end);
				source.off('error', error);
				source.off('aborted', aborted);
				source.off('close', close);
			};
			source.on('data', data);
			source.once('end', end);
			source.once('error', error);
			source.once('aborted', aborted);
			source.once('close', close);
			if (source.readableEnded) end();
			else if (source.destroyed) error(new Error('API request body closed'));
		},
		pull() {
			if (active) source.resume();
		},
		cancel() {
			if (!active) return;
			active = false;
			detach();
			drainSource();
		},
	}, { highWaterMark: 64 * 1024, size: chunk => chunk?.byteLength ?? 0 });
	return {
		stream,
		drain() {
			if (!active) return;
			active = false;
			detach();
			drainSource();
			target.close();
		},
	};
}
