/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { afterAll, beforeAll, describe, test } from 'vitest';
import { loadConfig } from '@/config.js';
import { IdService } from '@/core/IdService.js';
import { MiApp } from '@/models/App.js';
import { MiAccessToken } from '@/models/AccessToken.js';
import { MiRegistryItem } from '@/models/RegistryItem.js';
import { api, initTestDb, relativeFetch, signup } from '../utils.js';
import type { DataSource } from 'typeorm';
import type * as misskey from 'misskey-js';

function assertRecord(value: unknown): asserts value is Record<string, unknown> {
	assert.ok(typeof value === 'object' && value !== null && !Array.isArray(value));
}

function assertLegacyQueryError(response: { status: number, body: unknown }, message: string): string {
	assert.equal(response.status, 500);
	const body = response.body;
	assertRecord(body);
	assertRecord(body.error);
	assertRecord(body.error.info);
	assertRecord(body.error.info.e);
	const id = body.error.info.e.id;
	assert.equal(typeof id, 'string');
	assert.match(String(id), /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
	assert.deepEqual(body, { error: {
		message: 'Internal error occurred. Please contact us if the error persists.',
		code: 'INTERNAL_ERROR', id: '5d37dbcb-891e-41ca-a3d6-e690c97775ac', kind: 'server',
		info: { e: { message, code: 'QueryFailedError', id } },
	} });
	return String(id);
}

describe('Registry and application read API compatibility', () => {
	let connection: DataSource;
	let alice: misskey.entities.SignupResponse;
	let bob: misskey.entities.SignupResponse;
	let app: MiApp;
	let otherApp: MiApp;
	let nativeToken: MiAccessToken;
	let appToken: MiAccessToken;
	let secondAppToken: MiAccessToken;
	const ids = new IdService(loadConfig());
	const scope = ['rust', 'registry'];

	beforeAll(async () => {
		alice = await signup({ username: 'registryalice' });
		bob = await signup({ username: 'registrybob' });
		connection = await initTestDb(true);
		const apps = connection.getRepository(MiApp);
		app = apps.create({
			id: ids.gen(), userId: alice.id, secret: 'test-app-secret', name: 'Owner application',
			description: 'Application description', permission: ['read:account', 'write:account'], callbackUrl: null,
		});
		otherApp = apps.create({
			id: ids.gen(), userId: alice.id, secret: 'other-app-secret', name: 'Other application',
			description: '', permission: [], callbackUrl: 'https://example.com/callback',
		});
		await apps.insert([app, otherApp]);
		const tokens = connection.getRepository(MiAccessToken);
		const token = (time: number, appId: string | null, name: string | null) => {
			const secret = randomBytes(16).toString('hex');
			return tokens.create({
				id: ids.gen(time), userId: alice.id, appId, token: secret, hash: secret,
				permission: ['read:account', 'write:account'], name, description: null,
				iconUrl: null, session: null, lastUsedAt: null,
			});
		};
		nativeToken = token(Date.UTC(2001, 0, 1), null, null);
		appToken = token(Date.UTC(2002, 0, 1), app.id, null);
		secondAppToken = token(Date.UTC(2003, 0, 1), app.id, '');
		await tokens.insert([nativeToken, appToken, secondAppToken]);
	}, 120_000);

	afterAll(async () => {
		await connection?.destroy();
	});

	async function registryItem(key: string, value: MiRegistryItem['value'], domain: string | null = null, itemScope = scope) {
		await connection.getRepository(MiRegistryItem).insert({
			id: ids.gen(), userId: alice.id, key, value, domain, scope: itemScope,
			updatedAt: new Date('2020-02-29T12:34:56.789Z'),
		});
	}

	test('registry get distinguishes missing keys, stored null, domains, users and scope order', async () => {
		await registryItem('null-value', null);
		await registryItem('empty-domain', { value: 'empty' }, '');
		await registryItem('ordered', { value: 'forward' });
		await registryItem('ordered', { value: 'reverse' }, null, [...scope].reverse());
		assert.deepEqual((await api('i/registry/get', { key: 'null-value', scope }, alice)).body, null);
		assert.deepEqual((await api('i/registry/get', { key: 'empty-domain', scope, domain: '' }, alice)).body, { value: 'empty' });
		assert.deepEqual((await api('i/registry/get', { key: 'ordered', scope }, alice)).body, { value: 'forward' });
		assert.deepEqual((await api('i/registry/get', { key: 'ordered', scope: [...scope].reverse() }, alice)).body, { value: 'reverse' });
		for (const [endpoint, id] of [
			['i/registry/get', 'ac3ed68a-62f0-422b-a7bc-d5e09e8f6a6a'],
			['i/registry/get-detail', '97a1e8e7-c0f7-47d2-957a-92e61256e01a'],
		] as const) {
			for (const [key, user] of [['missing', alice], ['null-value', bob]] as const) {
				const response = await api(endpoint, { key, scope }, user);
				assert.equal(response.status, 400);
				assert.deepEqual(response.body, { error: { message: 'No such key.', code: 'NO_SUCH_KEY', id, kind: 'client' } });
			}
		}
	});

	test('registry detail hydrates PostgreSQL timestamps at JavaScript millisecond precision', async () => {
		await registryItem('date', { nested: [null, true, 'text'] });
		await connection.query('UPDATE registry_item SET "updatedAt" = $1 WHERE "userId" = $2 AND key = $3', [
			'1969-12-31T23:59:59.999999Z', alice.id, 'date',
		]);
		const response = await api('i/registry/get-detail', { key: 'date', scope }, alice);
		assert.equal(response.status, 200);
		assert.deepEqual(response.body, { updatedAt: '1969-12-31T23:59:59.999Z', value: { nested: [null, true, 'text'] } });
	});

	test('registry collections retain JSON kinds, numeric key enumeration and __proto__ assignment', async () => {
		const collectionScope = ['collection'];
		for (const [key, value] of [
			['z', null], ['10', [1]], ['2', 9007199254740992], ['__proto__', { hidden: true }],
			['boolean', false], ['string', 'value'], ['object', { nested: true }], ['constructor', 'ordinary'],
		] as const) await registryItem(key, value, null, collectionScope);
		const all = await api('i/registry/get-all', { scope: collectionScope }, alice);
		assert.equal(all.status, 200);
		assert.deepEqual(Object.keys(all.body), ['2', '10', 'z', 'boolean', 'string', 'object', 'constructor']);
		assert.deepEqual(all.body, { 2: 9007199254740992, 10: [1], z: null, boolean: false, string: 'value', object: { nested: true }, constructor: 'ordinary' });
		const types = await api('i/registry/keys-with-type', { scope: collectionScope }, alice);
		assert.deepEqual(types.body, { 2: 'number', 10: 'array', z: 'null', boolean: 'boolean', string: 'string', object: 'object', constructor: 'string' });
		const keys = await api('i/registry/keys', { scope: collectionScope }, alice);
		assert.deepEqual(keys.body, ['z', '10', '2', '__proto__', 'boolean', 'string', 'object', 'constructor']);
		const scalar = await relativeFetch('api/i/registry/get', {
			method: 'POST', headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ i: alice.token, key: 'string', scope: collectionScope }),
		});
		assert.equal(scalar.status, 200);
		assert.equal(scalar.headers.get('content-type'), 'text/plain; charset=utf-8');
		assert.equal(await scalar.text(), '"value"');
	});

	test('access tokens select their own registry domain for every registry reader', async () => {
		await registryItem('token-owned', { value: 'native' }, null, ['token']);
		await registryItem('token-owned', { value: 'third-party' }, nativeToken.id, ['token']);
		await registryItem('token-owned', { value: 'application' }, appToken.id, ['token']);
		for (const [token, expected] of [[nativeToken, 'third-party'], [appToken, 'application']] as const) {
			const auth = { token: token.token };
			const params = { key: 'token-owned', scope: ['token'], domain: 'ignored.example' };
			assert.deepEqual((await api('i/registry/get', params, auth)).body, { value: expected });
			assert.deepEqual((await api('i/registry/get-detail', params, auth)).body.value, { value: expected });
			assert.deepEqual((await api('i/registry/get-all', params, auth)).body, { 'token-owned': { value: expected } });
			assert.deepEqual((await api('i/registry/keys', params, auth)).body, ['token-owned']);
			assert.deepEqual((await api('i/registry/keys-with-type', params, auth)).body, { 'token-owned': 'object' });
		}
	});

	test('scope listing keeps domain first occurrence and deduplicates scopes', async () => {
		const response = await api('i/registry/scopes-with-domain', {}, alice);
		assert.equal(response.status, 200);
		assert.deepEqual(response.body, [
			{ domain: null, scopes: [scope, [...scope].reverse(), ['collection'], ['token']] },
			{ domain: '', scopes: [scope] },
			{ domain: nativeToken.id, scopes: [['token']] },
			{ domain: appToken.id, scopes: [['token']] },
		]);
		assert.deepEqual((await api('i/registry/scopes-with-domain', {}, bob)).body, []);
	});

	test('app show includes secrets only for the owner using a native credential', async () => {
		const publicApp = { id: app.id, name: app.name, callbackUrl: null, permission: app.permission };
		const anonymous = await api('app/show', { appId: app.id });
		assert.equal(anonymous.status, 200);
		assert.deepEqual(anonymous.body, publicApp);
		assert.deepEqual((await api('app/show', { appId: app.id }, alice)).body, { ...publicApp, secret: app.secret, isAuthorized: true });
		assert.deepEqual((await api('app/show', { appId: app.id }, bob)).body, { ...publicApp, isAuthorized: false });
		for (const token of [nativeToken, appToken]) {
			assert.deepEqual((await api('app/show', { appId: app.id }, { token: token.token })).body, { ...publicApp, isAuthorized: true });
		}
		const missing = await api('app/show', { appId: 'nonexistent' });
		assert.equal(missing.status, 400);
		assert.deepEqual(missing.body, { error: { message: 'No such app.', code: 'NO_SUCH_APP', id: 'dce83913-2dc6-4093-8a7b-71dbb11718a3', kind: 'client' } });
	});

	test('my apps honors owner, pagination, isAuthorized and secret omission', async () => {
		const response = await api('my/apps', {}, alice);
		assert.equal(response.status, 200);
		assert.deepEqual(response.body.map(value => [value.id, value.isAuthorized, Object.hasOwn(value, 'secret')]), [
			[app.id, true, false], [otherApp.id, false, false],
		]);
		assert.deepEqual((await api('my/apps', { limit: 1, offset: 1 }, alice)).body.map(value => value.id), [otherApp.id]);
		assert.deepEqual((await api('my/apps', {}, bob)).body, []);
		const negative = assertLegacyQueryError(await api('my/apps', { offset: -1 }, alice), 'OFFSET must not be negative');
		const overflow = assertLegacyQueryError(await api('my/apps', { offset: 1e21 }, alice), 'bigint out of range');
		assert.notEqual(negative, overflow);
	});

	test('PostgreSQL failures retain QueryFailedError details and fresh correlation IDs', async () => {
		const ids = [
			assertLegacyQueryError(await api('hashtags/search', { query: 'fixture', offset: -1 }), 'OFFSET must not be negative'),
			assertLegacyQueryError(await api('i/authorized-apps', { offset: -1 }, alice), 'OFFSET must not be negative'),
			assertLegacyQueryError(await api('i/registry/get', { key: '\0', scope }, alice), 'invalid byte sequence for encoding "UTF8": 0x00'),
		];
		assert.equal(new Set(ids).size, ids.length);
	});

	test('authorized apps preserves duplicate application tokens, order and pagination', async () => {
		for (const sort of ['asc', 'desc'] as const) {
			const response = await api('i/authorized-apps', { sort }, alice);
			assert.equal(response.status, 200);
			assert.deepEqual(response.body.map(value => value.id), [app.id, app.id]);
			assert.ok(response.body.every(value => value.isAuthorized && !Object.hasOwn(value, 'secret')));
			assert.deepEqual((await api('i/authorized-apps', { sort, offset: 1, limit: 1 }, alice)).body.map(value => value.id), [app.id]);
		}
		assert.deepEqual((await api('i/authorized-apps', {}, bob)).body, []);
	});

	test('i/apps retains nullish fallbacks, omission, date hydration and all sort modes', async () => {
		await connection.query('UPDATE access_token SET "lastUsedAt" = CASE id WHEN $1 THEN NULL WHEN $2 THEN $4::timestamptz ELSE $5::timestamptz END WHERE "userId" = $3', [
			nativeToken.id, appToken.id, alice.id, '1969-12-31T23:59:59.999999Z', '2020-01-01T00:00:00Z',
		]);
		const ascending = [nativeToken.id, appToken.id, secondAppToken.id];
		for (const [sort, expected] of [
			[undefined, ascending], ['-createdAt', ascending], ['+createdAt', [...ascending].reverse()],
			['+lastUsedAt', [nativeToken.id, secondAppToken.id, appToken.id]],
			['-lastUsedAt', [appToken.id, secondAppToken.id, nativeToken.id]],
		] as const) {
			const response = await api('i/apps', sort ? { sort } : {}, alice);
			assert.equal(response.status, 200);
			assert.deepEqual(response.body.map(value => value.id), expected);
		}
		const rows = (await api('i/apps', {}, alice)).body;
		assert.deepEqual(rows[0], { id: nativeToken.id, createdAt: '2001-01-01T00:00:00.000Z', permission: nativeToken.permission, iconUrl: null, description: null });
		assert.deepEqual(rows[1], { id: appToken.id, name: app.name, createdAt: '2002-01-01T00:00:00.000Z', lastUsedAt: '1969-12-31T23:59:59.999Z', permission: app.permission, iconUrl: null, description: app.description });
		assert.equal(rows[2].name, '');
		assert.deepEqual((await api('i/apps', {}, bob)).body, []);
	});
});
