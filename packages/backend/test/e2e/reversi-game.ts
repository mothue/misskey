/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

process.env.NODE_ENV = 'test';

import * as assert from 'assert';
import { randomBytes } from 'node:crypto';
import { describe, beforeAll, test } from 'vitest';
import { ReversiMatchResponse } from 'misskey-js/entities.js';
import { Redis } from 'ioredis';
import { In } from 'typeorm';
import { api, initTestDb, relativeFetch, role, signup } from '../utils.js';
import { loadConfig } from '@/config.js';
import { IdService } from '@/core/IdService.js';
import { MiAccessToken, MiUser } from '@/models/_.js';
import type * as misskey from 'misskey-js';

describe('ReversiGame', () => {
	let alice: misskey.entities.SignupResponse;
	let bob: misskey.entities.SignupResponse;

	beforeAll(async () => {
		alice = await signup({ username: 'alice' });
		bob = await signup({ username: 'bob' });
	}, 1000 * 60 * 2);

	describe('reversi/invitations', () => {
		async function withInvitations(run: (redis: Redis, keys: { alice: string; bob: string }) => Promise<void>) {
			const redis = new Redis(loadConfig().redis);
			const keys = { alice: `reversi:matchSpecific:${alice.id}`, bob: `reversi:matchSpecific:${bob.id}` };
			const saved: { key: string; value: Buffer | null; expires: number }[] = [];
			try {
				for (const key of Object.values(keys)) {
					const value = await redis.dumpBuffer(key);
					const expires = await redis.pexpiretime(key);
					saved.push({ key, value, expires });
				}
				await redis.del(...Object.values(keys));
				await run(redis, keys);
			} finally {
				try {
					for (const { key, value, expires } of saved) {
						await redis.del(key);
						if (value === null || expires === -2) continue;
						if (expires >= 0 && expires <= Date.now()) continue;
						if (expires < 0) await redis.restore(key, 0, value, 'REPLACE');
						else await redis.restore(key, expires, value, 'REPLACE', 'ABSTTL');
					}
				} finally { await redis.quit(); }
			}
		}

		test('empty and missing invitations are arrays; singleton reads preserve members and TTL', async () => {
			await withInvitations(async (redis, keys) => {
				const empty = await api('reversi/invitations', {}, bob);
				assert.strictEqual(empty.status, 200);
				assert.deepStrictEqual(empty.body, []);
				// Predeclared +infinity keeps the fixture stable beyond the 20-second window.
				await redis.zadd(keys.bob, '+inf', 'missing!not-an-id');
				const missing = await api('reversi/invitations', {}, bob);
				assert.strictEqual(missing.status, 200);
				assert.deepStrictEqual(missing.body, []);
				await redis.zadd(keys.bob, '+inf', alice.id, '-inf', bob.id);
				await redis.pexpire(keys.bob, 60_000);
				const members = await redis.zrange(keys.bob, 0, -1, 'WITHSCORES');
				const expires = await redis.pexpiretime(keys.bob);
				const response = await api('reversi/invitations', {}, bob);
				assert.strictEqual(response.status, 200);
				assert.deepStrictEqual(response.body.map(user => user.id), [alice.id]);
				assert.strictEqual(response.body[0].username, alice.username);
				assert.deepStrictEqual(await redis.zrange(keys.bob, 0, -1, 'WITHSCORES'), members);
				assert.strictEqual(await redis.pexpiretime(keys.bob), expires);
			});
		});

		test('mixed missing, remote and tied inviters keep the actual user query result sequence', async () => {
			await withInvitations(async (redis, keys) => {
				const connection = await initTestDb(true);
				const users = connection.getRepository(MiUser);
				const ids = new IdService(loadConfig());
				const owned = [ids.gen(Date.UTC(2020, 0, 2)), ids.gen(Date.UTC(2020, 0, 1))];
				try {
					await users.insert([
						{ id: owned[0], username: 'invitationfirst', usernameLower: 'invitationfirst', name: 'First 雲😀' },
						{ id: owned[1], username: 'invitationremote', usernameLower: 'invitationremote', host: 'invitations.example.test', isSuspended: true },
					]);
					const members = [owned[0], 'missing!not-an-id', alice.id, owned[1]];
					await redis.zadd(keys.bob, ...members.flatMap(id => ['+inf', id]));
					// Complete the same ID sequence as the source; public order comes from DB rows.
					const invitationIds = await redis.zrange(keys.bob, 0, -1);
					const expected = await users.findBy({ id: In(invitationIds) });
					assert.strictEqual(expected.length, 3);
					const response = await api('reversi/invitations', {}, bob);
					assert.strictEqual(response.status, 200);
					assert.deepStrictEqual(response.body.map(user => user.id), expected.map(user => user.id));
					assert.strictEqual(response.body.find(user => user.id === owned[0])?.name, 'First 雲😀');
					assert.strictEqual(response.body.find(user => user.id === owned[1])?.host, 'invitations.example.test');
					for (const user of response.body) {
						assert.strictEqual(Object.hasOwn(user, 'description'), false);
						assert.strictEqual(Object.hasOwn(user, 'isFollowing'), false);
					}
				} finally {
					try { await users.delete(owned); } finally { await connection.destroy(); }
				}
			});
		});

		test('viewer keys are separate and the actual administrator sees private badge roles', async () => {
			await withInvitations(async (redis, keys) => {
				const badges: misskey.entities.Role[] = [];
				try {
					for (const isPublic of [true, false]) badges.push(await role(alice, {
						name: `Invitation badge ${isPublic}`, target: 'conditional',
						condFormula: { id: 'invitations-local', type: 'isLocal' }, asBadge: true, isPublic,
					}));
					await redis.zadd(keys.bob, '+inf', alice.id);
					await redis.zadd(keys.alice, '+inf', bob.id);
					for (const [viewer, inviter] of [[bob, alice], [alice, bob]] as const) {
						const response = await api('reversi/invitations', {}, viewer);
						assert.strictEqual(response.status, 200);
						assert.deepStrictEqual(response.body.map(user => user.id), [inviter.id]);
						const roles = response.body[0].badgeRoles;
						assert.ok(Array.isArray(roles));
						assert.strictEqual(roles.some(badge => badge.name === badges[0].name), true);
						assert.strictEqual(roles.some(badge => badge.name === badges[1].name), viewer === alice);
					}
				} finally {
					const statuses: number[] = [];
					for (const badge of badges) statuses.push((await api('admin/roles/delete', { roleId: badge.id }, alice)).status);
					assert.ok(statuses.every(status => status === 204));
				}
			});
		});

		test('read:account scope and ignored JSON body shapes retain common HTTP behavior', async () => {
			await withInvitations(async (redis, keys) => {
				await redis.zadd(keys.bob, '+inf', alice.id);
				const connection = await initTestDb(true);
				const tokens = connection.getRepository(MiAccessToken);
				const ids = new IdService(loadConfig());
				const reader = randomBytes(16).toString('hex');
				const writer = randomBytes(16).toString('hex');
				const owned = [ids.gen(), ids.gen()];
				try {
					// Direct scoped token fixtures avoid create-token notifications.
					await tokens.insert([reader, writer].map((token, index) => ({
						id: owned[index], userId: bob.id, appId: null, token, hash: token,
						permission: [index === 0 ? 'read:account' : 'write:notes'], lastUsedAt: null,
						name: null, description: null, iconUrl: null, session: null,
					})));
					assert.strictEqual((await api('reversi/invitations', {})).status, 401);
					assert.strictEqual((await api('reversi/invitations', {}, { token: writer })).status, 403);
					const allowed = await api('reversi/invitations', {}, { token: reader });
					assert.strictEqual(allowed.status, 200);
					assert.deepStrictEqual(allowed.body.map(user => user.id), [alice.id]);
					for (const body of [null, [], 42, 'ignored', true, { extra: { accepted: true } }]) {
						const response = await relativeFetch('api/reversi/invitations', {
							method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${bob.token}` },
							body: JSON.stringify(body),
						});
						assert.strictEqual(response.status, 200);
						const users = await response.json() as misskey.entities.UserLite[];
						assert.deepStrictEqual(users.map(user => user.id), [alice.id]);
					}
					const get = await relativeFetch('api/reversi/invitations', { headers: { authorization: `Bearer ${bob.token}` } });
					assert.strictEqual(get.status, 405);
					assert.strictEqual(await get.text(), '');
				} finally {
					try { await tokens.delete(owned); } finally { await connection.destroy(); }
				}
			});
		});
	});

	test('matches when alice invites bob and bob accepts', async () => {
		const response1 = await api('reversi/match', { userId: bob.id }, alice);
		assert.strictEqual(response1.status, 204);
		assert.strictEqual(response1.body, null);
		const response2 = await api('reversi/match', { userId: alice.id }, bob);
		assert.strictEqual(response2.status, 200);
		assert.notStrictEqual(response2.body, null);
		const body = response2.body as ReversiMatchResponse;
		assert.strictEqual(body.user1.id, alice.id);
		assert.strictEqual(body.user2.id, bob.id);
	});
});
