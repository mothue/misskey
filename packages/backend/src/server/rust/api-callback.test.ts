/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test, vi } from 'vitest';
import { ApiCallbackServer, decodeApiInvocation, encodeApiResult } from './api-callback.js';
import { apiEncodeValue } from './api-wire.js';
import type { FastifyRequest } from 'fastify';

vi.mock('@/server/api/AuthenticateService.js', () => ({ AuthenticationError: class AuthenticationError extends Error {} }));
const id = 'a'.repeat(64);
const invocation = (requestId = id) => ({ v: 1, requestId, kind: 'endpoint', endpoint: 'notes/show', params: apiEncodeValue({ noteId: 'id' }), user: null, token: null });

test('callback framing validates fields, typed values and nullable file names', () => {
	expect(decodeApiInvocation(invocation())).toMatchObject({ params: { noteId: 'id' }, user: null, token: null });
	expect(decodeApiInvocation({ ...invocation(), file: { name: null, path: '/tmp/upload-a' }, routeParams: { session: 's' } })).toMatchObject({ file: { name: null, path: '/tmp/upload-a' }, routeParams: { session: 's' } });
	expect(decodeApiInvocation({ ...invocation(), file: { name: 'file.txt', path: '/tmp/upload-a' } }).file).toEqual({ name: 'file.txt', path: '/tmp/upload-a' });
	const unnamed = decodeApiInvocation({ ...invocation(), file: { path: '/tmp/upload-a' } }).file;
	expect(unnamed).toEqual({ name: undefined, path: '/tmp/upload-a' });
	expect(unnamed!.name).toBeUndefined();
	expect(Object.hasOwn(unnamed!, 'name')).toBe(true);
	expect(() => decodeApiInvocation({ ...invocation(), file: { name: undefined, path: '/tmp/upload-a' } })).toThrow();
	for (const input of [{}, { ...invocation(), requestId: 'public-id' }, { ...invocation(), v: 2 }, { ...invocation(), kind: 'auth' }, { ...invocation(), endpoint: null }, { ...invocation(), params: {} }, { ...invocation(), file: { name: 'x', path: '' } }, { ...invocation(), file: { name: 1, path: '/tmp/x' } }, { ...invocation(), routeParams: { session: 1 } }, { ...invocation(), extra: true }]) expect(() => decodeApiInvocation(input)).toThrow();
});

test('business result framing preserves Date/toJSON, omissions, binary and top-level distinctions', () => {
	expect(encodeApiResult({ date: new Date(0), omitted: undefined, array: [undefined, NaN], surrogate: '\ud800' })).toEqual({ v: 1, kind: 'json', resultJson: '{"date":"1970-01-01T00:00:00.000Z","array":[null,null],"surrogate":"\\ud800"}' });
	expect(encodeApiResult({ toJSON: () => ({ x: 1 }) })).toEqual({ v: 1, kind: 'json', resultJson: '{"x":1}' });
	expect(encodeApiResult({ toJSON: () => undefined })).toEqual({ v: 1, kind: 'json', resultJson: undefined });
	expect(encodeApiResult(Buffer.from([0, 0xff]))).toEqual({ v: 1, kind: 'bytes', base64: 'AP8=' });
	expect(encodeApiResult(null)).toEqual({ v: 1, kind: 'value', value: { t: 'null' } });
	expect(encodeApiResult(undefined)).toEqual({ v: 1, kind: 'value', value: { t: 'undefined' } });
	expect(encodeApiResult(NaN)).toEqual({ v: 1, kind: 'value', value: { t: 'number', v: 'NaN' } });
});

test('private callbacks require both transport authorization and a live one-shot request capability', async () => {
	const execute = vi.fn(async () => encodeApiResult({ ok: true }));
	const server = new ApiCallbackServer(execute);
	try {
		const endpoint = await server.start();
		const pending = server.register({ url: '/api/notes/show' } as FastifyRequest);
		const send = (body: unknown, token = endpoint.token) => fetch(endpoint.url, { method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: JSON.stringify(body) });
		expect((await send(invocation(pending.requestId), 'wrong')).status).toBe(401);
		expect((await send(invocation())).status).toBe(409);
		const accepted = await send(invocation(pending.requestId));
		expect(accepted.status).toBe(200);
		expect(await accepted.json()).toEqual({ v: 1, kind: 'json', resultJson: '{"ok":true}' });
		expect((await send(invocation(pending.requestId))).status).toBe(409);
		expect(execute).toHaveBeenCalledTimes(1);
		const released = server.register({} as FastifyRequest);
		released.release();
		expect((await send(invocation(released.requestId))).status).toBe(409);
		pending.release();
	} finally { await server.stop(); }
});
