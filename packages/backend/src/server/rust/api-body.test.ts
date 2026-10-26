/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { PassThrough } from 'node:stream';
import { setImmediate } from 'node:timers/promises';
import { expect, test } from 'vitest';
import { apiRequestBody } from './api-body.js';

const counts = (source: PassThrough) => ['data', 'end', 'error', 'aborted', 'close'].map(event => source.listenerCount(event));

test('body cancellation drains the original readable without destroying it or retaining listeners', async () => {
	const source = new PassThrough({ autoDestroy: false });
	const before = counts(source);
	const { stream: body } = apiRequestBody(source);
	const reader = body.getReader();
	source.write(Buffer.from([0, 1, 255]));
	expect((await reader.read()).value).toEqual(Buffer.from([0, 1, 255]));
	await reader.cancel('early upstream 413');
	expect(source.destroyed).toBe(false);
	const ended = new Promise<void>(resolve => source.once('end', resolve));
	source.end(Buffer.alloc(128 * 1024));
	await ended;
	expect(source.destroyed).toBe(false);
	expect(counts(source)).toEqual(before);
	source.destroy();
});

test('body forwarding applies byte backpressure and preserves complete binary bytes', async () => {
	const source = new PassThrough({ autoDestroy: false });
	const { stream: body } = apiRequestBody(source);
	const input = Buffer.alloc(128 * 1024, 0xff);
	source.write(input.subarray(0, 64 * 1024));
	await setImmediate();
	expect(source.isPaused()).toBe(true);
	source.end(input.subarray(64 * 1024));
	expect(Buffer.from(await new Response(body).arrayBuffer())).toEqual(input);
	expect(counts(source)).toEqual([0, 0, 0, 0, 0]);
	source.destroy();
});

test('aborted-then-error streams reject forwarding and clean drain listeners on close', async () => {
	const source = new PassThrough({ autoDestroy: false });
	const reader = apiRequestBody(source).stream.getReader();
	const reading = reader.read();
	source.emit('aborted');
	await expect(reading).rejects.toThrow('aborted');
	const closed = new Promise<void>(resolve => source.once('close', resolve));
	source.destroy(new Error('socket closed after aborted'));
	await closed;
	expect(counts(source)).toEqual([0, 0, 0, 0, 0]);
});

test('an explicit early-response drain also releases a locked stream that fetch never consumed', async () => {
	const source = new PassThrough({ autoDestroy: false });
	const body = apiRequestBody(source);
	const reader = body.stream.getReader();
	source.write(Buffer.from('first'));
	await reader.read();
	body.drain();
	expect(await reader.read()).toEqual({ done: true, value: undefined });
	expect(source.destroyed).toBe(false);
	const ended = new Promise<void>(resolve => source.once('end', resolve));
	source.end(Buffer.alloc(128 * 1024));
	await ended;
	expect(counts(source)).toEqual([0, 0, 0, 0, 0]);
	source.destroy();
});
