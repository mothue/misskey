/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { describe, test } from 'vitest';
import { loadConfig } from '@/config.js';
import { MiUser } from '@/models/User.js';
import { api, initTestDb, signup } from '../utils.js';

describe('Rust online users count', () => {
	test('anonymous GET/POST retain count semantics, extras and public GET caching', async () => {
		// Initial signup also initializes the administrator; follow the existing
		// E2E convention and complete it before creating the other fixture users.
		const users = [];
		for (const username of ['online', 'old', 'unset', 'future']) {
			const user = await signup({ username });
			assert.equal(typeof user.id, 'string', 'fixture signup must succeed');
			users.push(user);
		}
		const connection = await initTestDb(true);
		try {
			const repository = connection.getRepository(MiUser);
			// This suite owns its dedicated, reset test DB. Remove activity from
			// system/sign-up users so null and an empty online set are deterministic.
			await repository.createQueryBuilder().update().set({ lastActiveDate: null }).execute();
			const empty = await api('get-online-users-count', {});
			assert.equal(empty.status, 200);
			assert.deepEqual(empty.body, { count: 0 });
			const now = Date.now();
			await repository.update(users[0].id, { lastActiveDate: new Date(now), hideOnlineStatus: true });
			await repository.update(users[1].id, { lastActiveDate: new Date(now - 60 * 60 * 1000) });
			await repository.update(users[3].id, { lastActiveDate: new Date(now + 60 * 60 * 1000) });
			const post = await api('get-online-users-count', {});
			assert.equal(post.status, 200);
			assert.deepEqual(post.body, { count: 2 });
			const get = await fetch(`http://127.0.0.1:${loadConfig().port}/api/get-online-users-count`);
			assert.equal(get.status, 200);
			assert.deepEqual(await get.json(), { count: 2 });
			assert.equal(get.headers.get('cache-control'), 'public, max-age=60');
			const extra = await fetch(`http://127.0.0.1:${loadConfig().port}/api/get-online-users-count`, {
				method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ extra: null }),
			});
			assert.equal(extra.status, 200);
			assert.deepEqual(await extra.json(), { count: 2 });
		} finally { await connection.destroy(); }
	});

	test('Rust kernel rejects malformed JSON and non-object parameters', async () => {
		for (const body of ['{', '[]', 'null', '42']) {
			const response = await fetch(`http://127.0.0.1:${loadConfig().port}/api/get-online-users-count`, {
				method: 'POST', headers: { 'content-type': 'application/json' }, body,
			});
			assert.equal(response.status, 400);
		}
	});
});
