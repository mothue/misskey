/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { beforeAll, describe, test } from 'vitest';
import { relativeFetch, signup } from '../utils.js';
import type { UserToken } from '../utils.js';

const privateCache = 'private, max-age=0, must-revalidate';
const jsonType = 'application/json; charset=utf-8';
const invalidEffectiveTokens = [
	{ name: 'zero', value: 0 },
	{ name: 'false', value: false },
	{ name: 'true', value: true },
	{ name: 'array', value: [] },
	{ name: 'object', value: {} },
];

function postTest(body: unknown, token?: string) {
	return relativeFetch('api/test', {
		method: 'POST', headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) },
		body: JSON.stringify(body), signal: AbortSignal.timeout(5000),
	});
}

describe('Rust API execution kernel', () => {
	let account: UserToken;

	beforeAll(async () => {
		const user = await signup({ username: 'rustapi' });
		assert.equal(typeof user.id, 'string', 'fixture signup must succeed');
		assert.equal(typeof user.token, 'string', 'fixture must have a native token');
		account = user;
	});

	test.each(invalidEffectiveTokens)('non-string effective token ($name) completes with an empty 400', async ({ value }) => {
		const response = await postTest({ required: true, i: value });
		assert.equal(response.status, 400);
		assert.equal(await response.text(), '');
		assert.equal(response.headers.get('www-authenticate'), null);
	});

	test.each([
		{ name: 'omitted', body: { required: true } },
		{ name: 'null', body: { required: true, i: null } },
	])('empty token boundary ($name) remains anonymous', async ({ body }) => {
		const response = await postTest(body);
		assert.equal(response.status, 200);
		assert.deepEqual(await response.json(), { ...body, default: 'hello', nullableDefault: 'hello' });
	});

	test('an empty string token is supplied authentication and fails with 401', async () => {
		const response = await postTest({ required: true, i: '' });
		assert.equal(response.status, 401);
		assert.match(response.headers.get('www-authenticate') ?? '', /error="invalid_token"/);
		assert.deepEqual(await response.json(), { error: {
			message: 'Authentication failed. Please ensure your token is correct.', code: 'AUTHENTICATION_FAILED', id: 'b0a7f5f8-dc2f-4171-b91f-de88ad238e14', kind: 'client',
		} });
	});

	test.each(invalidEffectiveTokens)('valid Authorization overrides a non-string body token ($name)', async ({ value }) => {
		const response = await postTest({ required: true, i: value }, account.token);
		assert.equal(response.status, 200);
		assert.deepEqual(await response.json(), { required: true, i: value, default: 'hello', nullableDefault: 'hello' });
	});

	test('unknown GET retains API error headers while unknown POST retains the framework 404', async () => {
		const path = 'api/__rust_api_unknown__';
		const get = await relativeFetch(path);
		assert.equal(get.status, 404);
		assert.equal(get.headers.get('access-control-allow-origin'), '*');
		assert.equal(get.headers.get('cache-control'), privateCache);
		assert.equal(get.headers.get('content-type'), jsonType);
		assert.equal(get.headers.get('www-authenticate'), null);
		assert.deepEqual(await get.json(), {
			error: { message: 'Unknown API endpoint.', code: 'UNKNOWN_API_ENDPOINT', id: '2ca3b769-540a-4f08-9dd5-b5a825b6d0f1', kind: 'client' },
		});
		const post = await relativeFetch(path, { method: 'POST' });
		assert.equal(post.status, 404);
		assert.equal(post.headers.get('access-control-allow-origin'), null);
		assert.equal(post.headers.get('cache-control'), null);
		assert.equal(post.headers.get('content-type'), jsonType);
		assert.deepEqual(await post.json(), { message: 'Route POST:/api/__rust_api_unknown__ not found', error: 'Not Found', statusCode: 404 });
	});

	test('valid preflight returns CORS headers before endpoint authentication and body parsing', async () => {
		const response = await relativeFetch('api/i', {
			method: 'OPTIONS',
			headers: { origin: 'https://client.test', 'access-control-request-method': 'POST', 'access-control-request-headers': 'authorization,content-type' },
		});
		assert.equal(response.status, 204);
		assert.equal(await response.text(), '');
		assert.equal(response.headers.get('access-control-allow-origin'), '*');
		assert.equal(response.headers.get('access-control-allow-methods'), 'GET,HEAD,POST');
		assert.equal(response.headers.get('access-control-allow-headers'), 'authorization,content-type');
		assert.match(response.headers.get('vary') ?? '', /(?:^|,\s*)Access-Control-Request-Headers(?:,|$)/i);
		assert.equal(response.headers.get('cache-control'), null);
		assert.equal(response.headers.get('www-authenticate'), null);
	});

	test('incomplete preflight retains the plain 400 response', async () => {
		const response = await relativeFetch('api/i', { method: 'OPTIONS', headers: { origin: 'https://client.test' } });
		assert.equal(response.status, 400);
		assert.equal(await response.text(), 'Invalid Preflight Request');
		assert.equal(response.headers.get('access-control-allow-origin'), '*');
		assert.equal(response.headers.get('content-type'), 'text/plain');
		assert.equal(response.headers.get('cache-control'), null);
	});

	test.each([
		{ name: 'incomplete JSON', body: '{"required":', code: 'FST_ERR_CTP_INVALID_JSON_BODY', message: "Body is not valid JSON but content-type is set to 'application/json'" },
		{ name: 'trailing JSON', body: '{"required":true}{}', code: 'FST_ERR_CTP_INVALID_JSON_BODY', message: "Body is not valid JSON but content-type is set to 'application/json'" },
		{ name: 'empty JSON', body: '', code: 'FST_ERR_CTP_EMPTY_JSON_BODY', message: "Body cannot be empty when content-type is set to 'application/json'" },
		{ name: 'prototype key', body: '{"required":true,"__proto__":{"polluted":true}}', code: 'FST_ERR_CTP_INVALID_JSON_BODY', message: "Body is not valid JSON but content-type is set to 'application/json'" },
		{ name: 'nested prototype key', body: '{"required":true,"extra":{"__proto__":{"polluted":true}}}', code: 'FST_ERR_CTP_INVALID_JSON_BODY', message: "Body is not valid JSON but content-type is set to 'application/json'" },
		{ name: 'constructor prototype', body: '{"required":true,"extra":{"constructor":{"prototype":{"polluted":true}}}}', code: 'FST_ERR_CTP_INVALID_JSON_BODY', message: "Body is not valid JSON but content-type is set to 'application/json'" },
	])('rejects $name at the JSON boundary', async ({ body, code, message }) => {
		const response = await relativeFetch('api/test', { method: 'POST', headers: { 'content-type': 'application/json' }, body });
		assert.equal(response.status, 400);
		assert.equal(response.headers.get('access-control-allow-origin'), '*');
		assert.equal(response.headers.get('cache-control'), privateCache);
		assert.equal(response.headers.get('www-authenticate'), null);
		assert.deepEqual(await response.json(), { statusCode: 400, code, error: 'Bad Request', message });
	});

	test('JSON larger than the public body limit returns 413 through the public connection', async () => {
		const response = await relativeFetch('api/test', {
			method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ required: true, extra: 'x'.repeat(1024 * 1024) }),
		});
		assert.equal(response.status, 413);
		assert.equal(response.headers.get('access-control-allow-origin'), '*');
		assert.equal(response.headers.get('cache-control'), privateCache);
		assert.deepEqual(await response.json(), { statusCode: 413, code: 'FST_ERR_CTP_BODY_TOO_LARGE', error: 'Payload Too Large', message: 'Request body is too large' });
	});

	test('GET casts numbers and distinguishes cast failures from parsed values with the wrong type', async () => {
		const valid = await relativeFetch('api/notes/featured?limit=1');
		assert.equal(valid.status, 200);
		const notes = await valid.json();
		assert.ok(Array.isArray(notes));
		assert.ok(notes.length <= 1);
		const cast = await relativeFetch('api/notes/featured?limit=not-a-number');
		assert.equal(cast.status, 400);
		assert.deepEqual(await cast.json(), { error: {
			message: 'Invalid param.', code: 'INVALID_PARAM', id: '0b5f1631-7c1a-41a6-b399-cce335f34d85', kind: 'client', info: { param: 'limit', reason: 'cannot cast to integer' },
		} });
		const typed = await relativeFetch('api/notes/featured?limit=null');
		assert.equal(typed.status, 400);
		assert.deepEqual(await typed.json(), { error: {
			message: 'Invalid param.', code: 'INVALID_PARAM', id: '3d81ceae-475f-4600-b2a8-2bc116157532', kind: 'client', info: { param: '#/properties/limit/type', reason: 'must be integer' },
		} });
	});

	test('validation supplies defaults while retaining null, extra values and UTF-16 strings', async () => {
		const extra = { surrogate: '\ud800', names: ['__proto__', 'constructor'], nested: { value: null } };
		const response = await relativeFetch('api/test', {
			method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ required: false, nullableDefault: null, extra }),
		});
		assert.equal(response.status, 200);
		assert.deepEqual(await response.json(), { required: false, nullableDefault: null, extra, default: 'hello' });
	});

	test('anonymous GET uses public caching while a valid native token keeps the response private', async () => {
		const anonymous = await relativeFetch('api/emojis');
		assert.equal(anonymous.status, 200);
		assert.equal(anonymous.headers.get('cache-control'), 'public, max-age=3600');
		const body = await anonymous.json();
		const authenticated = await relativeFetch('api/emojis', { headers: { authorization: `Bearer ${account.token}` } });
		assert.equal(authenticated.status, 200);
		assert.equal(authenticated.headers.get('cache-control'), privateCache);
		assert.deepEqual(await authenticated.json(), body);
	});

	test('MiAuth decodes an encoded slash as part of one session parameter', async () => {
		const response = await relativeFetch('api/miauth/unknown%2Fsession/check', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
		assert.equal(response.status, 200);
		assert.deepEqual(await response.json(), { ok: false });
	});
});
