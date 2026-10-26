/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'vitest';
import type { DataSource } from 'typeorm';
import type * as Misskey from 'misskey-js';
import { loadConfig } from '@/config.js';
import { MiEmoji } from '@/models/Emoji.js';
import { MiHashtag } from '@/models/Hashtag.js';
import { MiRetentionAggregation } from '@/models/RetentionAggregation.js';
import { MiUsedUsername } from '@/models/UsedUsername.js';
import { api, initTestDb, signup } from '../utils.js';

describe('Discovery read API compatibility', () => {
	let connection: DataSource;
	const original = 'https://example.test/original.png';
	const detailed = (id: string, name: string) => ({
		id,
		aliases: ['shared'],
		name,
		category: 'shared',
		host: null,
		url: original,
		license: null,
		isSensitive: false,
		localOnly: false,
		roleIdsThatCanBeUsedThisEmojiAsReaction: [],
	});

	beforeAll(async () => {
		await signup({ username: 'discoveryroot' });
		connection = await initTestDb(true);
		await connection.getRepository(MiEmoji).insert([
			{ id: 'zzdisc001', name: 'alpha', host: null, category: null, originalUrl: original, aliases: ['shared'], publicUrl: '' },
			{ id: 'zzdisc002', name: 'beta', host: null, category: 'shared', originalUrl: original, aliases: ['shared'], publicUrl: 'https://example.test/public.webp', license: 'CC0', localOnly: true, isSensitive: true, roleIdsThatCanBeUsedThisEmojiAsReaction: ['role1'] },
			{ id: 'zzdisc003', name: 'gamma', host: null, category: 'shared', originalUrl: original, aliases: ['shared', 'astral😀'], publicUrl: '' },
			{ id: 'zzdisc004', name: 'delta', host: null, category: 'shared', originalUrl: original, aliases: ['shared'], publicUrl: '' },
			{ id: 'zzdisc005', name: 'remote', host: 'example.test', originalUrl: original, publicUrl: '' },
		]);
		await connection.getRepository(MiHashtag).insert([
			{ id: 'zzhash001', name: 'hello', mentionedUsersCount: 20, mentionedLocalUsersCount: 10, mentionedRemoteUsersCount: 10, attachedUsersCount: 8, attachedLocalUsersCount: 4, attachedRemoteUsersCount: 4 },
			{ id: 'zzhash002', name: 'hello_world', mentionedUsersCount: 30, mentionedLocalUsersCount: 20, mentionedRemoteUsersCount: 10 },
			{ id: 'zzhash003', name: 'hello%tag', mentionedUsersCount: 40, mentionedLocalUsersCount: 30, mentionedRemoteUsersCount: 10, attachedUsersCount: 9, attachedLocalUsersCount: 9 },
			{ id: 'zzhash004', name: 'ος', mentionedUsersCount: 2147483647, mentionedLocalUsersCount: 2147483646, mentionedRemoteUsersCount: 1, attachedUsersCount: 4, attachedRemoteUsersCount: 4 },
			{ id: 'zzhash005', name: 'i̇', mentionedUsersCount: 1, mentionedLocalUsersCount: 1, attachedUsersCount: 1, attachedLocalUsersCount: 1 },
		]);
		await connection.getRepository(MiUsedUsername).insert({ username: 'used_discovery', createdAt: new Date() });
		await connection.getRepository(MiRetentionAggregation).insert(Array.from({ length: 32 }, (_, index) => ({
			id: `zzret${String(index).padStart(3, '0')}`,
			createdAt: new Date(Date.UTC(2024, 0, index + 1, 3, 4, 5, 123)),
			updatedAt: new Date(),
			dateKey: `discovery-${index}`,
			userIds: [],
			usersCount: index,
			data: { 10: 9007199254740992, 2: 0.25, label: 2 },
		})));
		await connection.query('UPDATE retention_aggregation SET "createdAt" = $1 WHERE id = $2', ['1969-12-31T23:59:59.999987Z', 'zzret031']);
	});

	afterAll(async () => {
		await connection?.destroy();
	});

	test('emoji detailed and simple SDK contracts retain null, omitted flags and fallback URLs', async () => {
		const response = await api('emoji', { name: 'alpha' });
		assert.equal(response.status, 200);
		const emoji: Misskey.Endpoints['emoji']['res'] = response.body;
		assert.deepEqual(emoji, { ...detailed('zzdisc001', 'alpha'), category: null });

		const list = await api('emojis', {});
		assert.equal(list.status, 200);
		const simple: Misskey.Endpoints['emojis']['res'] = list.body;
		assert.deepEqual(simple.emojis.map(item => item.name), ['beta', 'delta', 'gamma', 'alpha']);
		assert.deepEqual(simple.emojis.at(-1), { aliases: ['shared'], name: 'alpha', category: null, url: original });
		assert.deepEqual(simple.emojis[0], {
			aliases: ['shared'], name: 'beta', category: 'shared', url: 'https://example.test/public.webp', localOnly: true, isSensitive: true, roleIdsThatCanBeUsedThisEmojiAsReaction: ['role1'],
		});
	});

	test('emoji missing is still an internal error and GET keeps public cache metadata', async () => {
		for (const name of ['missing', 'Alpha', '\ud800', 'line\n"quote\\end']) {
			const missing = await fetch(`http://127.0.0.1:${loadConfig().port}/api/emoji`, {
				method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name }),
			});
			assert.equal(missing.status, 500);
			const body = await missing.json();
			assert.ok(body !== null && typeof body === 'object' && 'error' in body);
			const error = body.error;
			assert.ok(error !== null && typeof error === 'object' && 'info' in error);
			const info = error.info;
			assert.ok(info !== null && typeof info === 'object' && 'e' in info);
			const exception = info.e;
			assert.ok(exception !== null && typeof exception === 'object' && 'id' in exception);
			assert.equal(typeof exception.id, 'string');
			assert.match(String(exception.id), /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
			assert.deepEqual(body, { error: {
				message: 'Internal error occurred. Please contact us if the error persists.',
				code: 'INTERNAL_ERROR',
				id: '5d37dbcb-891e-41ca-a3d6-e690c97775ac',
				kind: 'server',
				info: { e: {
					message: `Could not find any entity of type "MiEmoji" matching: ${JSON.stringify({ where: { name, host: { _type: 'isNull', _useParameter: false, _multipleParameters: false } } }, null, 4)}`,
					code: 'EntityNotFoundError',
					id: exception.id,
				} },
			} });
		}
		for (const endpoint of ['emoji?name=alpha', 'emojis']) {
			const response = await fetch(`http://127.0.0.1:${loadConfig().port}/api/${endpoint}`);
			assert.equal(response.status, 200);
			assert.equal(response.headers.get('cache-control'), 'public, max-age=3600');
		}
	});

	test('hashtag list retains every count field, sort direction and combined attached filters', async () => {
		const response = await api('hashtags/list', { sort: '+mentionedUsers' });
		assert.equal(response.status, 200);
		const tags: Misskey.Endpoints['hashtags/list']['res'] = response.body;
		assert.deepEqual(tags[0], { tag: 'ος', mentionedUsersCount: 2147483647, mentionedLocalUsersCount: 2147483646, mentionedRemoteUsersCount: 1, attachedUsersCount: 4, attachedLocalUsersCount: 0, attachedRemoteUsersCount: 4 });
		const reverse = await api('hashtags/list', { sort: '-mentionedUsers' });
		assert.deepEqual(reverse.body.map(item => item.tag), tags.map(item => item.tag).reverse());
		const attached = await api('hashtags/list', { sort: '+mentionedUsers', attachedToUserOnly: true, attachedToLocalUserOnly: true, attachedToRemoteUserOnly: true });
		assert.deepEqual(attached.body.map(item => item.tag), ['hello']);
	});

	test('hashtag prefix search lowercases, escapes SQL wildcards and applies offset', async () => {
		const response = await api('hashtags/search', { query: 'HELLO' });
		assert.equal(response.status, 200);
		assert.deepEqual(response.body, ['hello%tag', 'hello_world', 'hello']);
		for (const [query, expected] of [['hello_', 'hello_world'], ['hello%', 'hello%tag']]) {
			const literal = await api('hashtags/search', { query });
			assert.deepEqual(literal.body, [expected]);
		}
		const offset = await api('hashtags/search', { query: 'hello', offset: 1, limit: 1 });
		assert.deepEqual(offset.body, ['hello_world']);
		const fullwidth = await api('hashtags/search', { query: 'ＨＥＬＬＯ' });
		assert.deepEqual(fullwidth.body, []);
		const negative = await api('hashtags/search', { query: 'hello', offset: -1 });
		assert.equal(negative.status, 500);
	});

	test('hashtag show normalizes compatibility characters and contextual lowercase', async () => {
		for (const [tag, expected] of [['ＨＥＬＬＯ', 'hello'], ['ΟΣ', 'ος'], ['İ', 'i̇']]) {
			const response = await api('hashtags/show', { tag });
			assert.equal(response.status, 200);
			assert.equal(response.body.tag, expected);
		}
		const missing = await fetch(`http://127.0.0.1:${loadConfig().port}/api/hashtags/show`, {
			method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ tag: 'missing' }),
		});
		assert.equal(missing.status, 400);
		const body = await missing.json();
		assert.ok(body !== null && typeof body === 'object' && 'error' in body);
		assert.deepEqual(body.error, { message: 'No such hashtag.', code: 'NO_SUCH_HASHTAG', id: '110ee688-193e-4a3a-9ecf-c167b2e6981e', kind: 'client' });
	});

	test('username availability checks existing, historical and reserved names without case', async () => {
		for (const [username, available] of [['DISCOVERYROOT', false], ['USED_DISCOVERY', false], ['AdMiN', false], ['fresh_discovery', true]] as const) {
			const response = await api('username/available', { username });
			assert.equal(response.status, 200);
			const result: Misskey.Endpoints['username/available']['res'] = response.body;
			assert.deepEqual(result, { available });
		}
	});

	test('retention returns newest thirty with pg Date millisecond precision and numeric JSON', async () => {
		const response = await api('retention', {});
		assert.equal(response.status, 200);
		const records: Misskey.Endpoints['retention']['res'] = response.body;
		assert.equal(records.length, 30);
		assert.deepEqual(records[0], { createdAt: '1969-12-31T23:59:59.999Z', users: 31, data: { 2: 0.25, 10: 9007199254740992, label: 2 } });
		assert.equal(records.at(-1)?.users, 2);
		const get = await fetch(`http://127.0.0.1:${loadConfig().port}/api/retention`);
		assert.equal(get.headers.get('cache-control'), 'public, max-age=3600');
		assert.deepEqual(await get.json(), records);
	});
});
