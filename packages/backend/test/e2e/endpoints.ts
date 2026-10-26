/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

process.env.NODE_ENV = 'test';

import * as assert from 'assert';
import { randomUUID } from 'node:crypto';
import { setTimeout } from 'node:timers/promises';
import { describe, beforeAll, afterAll, test, expect, vi } from 'vitest';
// node-fetch only supports it's own Blob yet
// https://github.com/node-fetch/node-fetch/pull/1664
import { Blob } from 'node-fetch';
import { Redis } from 'ioredis';
import { api, castAsError, initTestDb, post, relativeFetch, role, signup, simpleGet, uploadFile } from '../utils.js';
import type * as misskey from 'misskey-js';
import type { DataSource } from 'typeorm';
import { loadConfig } from '@/config.js';
import { MiAnnouncement, MiAnnouncementRead, MiBubbleGameRecord, MiUser } from '@/models/_.js';
import { IdService } from '@/core/IdService.js';

const waitForPushToTlOptions = { timeout: 3000, interval: 25 };

describe('hashtags/trend', () => {
	const epoch = Date.parse('2023-01-01T00:00:00Z');
	const hour = 60 * 60 * 1000;

	async function withRanking(run: (redis: Redis, ranking: [string, string], owned: Set<string>) => Promise<void>) {
		// Avoid changing ranking windows halfway through a small fixture request.
		const remaining = hour - ((Date.now() - epoch) % hour);
		if (remaining < 2000) await setTimeout(remaining + 100);
		const window = Math.floor((Date.now() - epoch) / hour);
		const ranking: [string, string] = [`featuredHashtagsRanking:${window}`, `featuredHashtagsRanking:${window - 1}`];
		const redis = new Redis(loadConfig().redis);
		const owned = new Set<string>();
		const saved: { key: string; value: Buffer | null; expires: number }[] = [];
		try {
			// Other files share this dedicated test Redis. Restore their ranking
			// data and expiry instead of flushing the DB or deleting unrelated keys.
			for (const key of ranking) {
				const value = await redis.dumpBuffer(key);
				const ttl = await redis.pttl(key);
				saved.push({ key, value, expires: ttl >= 0 ? Date.now() + ttl : ttl });
			}
			await redis.del(...ranking);
			await run(redis, ranking, owned);
		} finally {
			try {
				if (owned.size > 0) await redis.del(...owned);
				for (const { key, value, expires } of saved) {
					await redis.del(key);
					if (value === null || expires === -2) continue;
					const ttl = expires < 0 ? 0 : expires - Date.now();
					if (expires >= 0 && ttl <= 0) continue;
					await redis.restore(key, ttl, value, 'REPLACE');
				}
			} finally {
				await redis.quit();
			}
		}
	}

	test('空のランキングを匿名GET/POSTで取得でき、GETのキャッシュ期限を保持する', async () => {
		await withRanking(async () => {
			const get = await relativeFetch('api/hashtags/trend?extra=accepted');
			assert.strictEqual(get.status, 200);
			assert.deepStrictEqual(await get.json(), []);
			assert.strictEqual(get.headers.get('cache-control'), 'public, max-age=60');
			const response = await api('hashtags/trend', { extra: { accepted: true } });
			assert.strictEqual(response.status, 200);
			assert.deepStrictEqual(response.body, []);
			assert.strictEqual(response.headers.get('cache-control'), 'private, max-age=0, must-revalidate');
		});
	});

	test('両期間の上位11件を現在期間からの順序で列挙し、重複だけを除く', async () => {
		await withRanking(async (redis, ranking) => {
			const prefix = `trend-${randomUUID()}`;
			const current = Array.from({ length: 12 }, (_, i) => `${prefix}-current-${i}`);
			const previous = Array.from({ length: 12 }, (_, i) => `${prefix}-previous-${i}`);
			for (const [key, tags] of [[ranking[0], current], [ranking[1], previous]] as const) {
				await redis.zadd(key, ...tags.flatMap((tag, index) => [100 - index, tag]));
			}
			const response = await api('hashtags/trend', {});
			assert.strictEqual(response.status, 200);
			const included = [...current.slice(0, 11), ...previous.slice(0, 11)];
			assert.deepStrictEqual(response.body, included.map(tag => ({ tag, chart: Array(20).fill(0), usersCount: 0 })));

			await redis.zadd(ranking[1], 101, current[0]);
			const overlapping = await api('hashtags/trend', {});
			assert.strictEqual(overlapping.status, 200);
			assert.deepStrictEqual(overlapping.body.map(row => row.tag), [...current.slice(0, 11), ...previous.slice(0, 10)]);
		});
	});

	test('実際のHyperLogLogから20点のチャートと最大利用者数を返す', async () => {
		await withRanking(async (redis, ranking, owned) => {
			const tags = [`trend-${randomUUID()}`, `trend-${randomUUID()}`];
			await redis.zadd(ranking[0], 2, tags[0], 1, tags[1]);
			const rounded = new Date();
			rounded.setMinutes(Math.floor(rounded.getMinutes() / 10) * 10, 0, 0);
			const pipeline = redis.pipeline();
			// Fill the whole read horizon. Recent windows have a larger count,
			// so the last chart point cannot stand in for Math.max(...chart).
			// Native inline fixtures separately cover local setters and DST.
			for (let offset = -1; offset <= 32 * 6; offset++) {
				const date = new Date(rounded.getTime() - offset * 10 * 60 * 1000);
				const window = date.toISOString().slice(0, 16).replace(/\D/g, '');
				for (const [index, tag] of tags.entries()) {
					const key = `hashtagUsers:${tag}:${window}`;
					owned.add(key);
					pipeline.pfadd(key, ...Array.from({ length: index + 1 }, (_, i) => `user-${i}`));
					if (offset <= 1) pipeline.pfadd(key, 'recent-user');
				}
			}
			const seeded = await pipeline.exec();
			assert.ok(seeded !== null && seeded.every(([error]) => error === null));
			const response = await api('hashtags/trend', {});
			assert.strictEqual(response.status, 200);
			assert.deepStrictEqual(response.body, tags.map((tag, index) => ({
				tag,
				chart: [...Array(3).fill(index + 2), ...Array(17).fill(index + 1)],
				usersCount: index + 2,
			})));
		});
	});
});

describe('Endpoints', () => {
	let alice: misskey.entities.SignupResponse;
	let bob: misskey.entities.SignupResponse;
	let carol: misskey.entities.SignupResponse;
	let dave: misskey.entities.SignupResponse;

	beforeAll(async () => {
		alice = await signup({ username: 'alice' });
		bob = await signup({ username: 'bob' });
		carol = await signup({ username: 'carol' });
		dave = await signup({ username: 'dave' });
		await api('admin/update-meta', { federation: 'all' }, alice as misskey.entities.SignupResponse);
	}, 1000 * 60 * 2);

	describe('announcements/show', () => {
		const ids = new IdService(loadConfig());
		const createdAt = Date.UTC(2000, 0, 1);
		const owned: string[] = [];
		const noSuchAnnouncement = { error: {
			message: 'No such announcement.', code: 'NO_SUCH_ANNOUNCEMENT',
			id: 'b57b5e1d-4f49-404a-9edb-46b00268f121', kind: 'client',
		} };
		let connection: DataSource;
		let global: MiAnnouncement;
		let targetedUnread: MiAnnouncement;
		let targetedRead: MiAnnouncement;

		async function announcement(options: Partial<MiAnnouncement> = {}) {
			const repository = connection.getRepository(MiAnnouncement);
			const row = repository.create({
				id: ids.gen(), updatedAt: null, title: '', text: '', imageUrl: null,
				icon: 'info', display: 'normal', needConfirmationToRead: false,
				isActive: true, forExistingUsers: false, silence: false, userId: null, ...options,
			});
			owned.push(row.id);
			await repository.insert(row);
			return row;
		}

		beforeAll(async () => {
			connection = await initTestDb(true);
			// Direct fixture insertion avoids publishing announcement notifications.
			global = await announcement({ id: ids.gen(createdAt) });
			targetedUnread = await announcement({ userId: bob.id, title: 'For you 雲😀' });
			targetedRead = await announcement({ userId: bob.id, title: 'Already read' });
			await connection.getRepository(MiAnnouncementRead).insert([
				{ id: ids.gen(), announcementId: global.id, userId: alice.id },
				{ id: ids.gen(), announcementId: targetedRead.id, userId: bob.id },
			]);
		}, 120_000);

		afterAll(async () => {
			if (!connection) return;
			try {
				if (owned.length > 0) await connection.getRepository(MiAnnouncement).delete(owned);
			} finally {
				await connection.destroy();
			}
		});

		test('global anonymous detail preserves nulls, omission and response key order', async () => {
			const response = await api('announcements/show', { announcementId: global.id, extra: { accepted: true } });
			assert.strictEqual(response.status, 200);
			assert.deepStrictEqual(response.body, {
				id: global.id, createdAt: new Date(createdAt).toISOString(), updatedAt: null,
				title: '', text: '', imageUrl: null, icon: 'info', display: 'normal',
				forYou: false, needConfirmationToRead: false, silence: false,
			});
			assert.deepStrictEqual(Object.keys(response.body), [
				'id', 'createdAt', 'updatedAt', 'title', 'text', 'imageUrl', 'icon', 'display',
				'forYou', 'needConfirmationToRead', 'silence',
			]);
			assert.strictEqual(Object.hasOwn(response.body, 'isRead'), false);
			assert.strictEqual(response.headers.get('cache-control'), 'private, max-age=0, must-revalidate');
		});

		test('read state is viewer-specific and targeted detail has no administrator bypass or implicit mark-read', async () => {
			for (const [row, viewer, isRead, forYou] of [
				[global, bob, false, false], [global, alice, true, false],
				[targetedUnread, bob, false, true], [targetedRead, bob, true, true],
			] as const) {
				for (let request = 0; request < 2; request++) {
					const response = await api('announcements/show', { announcementId: row.id }, viewer);
					assert.strictEqual(response.status, 200);
					assert.strictEqual(response.body.isRead, isRead);
					assert.strictEqual(response.body.forYou, forYou);
					assert.strictEqual(Object.keys(response.body).at(-1), 'isRead');
				}
			}
			for (const row of [targetedUnread, targetedRead]) {
				for (const viewer of [undefined, carol, alice]) {
					const response = await api('announcements/show', { announcementId: row.id }, viewer);
					assert.strictEqual(response.status, 400);
					assert.deepStrictEqual(response.body, noSuchAnnouncement);
				}
			}
			const reads = connection.getRepository(MiAnnouncementRead);
			assert.strictEqual(await reads.countBy({ announcementId: targetedUnread.id }), 0);
			assert.strictEqual(await reads.countBy({ announcementId: global.id, userId: bob.id }), 0);
			assert.strictEqual(await reads.countBy({ announcementId: global.id, userId: alice.id }), 1);
			assert.strictEqual(await reads.countBy({ announcementId: targetedRead.id, userId: bob.id }), 1);
		});

		test('inactive, existing-users-only and silent announcements remain directly visible', async () => {
			const row = await announcement({
				id: ids.gen(createdAt), title: 'Archived 雲😀', text: '本文\nUnicode 😀',
				updatedAt: new Date('1969-12-31T23:59:59.999Z'), imageUrl: 'https://example.test/opaque-image',
				icon: 'warning', display: 'dialog', isActive: false, forExistingUsers: true,
				silence: true, needConfirmationToRead: true,
			});
			for (const viewer of [undefined, bob]) {
				const response = await api('announcements/show', { announcementId: row.id }, viewer);
				assert.strictEqual(response.status, 200);
				assert.strictEqual(response.body.title, row.title);
				assert.strictEqual(response.body.text, row.text);
				assert.strictEqual(response.body.updatedAt, '1969-12-31T23:59:59.999Z');
				assert.strictEqual(response.body.imageUrl, row.imageUrl);
				assert.strictEqual(response.body.icon, 'warning');
				assert.strictEqual(response.body.display, 'dialog');
				assert.strictEqual(response.body.forYou, false);
				assert.strictEqual(response.body.silence, true);
				assert.strictEqual(response.body.needConfirmationToRead, true);
				assert.strictEqual(Object.hasOwn(response.body, 'isRead'), viewer !== undefined);
				if (viewer) assert.strictEqual(response.body.isRead, false);
			}
		});

		test('missing rows, invalid parameters and GET retain their public errors', async () => {
			const missing = await api('announcements/show', { announcementId: ids.gen() });
			assert.strictEqual(missing.status, 400);
			assert.deepStrictEqual(missing.body, noSuchAnnouncement);
			for (const params of [{}, { announcementId: null }, { announcementId: 0 }, { announcementId: '' }, { announcementId: '!' }]) {
				const response = await relativeFetch('api/announcements/show', {
					method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(params),
				});
				assert.strictEqual(response.status, 400);
				assert.strictEqual(castAsError(await response.json() as Record<string, unknown>).error.code, 'INVALID_PARAM');
			}
			const get = await relativeFetch(`api/announcements/show?announcementId=${encodeURIComponent(global.id)}`);
			assert.strictEqual(get.status, 405);
			assert.strictEqual(await get.text(), '');
		});

		test('a hidden targeted announcement with an unformattable timestamp still returns NO_SUCH', async () => {
			const hidden = await announcement({ userId: bob.id });
			// Infinity is a legal PostgreSQL timestamp. Privacy must precede date packing.
			await connection.query('UPDATE announcement SET "updatedAt" = $1::timestamptz WHERE id = $2', ['infinity', hidden.id]);
			for (const viewer of [undefined, carol, alice]) {
				const response = await api('announcements/show', { announcementId: hidden.id }, viewer);
				assert.strictEqual(response.status, 400);
				assert.deepStrictEqual(response.body, noSuchAnnouncement);
			}
		});
	});

	describe('bubble-game/ranking', () => {
		const ids = new IdService(loadConfig());
		const day = 24 * 60 * 60 * 1000;

		async function withRecords(run: (insert: (gameMode: string, score: number, options?: Partial<MiBubbleGameRecord>) => Promise<MiBubbleGameRecord>) => Promise<void>) {
			const connection = await initTestDb(true);
			const records = connection.getRepository(MiBubbleGameRecord);
			const owned: string[] = [];
			try {
				await run(async (gameMode, score, options = {}) => {
					const record = records.create({
						id: ids.gen(), userId: bob.id, gameMode, score,
						seededAt: new Date(Date.now() - 6 * day), seed: 'ranking-e2e',
						gameVersion: 1, logs: [], isVerified: false, ...options,
					});
					owned.push(record.id);
					await records.insert(record);
					return record;
				});
			} finally {
				try {
					if (owned.length > 0) await records.delete(owned);
				} finally {
					await connection.destroy();
				}
			}
		}

		test('公開上位10件はスコア降順で、重複作者・未検証記録・符号付きスコアを保持する', async () => {
			await withRecords(async insert => {
				const gameMode = `ranking-${randomUUID()}`;
				const scores = [2147483647, 90, 80, 70, 60, 50, 40, 30, 20, 10, 0];
				const rows: MiBubbleGameRecord[] = [];
				for (const [index, score] of scores.entries()) {
					rows.push(await insert(gameMode, score, { userId: index % 2 === 0 ? bob.id : carol.id, isVerified: index % 2 === 0 }));
				}
				// Use stable controls on either side of seven days; HTTP does not freeze the server clock.
				await insert(gameMode, 2147483647, { seededAt: new Date(Date.now() - 8 * day) });
				const response = await api('bubble-game/ranking', { gameMode });
				assert.strictEqual(response.status, 200);
				assert.deepStrictEqual(response.body.map(row => [row.id, row.score, row.user?.id]),
					rows.slice(0, 10).map(row => [row.id, row.score, row.userId]));
				for (const row of response.body) assert.deepStrictEqual(Object.keys(row).sort(), ['id', 'score', 'user']);

				const arbitraryMode = `${gameMode}-雲😀`;
				const signed: MiBubbleGameRecord[] = [];
				for (const score of [-2147483648, -1, 0]) signed.push(await insert(arbitraryMode, score));
				const other = await api('bubble-game/ranking', { gameMode: arbitraryMode });
				assert.strictEqual(other.status, 200);
				assert.deepStrictEqual(other.body.map(row => [row.id, row.score]), signed.reverse().map(row => [row.id, row.score]));
			});
		});

		test('認証済み管理者にも匿名UserLiteと公開バッジだけを返し、GETのキャッシュ期限を保持する', async () => {
			await withRecords(async insert => {
				const gameMode = `ranking-${randomUUID()}`;
				await insert(gameMode, 1);
				const badges: misskey.entities.Role[] = [];
				try {
					for (const isPublic of [true, false]) {
						const badge = await role(alice, { name: `${gameMode}-${isPublic}`, asBadge: true, isPublic });
						badges.push(badge);
						const assigned = await api('admin/roles/assign', { roleId: badge.id, userId: bob.id }, alice);
						assert.strictEqual(assigned.status, 204);
					}
					const anonymous = await api('bubble-game/ranking', { gameMode });
					const authenticated = await api('bubble-game/ranking', { gameMode }, alice);
					assert.strictEqual(anonymous.status, 200);
					assert.strictEqual(authenticated.status, 200);
					assert.deepStrictEqual(authenticated.body, anonymous.body);
					const user = anonymous.body[0].user;
					assert.ok(user);
					assert.strictEqual(user.id, bob.id);
					assert.strictEqual(user.username, bob.username);
					assert.deepStrictEqual(user.badgeRoles, [{ name: badges[0].name, iconUrl: null, displayOrder: 0 }]);
					for (const field of ['token', 'email', 'description', 'isFollowing', 'isBlocking', 'isAdmin']) {
						assert.strictEqual(Object.hasOwn(user, field), false);
					}
					const get = await relativeFetch(`api/bubble-game/ranking?gameMode=${encodeURIComponent(gameMode)}`);
					assert.strictEqual(get.status, 200);
					assert.deepStrictEqual(await get.json(), anonymous.body);
					assert.strictEqual(get.headers.get('cache-control'), 'public, max-age=60');
					assert.strictEqual(anonymous.headers.get('cache-control'), 'private, max-age=0, must-revalidate');
					assert.strictEqual(authenticated.headers.get('cache-control'), 'private, max-age=0, must-revalidate');
				} finally {
					const statuses: number[] = [];
					for (const badge of badges) {
						const deleted = await api('admin/roles/delete', { roleId: badge.id }, alice);
						statuses.push(deleted.status);
					}
					assert.ok(statuses.every(status => status === 204));
				}
			});
		});

		test('モードは必須の文字列で、空文字・長い文字列・孤立UTF16サロゲートを扱う', async () => {
			for (const params of [{}, { gameMode: null }, { gameMode: 0 }, { gameMode: [] }, { gameMode: {} }]) {
				const invalid = await relativeFetch('api/bubble-game/ranking', {
					method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(params),
				});
				assert.strictEqual(invalid.status, 400);
				const body = castAsError(await invalid.json() as Record<string, unknown>);
				assert.strictEqual(body.error.code, 'INVALID_PARAM');
			}
			for (const gameMode of ['', 'x'.repeat(129)]) {
				const empty = await api('bubble-game/ranking', { gameMode });
				assert.strictEqual(empty.status, 200);
				assert.deepStrictEqual(empty.body, []);
			}
			await withRecords(async insert => {
				const prefix = `ranking-${randomUUID()}-`;
				const record = await insert(`${prefix}\uFFFD`, 7);
				for (const suffix of ['\uFFFD', '\uD800', '\uDC00']) {
					const response = await api('bubble-game/ranking', { gameMode: prefix + suffix });
					assert.strictEqual(response.status, 200);
					assert.deepStrictEqual(response.body.map(row => row.id), [record.id]);
				}
			});
		});

		test('NULを含むモードの公開500はQueryFailedErrorの詳細を保持する', async () => {
			const response = await api('bubble-game/ranking', { gameMode: 'ranking\u0000' });
			assert.strictEqual(response.status, 500);
			const error = castAsError(response.body as unknown as Record<string, unknown>).error;
			assert.strictEqual(error.code, 'INTERNAL_ERROR');
			assert.strictEqual(error.kind, 'server');
			assert.strictEqual(error.id, '5d37dbcb-891e-41ca-a3d6-e690c97775ac');
			assert.strictEqual(error.message, 'Internal error occurred. Please contact us if the error persists.');
			const info = error.info;
			assert.ok(info !== null && typeof info === 'object' && 'e' in info);
			const cause = info.e;
			assert.ok(cause !== null && typeof cause === 'object' && 'code' in cause && 'message' in cause && 'id' in cause);
			assert.strictEqual(typeof cause.id, 'string');
			assert.match(cause.id, /^[0-9a-f-]{36}$/);
			assert.deepStrictEqual(response.body, { error: {
				message: 'Internal error occurred. Please contact us if the error persists.',
				code: 'INTERNAL_ERROR', id: '5d37dbcb-891e-41ca-a3d6-e690c97775ac', kind: 'server',
				info: { e: { message: 'invalid byte sequence for encoding "UTF8": 0x00', code: 'QueryFailedError', id: cause.id } },
			} });
		});
	});

	describe('signup', () => {
		test('不正なユーザー名でアカウントが作成できない', async () => {
			const res = await api('signup', {
				username: 'test.',
				password: 'test',
			});
			assert.strictEqual(res.status, 400);
		});

		test('空のパスワードでアカウントが作成できない', async () => {
			const res = await api('signup', {
				username: 'test',
				password: '',
			});
			assert.strictEqual(res.status, 400);
		});

		test('正しくアカウントが作成できる', async () => {
			const me = {
				username: 'test1',
				password: 'test1',
			};

			const res = await api('signup', me);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.username, me.username);
		});

		test('同じユーザー名のアカウントは作成できない', async () => {
			const res = await api('signup', {
				username: 'test1',
				password: 'test1',
			});

			assert.strictEqual(res.status, 400);
		});
	});

	describe('signin-flow', () => {
		test('間違ったパスワードでサインインできない', async () => {
			const res = await api('signin-flow', {
				username: 'test1',
				password: 'bar',
			});

			assert.strictEqual(res.status, 403);
		});

		test('クエリをインジェクションできない', async () => {
			const res = await api('signin-flow', {
				username: 'test1',
				// @ts-expect-error password must be string
				password: {
					$gt: '',
				},
			});

			assert.strictEqual(res.status, 400);
		});

		test('正しい情報でサインインできる', async () => {
			const res = await api('signin-flow', {
				username: 'test1',
				password: 'test1',
			});

			assert.strictEqual(res.status, 200);
		});
	});

	describe('i/update', () => {
		test('アカウント設定を更新できる', async () => {
			const myName = '大室櫻子';
			const myLocation = '七森中';
			const myBirthday = '2000-09-07';

			const res = await api('i/update', {
				name: myName,
				location: myLocation,
				birthday: myBirthday,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.name, myName);
			assert.strictEqual(res.body.location, myLocation);
			assert.strictEqual(res.body.birthday, myBirthday);
		});

		test('名前を空白のみにした場合nullになる', async () => {
			const res = await api('i/update', {
				name: ' ',
			}, alice);
			assert.strictEqual(res.status, 200);
			assert.strictEqual(res.body.name, null);
		});

		test('名前の前後に空白（ホワイトスペース）を入れてもトリムされる', async () => {
			const res = await api('i/update', {
				// https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Lexical_grammar#white_space
				name: ' あ い う \u0009\u000b\u000c\u0020\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u202f\u205f\u3000\ufeff',
			}, alice);
			assert.strictEqual(res.status, 200);
			assert.strictEqual(res.body.name, 'あ い う');
		});

		test('誕生日の設定を削除できる', async () => {
			await api('i/update', {
				birthday: '2000-09-07',
			}, alice);

			const res = await api('i/update', {
				birthday: null,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.birthday, null);
		});

		test('不正な誕生日の形式で怒られる', async () => {
			const res = await api('i/update', {
				birthday: '2000/09/07',
			}, alice);
			assert.strictEqual(res.status, 400);
		});
	});

	describe('users/show', () => {
		test('ユーザーが取得できる', async () => {
			const res = await api('users/show', {
				userId: alice.id,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual((res.body as unknown as { id: string }).id, alice.id);
		});

		test('ユーザーが存在しなかったら怒る', async () => {
			const res = await api('users/show', {
				userId: '000000000000000000000000',
			});
			assert.strictEqual(res.status, 404);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('users/show', {
				userId: 'kyoppie',
			});
			assert.strictEqual(res.status, 404);
		});
	});

	describe('gallery/posts/show', () => {
		let gallery: misskey.entities.GalleryPost;
		let fileIds: string[];
		let galleryId: string | null = null;
		const uploadedFileIds: string[] = [];

		beforeAll(async () => {
			const first = await uploadFile(alice);
			assert.strictEqual(first.status, 200);
			assert.ok(first.body !== null);
			uploadedFileIds.push(first.body.id);
			const second = await uploadFile(alice, { path: '192.png' });
			assert.strictEqual(second.status, 200);
			assert.ok(second.body !== null);
			uploadedFileIds.push(second.body.id);
			fileIds = [second.body.id, first.body.id];
			const created = await api('gallery/posts/create', {
				title: 'Gallery detail test',
				description: null,
				fileIds,
				isSensitive: true,
			}, alice);
			assert.strictEqual(created.status, 200);
			gallery = created.body;
			galleryId = created.body.id;
		});

		afterAll(async () => {
			// Later tests expect Alice's drive to be empty. Reclaim only the
			// resources created here, including uploads if setup fails partway.
			try {
				if (galleryId !== null) {
					const deleted = await api('gallery/posts/delete', { postId: galleryId }, alice);
					assert.strictEqual(deleted.status, 204);
				}
			} finally {
				for (const fileId of uploadedFileIds) {
					const deleted = await api('drive/files/delete', { fileId }, alice);
					assert.strictEqual(deleted.status, 204);
				}
			}
		});

		test('匿名の詳細で添付順序と作者を保持し、空のタグとisLikedを省略する', async () => {
			const response = await api('gallery/posts/show', { postId: gallery.id });
			assert.strictEqual(response.status, 200);
			assert.strictEqual(response.body.id, gallery.id);
			assert.strictEqual(response.body.title, gallery.title);
			assert.strictEqual(response.body.description, null);
			assert.strictEqual(response.body.isSensitive, true);
			assert.strictEqual(response.body.userId, alice.id);
			assert.strictEqual(response.body.user.id, alice.id);
			assert.deepStrictEqual(response.body.fileIds, fileIds);
			const files = response.body.files;
			assert.ok(Array.isArray(files));
			assert.deepStrictEqual(files, gallery.files);
			assert.deepStrictEqual(files.map(file => file.id), fileIds);
			for (const file of files) {
				assert.strictEqual(file.userId, alice.id);
				assert.strictEqual(file.user, null);
				assert.strictEqual(file.folder, null);
			}
			assert.strictEqual(Object.hasOwn(response.body, 'tags'), false);
			assert.strictEqual(Object.hasOwn(response.body, 'isLiked'), false);
		});

		test('いいねした閲覧者だけisLikedがtrueになり、保存済み件数を返す', async () => {
			const ownerBefore = await api('gallery/posts/show', { postId: gallery.id }, alice);
			assert.strictEqual(ownerBefore.status, 200);
			assert.strictEqual(ownerBefore.body.isLiked, false);
			assert.strictEqual(ownerBefore.body.likedCount, 0);
			const liked = await api('gallery/posts/like', { postId: gallery.id }, bob);
			assert.strictEqual(liked.status, 204);
			// The existing TS writer starts the counter increment without awaiting
			// it. Wait for that stored value through the public read contract.
			await vi.waitFor(async () => {
				const viewer = await api('gallery/posts/show', { postId: gallery.id }, bob);
				assert.strictEqual(viewer.status, 200);
				assert.strictEqual(viewer.body.isLiked, true);
				assert.strictEqual(viewer.body.likedCount, 1);
			}, waitForPushToTlOptions);
			const owner = await api('gallery/posts/show', { postId: gallery.id }, alice);
			assert.strictEqual(owner.status, 200);
			assert.strictEqual(owner.body.isLiked, false);
			assert.strictEqual(owner.body.likedCount, 1);
			const anonymous = await api('gallery/posts/show', { postId: gallery.id });
			assert.strictEqual(anonymous.status, 200);
			assert.strictEqual(anonymous.body.likedCount, 1);
			assert.strictEqual(Object.hasOwn(anonymous.body, 'isLiked'), false);
		});

		test('存在しない投稿はNO_SUCH_POSTになり、GETは405のままになる', async () => {
			const missing = await api('gallery/posts/show', { postId: '0000000000000000' });
			assert.strictEqual(missing.status, 400);
			assert.deepStrictEqual(missing.body, { error: {
				message: 'No such post.',
				code: 'NO_SUCH_POST',
				id: '1137bf14-c5b0-4604-85bb-5b5371b1cd45',
				kind: 'client',
			} });
			const get = await relativeFetch(`api/gallery/posts/show?postId=${encodeURIComponent(gallery.id)}`);
			assert.strictEqual(get.status, 405);
			assert.strictEqual(await get.text(), '');
		});
	});

	describe('gallery/featured', () => {
		const missingId = 'zzzzzzzzzzzzzzzz';
		const posts: misskey.entities.GalleryPost[] = [];
		const ownedPostIds = new Set<string>();
		const saved: { key: string; value: Buffer | null; expires: number }[] = [];
		let redis: Redis | undefined;
		let fileId: string | null = null;
		let ranking: [string, string];
		let cacheWarmed = false;

		async function createPost(title: string) {
			assert.ok(fileId !== null);
			const created = await api('gallery/posts/create', { title, fileIds: [fileId] }, alice);
			assert.strictEqual(created.status, 200);
			ownedPostIds.add(created.body.id);
			return created.body;
		}

		beforeAll(async () => {
			redis = new Redis(loadConfig().redis);
			const uploaded = await uploadFile(alice);
			assert.strictEqual(uploaded.status, 200);
			assert.ok(uploaded.body !== null);
			fileId = uploaded.body.id;
			for (let index = 0; index < 3; index++) {
				posts.push(await createPost(`Featured ${index}`));
			}
			assert.ok(posts.every(post => post.id < missingId));
			const epoch = Date.parse('2023-01-01T00:00:00Z');
			const duration = 3 * 24 * 60 * 60 * 1000;
			const remaining = duration - ((Date.now() - epoch) % duration);
			if (remaining < 2000) await setTimeout(remaining + 100);
			const window = Math.floor((Date.now() - epoch) / duration);
			ranking = [`featuredGalleryPostsRanking:${window}`, `featuredGalleryPostsRanking:${window - 1}`];
			for (const key of ranking) {
				const value = await redis.dumpBuffer(key);
				const ttl = await redis.pttl(key);
				saved.push({ key, value, expires: ttl >= 0 ? Date.now() + ttl : ttl });
			}
			await redis.del(...ranking);
			const first = posts.at(0);
			const second = posts.at(1);
			const third = posts.at(2);
			assert.ok(first !== undefined && second !== undefined && third !== undefined);
			await redis.zadd(ranking[0], 500, missingId, 900, first.id, 2, second.id);
			await redis.zadd(ranking[1], 300, first.id, 1000, third.id);
		});

		afterAll(async () => {
			const failures: unknown[] = [];
			for (const postId of ownedPostIds) {
				try {
					const deleted = await api('gallery/posts/delete', { postId }, alice);
					assert.strictEqual(deleted.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (fileId !== null) {
				try {
					const deleted = await api('drive/files/delete', { fileId }, alice);
					assert.strictEqual(deleted.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (cacheWarmed && failures.length === 0) {
				try {
					const empty = await api('gallery/featured', { limit: 100 });
					assert.strictEqual(empty.status, 200);
					assert.deepStrictEqual(empty.body, []);
				} catch (error) { failures.push(error); }
			}
			if (redis !== undefined) {
				for (const { key, value, expires } of saved) {
					try {
						await redis.del(key);
						if (value === null || expires === -2) continue;
						const ttl = expires < 0 ? 0 : expires - Date.now();
						if (expires < 0 || ttl > 0) await redis.restore(key, ttl, value, 'REPLACE');
					} catch (error) { failures.push(error); }
				}
				try { await redis.quit(); } catch (error) { failures.push(error); }
			}
			if (failures.length > 0) throw new AggregateError(failures, 'Featured fixture cleanup failed');
		});

		test('IDだけをキャッシュし、絞り込みと閲覧者ごとの最新表示を保持する', async () => {
			assert.ok(redis !== undefined);
			const descending = posts.map(post => post.id).sort().reverse();
			const newestId = descending.at(0);
			const nextId = descending.at(1);
			assert.ok(newestId !== undefined && nextId !== undefined);
			// Selection sorts IDs before slicing and does not refill a missing row.
			const missing = await api('gallery/featured', { limit: 1 });
			assert.strictEqual(missing.status, 200);
			assert.deepStrictEqual(missing.body, []);
			cacheWarmed = true;
			const selected = await api('gallery/featured', { limit: 2 });
			assert.strictEqual(selected.status, 200);
			assert.deepStrictEqual(selected.body.map(post => post.id), [newestId]);
			const until = await api('gallery/featured', { limit: 1, untilId: newestId });
			assert.strictEqual(until.status, 200);
			assert.deepStrictEqual(until.body.map(post => post.id), [nextId]);

			const later = await createPost('Not in the cached ranking');
			await redis.del(...ranking);
			await redis.zadd(ranking[0], 1, later.id);
			const anonymous = await api('gallery/featured', { limit: 100 });
			assert.strictEqual(anonymous.status, 200);
			assert.ok(Array.isArray(anonymous.body));
			// The query has no ORDER BY. Check membership here; differential
			// evidence separately compares the actual returned database row order.
			assert.deepStrictEqual(anonymous.body.map(post => post.id).sort(), [...descending].sort());
			assert.ok(anonymous.body.every(post => !Object.hasOwn(post, 'isLiked')));
			const owner = await api('gallery/featured', { limit: 100 }, alice);
			assert.strictEqual(owner.status, 200);
			assert.deepStrictEqual(owner.body.map(post => post.id).sort(), [...descending].sort());
			assert.ok(owner.body.every(post => post.isLiked === false));

			const liked = await api('gallery/posts/like', { postId: newestId }, bob);
			assert.strictEqual(liked.status, 204);
			// The TS writer's stored count increment is asynchronous, but a cached
			// ranking must still pack current rows and the current viewer each time.
			await vi.waitFor(async () => {
				const viewer = await api('gallery/featured', {}, bob);
				assert.strictEqual(viewer.status, 200);
				assert.deepStrictEqual(viewer.body.map(post => post.id).sort(), [...descending].sort());
				const post = viewer.body.find(post => post.id === newestId);
				assert.ok(post !== undefined);
				assert.strictEqual(post.isLiked, true);
				assert.strictEqual(post.likedCount, 1);
			}, waitForPushToTlOptions);
			const ownerAfter = await api('gallery/featured', { limit: 100 }, alice);
			assert.strictEqual(ownerAfter.status, 200);
			const ownerPost = ownerAfter.body.find(post => post.id === newestId);
			assert.ok(ownerPost !== undefined);
			assert.strictEqual(ownerPost.isLiked, false);
			assert.strictEqual(ownerPost.likedCount, 1);
			for (const params of [{ limit: 0 }, { limit: 101 }, { untilId: '!' }]) {
				const invalid = await api('gallery/featured', params);
				assert.strictEqual(invalid.status, 400);
				const body: unknown = invalid.body;
				assert.ok(body !== null && typeof body === 'object' && 'error' in body);
				const error = body.error;
				assert.ok(error !== null && typeof error === 'object' && 'code' in error);
				assert.strictEqual(error.code, 'INVALID_PARAM');
			}
			const get = await relativeFetch('api/gallery/featured?limit=100');
			assert.strictEqual(get.status, 405);
			assert.strictEqual(await get.text(), '');
		});
	});

	describe('flash/show', () => {
		const script = 'Ui:render([Ui:C:text({ text: "Flash E2E 雲😀" })])';
		const badgeName = 'Flash private badge';
		let owner: misskey.entities.SignupResponse;
		let reader: misskey.entities.SignupResponse;
		let flash: misskey.entities.Flash;
		let flashId: string | null = null;
		let badgeRoleId: string | null = null;
		let badgeAssigned = false;
		const likedFlashIds = new Set<string>();

		beforeAll(async () => {
			owner = await signup({ username: 'flash_show_owner' });
			reader = await signup({ username: 'flash_show_reader' });
			const badge = await api('admin/roles/create', {
				name: badgeName,
				description: '',
				color: null,
				iconUrl: null,
				target: 'manual',
				condFormula: {},
				isPublic: false,
				isModerator: false,
				isAdministrator: false,
				asBadge: true,
				canEditMembersByModerator: false,
				displayOrder: 0,
				policies: {},
			}, alice);
			assert.strictEqual(badge.status, 200);
			badgeRoleId = badge.body.id;
			const assigned = await api('admin/roles/assign', { roleId: badgeRoleId, userId: owner.id }, alice);
			assert.strictEqual(assigned.status, 204);
			badgeAssigned = true;
			const created = await api('flash/create', {
				title: 'Flash detail 雲😀 e\u0301',
				summary: '',
				script,
				permissions: [],
				visibility: 'public',
			}, owner);
			assert.strictEqual(created.status, 200);
			flash = created.body;
			flashId = created.body.id;
		});

		afterAll(async () => {
			const failures: unknown[] = [];
			for (const id of likedFlashIds) {
				try {
					const unliked = await api('flash/unlike', { flashId: id }, reader);
					assert.strictEqual(unliked.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (flashId !== null) {
				try {
					const deleted = await api('flash/delete', { flashId }, owner);
					assert.strictEqual(deleted.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (badgeRoleId !== null) {
				if (badgeAssigned) {
					try {
						const unassigned = await api('admin/roles/unassign', { roleId: badgeRoleId, userId: owner.id }, alice);
						assert.strictEqual(unassigned.status, 204);
					} catch (error) { failures.push(error); }
				}
				try {
					const deleted = await api('admin/roles/delete', { roleId: badgeRoleId }, alice);
					assert.strictEqual(deleted.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (failures.length > 0) throw new AggregateError(failures, 'Flash fixture cleanup failed');
		});

		test('privateも詳細を公開し、実際の閲覧者で作者といいねを表示する', async () => {
			const viewers = [
				{ user: undefined, hasPrivateBadge: false },
				{ user: owner, hasPrivateBadge: false },
				{ user: reader, hasPrivateBadge: false },
				{ user: alice, hasPrivateBadge: true },
			];
			const visibilities: ('public' | 'private')[] = ['public', 'private'];
			for (const visibility of visibilities) {
				if (visibility === 'private') {
					const updated = await api('flash/update', { flashId: flash.id, visibility }, owner);
					assert.strictEqual(updated.status, 204);
				}
				for (const viewer of viewers) {
					const response = await api('flash/show', { flashId: flash.id }, viewer.user);
					assert.strictEqual(response.status, 200);
					assert.strictEqual(response.body.id, flash.id);
					assert.strictEqual(response.body.visibility, visibility);
					assert.strictEqual(response.body.title, flash.title);
					assert.strictEqual(response.body.summary, '');
					assert.strictEqual(response.body.script, script);
					assert.strictEqual(response.body.createdAt, flash.createdAt);
					assert.strictEqual(response.body.userId, owner.id);
					assert.strictEqual(response.body.user.id, owner.id);
					const badges = response.body.user.badgeRoles;
					assert.ok(Array.isArray(badges));
					assert.strictEqual(badges.some(badge => badge.name === badgeName), viewer.hasPrivateBadge);
					assert.strictEqual(Object.hasOwn(response.body, 'permissions'), false);
					assert.strictEqual(response.body.likedCount, 0);
					if (viewer.user === undefined) assert.strictEqual(Object.hasOwn(response.body, 'isLiked'), false);
					else assert.strictEqual(response.body.isLiked, false);
				}
			}

			const liked = await api('flash/like', { flashId: flash.id }, reader);
			assert.strictEqual(liked.status, 204);
			likedFlashIds.add(flash.id);
			// Existing TS like/unlike writes start the count update asynchronously.
			await vi.waitFor(async () => {
				const response = await api('flash/show', { flashId: flash.id }, reader);
				assert.strictEqual(response.status, 200);
				assert.strictEqual(response.body.isLiked, true);
				assert.strictEqual(response.body.likedCount, 1);
			}, waitForPushToTlOptions);
			const ownerAfter = await api('flash/show', { flashId: flash.id }, owner);
			assert.strictEqual(ownerAfter.status, 200);
			assert.strictEqual(ownerAfter.body.isLiked, false);
			assert.strictEqual(ownerAfter.body.likedCount, 1);
			const anonymous = await api('flash/show', { flashId: flash.id });
			assert.strictEqual(anonymous.status, 200);
			assert.strictEqual(Object.hasOwn(anonymous.body, 'isLiked'), false);
			assert.strictEqual(anonymous.body.likedCount, 1);
			const unliked = await api('flash/unlike', { flashId: flash.id }, reader);
			assert.strictEqual(unliked.status, 204);
			likedFlashIds.delete(flash.id);
			await vi.waitFor(async () => {
				const response = await api('flash/show', { flashId: flash.id }, reader);
				assert.strictEqual(response.status, 200);
				assert.strictEqual(response.body.isLiked, false);
				assert.strictEqual(response.body.likedCount, 0);
			}, waitForPushToTlOptions);
		});

		test('未知のFlashは固有400で、不正な入力とGETを拒否する', async () => {
			const missing = await api('flash/show', { flashId: '0000000000000000' });
			assert.strictEqual(missing.status, 400);
			assert.deepStrictEqual(missing.body, { error: {
				message: 'No such flash.',
				code: 'NO_SUCH_FLASH',
				id: 'f0d34a1a-d29a-401d-90ba-1982122b5630',
				kind: 'client',
			} });
			for (const params of [{}, { flashId: '!' }, { flashId: 0 }]) {
				const invalid = await relativeFetch('api/flash/show', {
					method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(params),
				});
				assert.strictEqual(invalid.status, 400);
				const body: unknown = await invalid.json();
				assert.ok(body !== null && typeof body === 'object' && 'error' in body);
				const error = body.error;
				assert.ok(error !== null && typeof error === 'object' && 'code' in error);
				assert.strictEqual(error.code, 'INVALID_PARAM');
			}
			const get = await relativeFetch(`api/flash/show?flashId=${encodeURIComponent(flash.id)}`);
			assert.strictEqual(get.status, 405);
			assert.strictEqual(await get.text(), '');
		});
	});

	describe('flash/featured', () => {
		const script = 'Ui:render([Ui:C:text({ text: "Featured 雲😀" })])';
		const badgeName = 'Featured Flash private badge';
		const flashIds = new Set<string>();
		const likes: { flashId: string; user: misskey.entities.SignupResponse }[] = [];
		let older: misskey.entities.Flash;
		let newer: misskey.entities.Flash;
		let popular: misskey.entities.Flash;
		let badgeRoleId: string | null = null;
		let badgeAssigned = false;

		async function createFlash(title: string, visibility: 'public' | 'private' = 'public') {
			const response = await api('flash/create', { title, summary: '', script, permissions: [], visibility }, bob);
			assert.strictEqual(response.status, 200);
			flashIds.add(response.body.id);
			return response.body;
		}

		async function likeFlash(flashId: string, user: misskey.entities.SignupResponse) {
			const response = await api('flash/like', { flashId }, user);
			assert.strictEqual(response.status, 204);
			likes.push({ flashId, user });
		}

		beforeAll(async () => {
			const badge = await api('admin/roles/create', {
				name: badgeName, description: '', color: null, iconUrl: null, target: 'manual', condFormula: {},
				isPublic: false, isModerator: false, isAdministrator: false, asBadge: true,
				canEditMembersByModerator: false, displayOrder: 0, policies: {},
			}, alice);
			assert.strictEqual(badge.status, 200);
			badgeRoleId = badge.body.id;
			const assigned = await api('admin/roles/assign', { roleId: badgeRoleId, userId: bob.id }, alice);
			assert.strictEqual(assigned.status, 204);
			badgeAssigned = true;
			older = await createFlash('Older featured');
			popular = await createFlash('Popular featured');
			await setTimeout(2);
			newer = await createFlash('Newer featured 雲😀');
			await createFlash('Public without likes');
			const hidden = await createFlash('Private with likes', 'private');
			for (const flash of [older, popular, newer, hidden]) await likeFlash(flash.id, carol);
			await likeFlash(popular.id, dave);
			// The TS writer starts count updates asynchronously; wait before paging.
			await vi.waitFor(async () => {
				for (const flash of [older, popular, newer, hidden]) {
					const response = await api('flash/show', { flashId: flash.id });
					assert.strictEqual(response.status, 200);
					assert.strictEqual(response.body.likedCount, flash.id === popular.id ? 2 : 1);
				}
			}, waitForPushToTlOptions);
		});

		afterAll(async () => {
			const failures: unknown[] = [];
			for (const { flashId, user } of likes) {
				try {
					const response = await api('flash/unlike', { flashId }, user);
					assert.strictEqual(response.status, 204);
				} catch (error) { failures.push(error); }
			}
			for (const flashId of flashIds) {
				try {
					const response = await api('flash/delete', { flashId }, bob);
					assert.strictEqual(response.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (badgeRoleId !== null) {
				if (badgeAssigned) {
					try {
						const response = await api('admin/roles/unassign', { roleId: badgeRoleId, userId: bob.id }, alice);
						assert.strictEqual(response.status, 204);
					} catch (error) { failures.push(error); }
				}
				try {
					const response = await api('admin/roles/delete', { roleId: badgeRoleId }, alice);
					assert.strictEqual(response.status, 204);
				} catch (error) { failures.push(error); }
			}
			if (failures.length > 0) throw new AggregateError(failures, 'Featured Flash fixture cleanup failed');
		});

		test('公開かついいねがあるPlayだけを人気順・更新順で返し、offsetでページングする', async () => {
			const all = await api('flash/featured', { limit: 100 });
			assert.strictEqual(all.status, 200);
			const owned = all.body.filter(flash => flashIds.has(flash.id));
			assert.deepStrictEqual(owned.map(flash => flash.id), [popular.id, newer.id, older.id]);
			assert.deepStrictEqual(owned.map(flash => flash.likedCount), [2, 1, 1]);
			for (const flash of owned) {
				const badges = flash.user.badgeRoles;
				assert.ok(Array.isArray(badges));
				assert.strictEqual(badges.some(badge => badge.name === badgeName), false);
				assert.strictEqual(Object.hasOwn(flash, 'permissions'), false);
			}
			for (const flash of all.body) {
				assert.strictEqual(flash.visibility, 'public');
				assert.ok(flash.likedCount > 0);
				assert.strictEqual(Object.hasOwn(flash, 'isLiked'), false);
			}
			const defaults = await api('flash/featured', {});
			assert.strictEqual(defaults.status, 200);
			assert.deepStrictEqual(defaults.body, all.body.slice(0, 10));
			for (const params of [
				{ offset: 0, limit: 10 }, { limit: 1 }, { limit: 5 },
				{ offset: 5, limit: 30 }, { offset: 30, limit: 5 },
			]) {
				const response = await api('flash/featured', params);
				assert.strictEqual(response.status, 200);
				const offset = params.offset ?? 0;
				assert.deepStrictEqual(response.body, all.body.slice(offset, offset + params.limit));
			}
		});

		test('閲覧者ごとのいいねと非公開バッジを保持し、入力・メソッド・巨大offsetを扱う', async () => {
			for (const viewer of [bob, carol, dave, alice]) {
				const response = await api('flash/featured', { limit: 100 }, viewer);
				assert.strictEqual(response.status, 200);
				const owned = response.body.filter(flash => flashIds.has(flash.id));
				assert.deepStrictEqual(owned.map(flash => flash.id), [popular.id, newer.id, older.id]);
				for (const flash of owned) {
					assert.strictEqual(flash.userId, bob.id);
					assert.strictEqual(flash.user.id, bob.id);
					assert.strictEqual(flash.script, script);
					assert.strictEqual(flash.summary, '');
					assert.strictEqual(flash.isLiked, viewer === carol || (viewer === dave && flash.id === popular.id));
					const badges = flash.user.badgeRoles;
					assert.ok(Array.isArray(badges));
					assert.strictEqual(badges.some(badge => badge.name === badgeName), viewer === alice);
				}
			}
			const empty = await api('flash/featured', { offset: Number.MAX_SAFE_INTEGER, limit: 5 }, carol);
			assert.strictEqual(empty.status, 200);
			assert.deepStrictEqual(empty.body, []);
			for (const params of [{ offset: -1 }, { offset: 0.5 }, { offset: null }, { limit: 0 }, { limit: 101 }, { limit: '5' }]) {
				const response = await relativeFetch('api/flash/featured', {
					method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(params),
				});
				assert.strictEqual(response.status, 400);
				const body: unknown = await response.json();
				assert.ok(body !== null && typeof body === 'object' && 'error' in body);
				const error = body.error;
				assert.ok(error !== null && typeof error === 'object' && 'code' in error);
				assert.strictEqual(error.code, 'INVALID_PARAM');
			}
			const overflow = await relativeFetch('api/flash/featured', {
				method: 'POST', headers: { 'content-type': 'application/json' },
				body: '{"offset":9223372036854775807,"limit":5}',
			});
			assert.strictEqual(overflow.status, 500);
			const body: unknown = await overflow.json();
			assert.ok(body !== null && typeof body === 'object' && 'error' in body);
			const error = body.error;
			assert.ok(error !== null && typeof error === 'object' && 'code' in error && 'info' in error);
			assert.strictEqual(error.code, 'INTERNAL_ERROR');
			const info = error.info;
			assert.ok(info !== null && typeof info === 'object' && 'e' in info);
			const cause = info.e;
			assert.ok(cause !== null && typeof cause === 'object' && 'code' in cause && 'message' in cause);
			assert.strictEqual(cause.code, 'QueryFailedError');
			assert.strictEqual(cause.message, 'bigint out of range');
			const get = await relativeFetch('api/flash/featured?limit=5');
			assert.strictEqual(get.status, 405);
			assert.strictEqual(await get.text(), '');
		});
	});

	describe('notes/show', () => {
		test('投稿が取得できる', async () => {
			const myPost = await post(alice, {
				text: 'test',
			});

			const res = await api('notes/show', {
				noteId: myPost.id,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.id, myPost.id);
			assert.strictEqual(res.body.text, myPost.text);
		});

		test('投稿が存在しなかったら怒る', async () => {
			const res = await api('notes/show', {
				noteId: '000000000000000000000000',
			});
			assert.strictEqual(res.status, 400);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('notes/show', {
				noteId: 'kyoppie',
			});
			assert.strictEqual(res.status, 400);
		});
	});

	describe('notes/reactions/create', () => {
		test('リアクションできる', async () => {
			const bobPost = await post(bob, { text: 'hi' });

			const res = await api('notes/reactions/create', {
				noteId: bobPost.id,
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 204);

			const resNote = await api('notes/show', {
				noteId: bobPost.id,
			}, alice);

			assert.strictEqual(resNote.status, 200);
			assert.strictEqual(resNote.body.reactions['🚀'], 1);
		});

		test('自分の投稿にもリアクションできる', async () => {
			const myPost = await post(alice, { text: 'hi' });

			const res = await api('notes/reactions/create', {
				noteId: myPost.id,
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 204);
		});

		test('二重にリアクションすると上書きされる', async () => {
			const bobPost = await post(bob, { text: 'hi' });

			await api('notes/reactions/create', {
				noteId: bobPost.id,
				reaction: '🥰',
			}, alice);

			const res = await api('notes/reactions/create', {
				noteId: bobPost.id,
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 204);

			const resNote = await api('notes/show', {
				noteId: bobPost.id,
			}, alice);

			assert.strictEqual(resNote.status, 200);
			assert.deepStrictEqual(resNote.body.reactions, { '🚀': 1 });
		});

		test('存在しない投稿にはリアクションできない', async () => {
			const res = await api('notes/reactions/create', {
				noteId: '000000000000000000000000',
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('リノートにリアクションできない', async () => {
			const bobNote = await post(bob, { text: 'hi' });
			const bobRenote = await post(bob, { renoteId: bobNote.id });

			const res = await api('notes/reactions/create', {
				noteId: bobRenote.id,
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 400);
			assert.ok(res.body);
			assert.strictEqual(castAsError(res.body).error.code, 'CANNOT_REACT_TO_RENOTE');
		});

		test('引用にリアクションできる', async () => {
			const bobNote = await post(bob, { text: 'hi' });
			const bobRenote = await post(bob, { text: 'hi again', renoteId: bobNote.id });

			const res = await api('notes/reactions/create', {
				noteId: bobRenote.id,
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 204);
		});

		test('空文字列のリアクションは\u2764にフォールバックされる', async () => {
			const bobNote = await post(bob, { text: 'hi' });

			const res = await api('notes/reactions/create', {
				noteId: bobNote.id,
				reaction: '',
			}, alice);

			assert.strictEqual(res.status, 204);

			const reaction = await api('notes/reactions', {
				noteId: bobNote.id,
			});

			assert.strictEqual(reaction.body.length, 1);
			assert.strictEqual(reaction.body[0].type, '\u2764');
		});

		test('絵文字ではない文字列のリアクションは\u2764にフォールバックされる', async () => {
			const bobNote = await post(bob, { text: 'hi' });

			const res = await api('notes/reactions/create', {
				noteId: bobNote.id,
				reaction: 'Hello!',
			}, alice);

			assert.strictEqual(res.status, 204);

			const reaction = await api('notes/reactions', {
				noteId: bobNote.id,
			});

			assert.strictEqual(reaction.body.length, 1);
			assert.strictEqual(reaction.body[0].type, '\u2764');
		});

		test('空のパラメータで怒られる', async () => {
			// @ts-expect-error param must not be empty
			const res = await api('notes/reactions/create', {}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('notes/reactions/create', {
				noteId: 'kyoppie',
				reaction: '🚀',
			}, alice);

			assert.strictEqual(res.status, 400);
		});
	});

	describe('following/create', () => {
		test('フォローできる', async () => {
			const res = await api('following/create', {
				userId: alice.id,
			}, bob);

			assert.strictEqual(res.status, 200);

			const connection = await initTestDb(true);
			const Users = connection.getRepository(MiUser);
			const newBob = await Users.findOneByOrFail({ id: bob.id });
			assert.strictEqual(newBob.followersCount, 0);
			assert.strictEqual(newBob.followingCount, 1);
			const newAlice = await Users.findOneByOrFail({ id: alice.id });
			assert.strictEqual(newAlice.followersCount, 1);
			assert.strictEqual(newAlice.followingCount, 0);
			connection.destroy();
		});

		test('既にフォローしている場合は怒る', async () => {
			const res = await api('following/create', {
				userId: alice.id,
			}, bob);

			assert.strictEqual(res.status, 400);
		});

		test('存在しないユーザーはフォローできない', async () => {
			const res = await api('following/create', {
				userId: '000000000000000000000000',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('自分自身はフォローできない', async () => {
			const res = await api('following/create', {
				userId: alice.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('空のパラメータで怒られる', async () => {
			// @ts-expect-error params must not be empty
			const res = await api('following/create', {}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('following/create', {
				userId: 'foo',
			}, alice);

			assert.strictEqual(res.status, 400);
		});
	});

	describe('following/delete', () => {
		test('フォロー解除できる', async () => {
			await api('following/create', {
				userId: alice.id,
			}, bob);

			const res = await api('following/delete', {
				userId: alice.id,
			}, bob);

			assert.strictEqual(res.status, 200);

			const connection = await initTestDb(true);
			const Users = connection.getRepository(MiUser);
			const newBob = await Users.findOneByOrFail({ id: bob.id });
			assert.strictEqual(newBob.followersCount, 0);
			assert.strictEqual(newBob.followingCount, 0);
			const newAlice = await Users.findOneByOrFail({ id: alice.id });
			assert.strictEqual(newAlice.followersCount, 0);
			assert.strictEqual(newAlice.followingCount, 0);
			connection.destroy();
		});

		test('フォローしていない場合は怒る', async () => {
			const res = await api('following/delete', {
				userId: alice.id,
			}, bob);

			assert.strictEqual(res.status, 400);
		});

		test('存在しないユーザーはフォロー解除できない', async () => {
			const res = await api('following/delete', {
				userId: '000000000000000000000000',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('自分自身はフォロー解除できない', async () => {
			const res = await api('following/delete', {
				userId: alice.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('空のパラメータで怒られる', async () => {
			// @ts-expect-error params must not be empty
			const res = await api('following/delete', {}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('following/delete', {
				userId: 'kyoppie',
			}, alice);

			assert.strictEqual(res.status, 400);
		});
	});

	describe('channels/search', () => {
		test('空白検索で一覧を取得できる', async () => {
			await api('channels/create', {
				name: 'aaa',
				description: 'bbb',
			}, bob);
			await api('channels/create', {
				name: 'ccc1',
				description: 'ddd1',
			}, bob);
			await api('channels/create', {
				name: 'ccc2',
				description: 'ddd2',
			}, bob);

			const res = await api('channels/search', {
				query: '',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 3);
		});
		test('名前のみの検索で名前を検索できる', async () => {
			const res = await api('channels/search', {
				query: 'aaa',
				type: 'nameOnly',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 1);
			assert.strictEqual(res.body[0].name, 'aaa');
		});
		test('名前のみの検索で名前を複数検索できる', async () => {
			const res = await api('channels/search', {
				query: 'ccc',
				type: 'nameOnly',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 2);
		});
		test('名前のみの検索で説明は検索できない', async () => {
			const res = await api('channels/search', {
				query: 'bbb',
				type: 'nameOnly',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 0);
		});
		test('名前と説明の検索で名前を検索できる', async () => {
			const res = await api('channels/search', {
				query: 'ccc1',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 1);
			assert.strictEqual(res.body[0].name, 'ccc1');
		});
		test('名前と説明での検索で説明を検索できる', async () => {
			const res = await api('channels/search', {
				query: 'ddd1',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 1);
			assert.strictEqual(res.body[0].name, 'ccc1');
		});
		test('名前と説明の検索で名前を複数検索できる', async () => {
			const res = await api('channels/search', {
				query: 'ccc',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 2);
		});
		test('名前と説明での検索で説明を複数検索できる', async () => {
			const res = await api('channels/search', {
				query: 'ddd',
			}, bob);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 2);
		});
	});

	describe('drive', () => {
		test('ドライブ情報を取得できる', async () => {
			const res = await api('drive', {}, alice);
			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			expect(res.body).toHaveProperty('usage', 0);
		});
	});

	describe('drive/files/create', () => {
		const assignRole = async (userId: string, policies: Record<string, unknown>) => {
			const createdRole = await role(alice, {}, policies);

			const assign = await api('admin/roles/assign', {
				userId,
				roleId: createdRole.id,
			}, alice);

			assert.strictEqual(assign.status, 204);

			return createdRole;
		};

		const cleanupRole = async (userId: string, roleId: string) => {
			await api('admin/roles/unassign', {
				userId,
				roleId,
			}, alice);

			await api('admin/roles/delete', {
				roleId,
			}, alice);
		};

		test('ファイルを作成できる', async () => {
			const res = await uploadFile(alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body!.name, '192.jpg');
		});

		test('ファイルに名前を付けられる', async () => {
			const res = await uploadFile(alice, { name: 'Belmond.jpg' });

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body!.name, 'Belmond.jpg');
		});

		test('ファイルに名前を付けられるが、拡張子は正しいものになる', async () => {
			const res = await uploadFile(alice, { name: 'Belmond.png' });

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body!.name, 'Belmond.png.jpg');
		});

		test('ファイル無しで怒られる', async () => {
			// @ts-expect-error params must not be empty
			const res = await api('drive/files/create', {}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('SVGファイルを作成できる', async () => {
			const res = await uploadFile(alice, { path: 'image.svg' });

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body!.name, 'image.svg');
			assert.strictEqual(res.body!.type, 'image/svg+xml');
		});

		for (const type of ['webp', 'avif']) {
			const mediaType = `image/${type}`;

			const getWebpublicType = async (user: misskey.entities.SignupResponse, fileId: string): Promise<string> => {
				// drive/files/create does not expose webpublicType directly, so get it by posting it
				const res = await post(user, {
					text: mediaType,
					fileIds: [fileId],
				});
				const apRes = await simpleGet(`notes/${res.id}`, 'application/activity+json');
				assert.strictEqual(apRes.status, 200);
				assert.ok(Array.isArray(apRes.body.attachment));
				return apRes.body.attachment[0].mediaType;
			};

			test(`透明な${type}ファイルを作成できる`, async () => {
				const path = `with-alpha.${type}`;
				const res = await uploadFile(alice, { path });

				assert.strictEqual(res.status, 200);
				assert.strictEqual(res.body!.name, path);
				assert.strictEqual(res.body!.type, mediaType);

				const webpublicType = await getWebpublicType(alice, res.body!.id);
				assert.strictEqual(webpublicType, 'image/webp');
			});

			test(`透明じゃない${type}ファイルを作成できる`, async () => {
				const path = `without-alpha.${type}`;
				const res = await uploadFile(alice, { path });
				assert.strictEqual(res.status, 200);
				assert.strictEqual(res.body!.name, path);
				assert.strictEqual(res.body!.type, mediaType);

				const webpublicType = await getWebpublicType(alice, res.body!.id);
				assert.strictEqual(webpublicType, 'image/webp');
			});
		}

		test('uploadableFileTypes が */* なら任意のファイルをアップロードできる', async () => {
			const createdRole = await assignRole(bob.id, {
				uploadableFileTypes: {
					useDefault: false,
					priority: 1,
					value: ['*/*'],
				},
			});

			try {
				const res = await uploadFile(bob, {
					blob: new Blob([new Uint8Array(10)]),
				});

				assert.strictEqual(res.status, 200);
			} finally {
				await cleanupRole(bob.id, createdRole.id);
			}
		});

		test('uploadableFileTypes に含まれない MIME type は拒否される', async () => {
			const createdRole = await assignRole(bob.id, {
				uploadableFileTypes: {
					useDefault: false,
					priority: 1,
					value: ['image/png'],
				},
			});

			try {
				const res = await uploadFile(bob, { path: '192.jpg' });

				assert.strictEqual(res.status, 400);
				assert.ok(res.body);
				assert.strictEqual(castAsError(res.body).error.code, 'UNALLOWED_FILE_TYPE');
			} finally {
				await cleanupRole(bob.id, createdRole.id);
			}
		});

		test('maxFileSizeMb 制限付きロールでも制限内ならアップロードできる', async () => {
			const allowAllTypesRole = await assignRole(bob.id, {
				uploadableFileTypes: {
					useDefault: false,
					priority: 1,
					value: ['*/*'],
				},
			});
			const tinyAttachmentRole = await assignRole(bob.id, {
				maxFileSizeMb: {
					useDefault: false,
					priority: 1,
					value: 10 / 1024 / 1024, // 10バイト
				},
			});

			try {
				const res = await uploadFile(bob, {
					blob: new Blob([new Uint8Array(10)]),
				});

				assert.strictEqual(res.status, 200);
			} finally {
				await cleanupRole(bob.id, tinyAttachmentRole.id);
				await cleanupRole(bob.id, allowAllTypesRole.id);
			}
		});

		test('maxFileSizeMb 制限を超えると 413 になる', async () => {
			const allowAllTypesRole = await assignRole(bob.id, {
				uploadableFileTypes: {
					useDefault: false,
					priority: 1,
					value: ['*/*'],
				},
			});
			const tinyAttachmentRole = await assignRole(bob.id, {
				maxFileSizeMb: {
					useDefault: false,
					priority: 1,
					value: 10 / 1024 / 1024, // 10バイト
				},
			});

			try {
				const res = await uploadFile(bob, {
					blob: new Blob([new Uint8Array(11)]),
				});

				assert.strictEqual(res.status, 413);
				assert.ok(res.body);
				assert.strictEqual(castAsError(res.body).error.code, 'MAX_FILE_SIZE_EXCEEDED');
			} finally {
				await cleanupRole(bob.id, tinyAttachmentRole.id);
				await cleanupRole(bob.id, allowAllTypesRole.id);
			}
		});
	});

	describe('drive/files/update', () => {
		test('名前を更新できる', async () => {
			const file = (await uploadFile(alice)).body;
			const newName = 'いちごパスタ.png';

			const res = await api('drive/files/update', {
				fileId: file!.id,
				name: newName,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.name, newName);
		});

		test('他人のファイルは更新できない', async () => {
			const file = (await uploadFile(alice)).body;

			const res = await api('drive/files/update', {
				fileId: file!.id,
				name: 'いちごパスタ.png',
			}, bob);

			assert.strictEqual(res.status, 400);
		});

		test('親フォルダを更新できる', async () => {
			const file = (await uploadFile(alice)).body;
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			const res = await api('drive/files/update', {
				fileId: file!.id,
				folderId: folder.id,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.folderId, folder.id);
		});

		test('親フォルダを無しにできる', async () => {
			const file = (await uploadFile(alice)).body;

			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			await api('drive/files/update', {
				fileId: file!.id,
				folderId: folder.id,
			}, alice);

			const res = await api('drive/files/update', {
				fileId: file!.id,
				folderId: null,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.folderId, null);
		});

		test('他人のフォルダには入れられない', async () => {
			const file = (await uploadFile(alice)).body;
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, bob)).body;

			const res = await api('drive/files/update', {
				fileId: file!.id,
				folderId: folder.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('存在しないフォルダで怒られる', async () => {
			const file = (await uploadFile(alice)).body;

			const res = await api('drive/files/update', {
				fileId: file!.id,
				folderId: '000000000000000000000000',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('不正なフォルダIDで怒られる', async () => {
			const file = (await uploadFile(alice)).body;

			const res = await api('drive/files/update', {
				fileId: file!.id,
				folderId: 'foo',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('ファイルが存在しなかったら怒る', async () => {
			const res = await api('drive/files/update', {
				fileId: '000000000000000000000000',
				name: 'いちごパスタ.png',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('不正なファイル名で怒られる', async () => {
			const file = (await uploadFile(alice)).body;
			const newName = '';

			const res = await api('drive/files/update', {
				fileId: file!.id,
				name: newName,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('間違ったIDで怒られる', async () => {
			const res = await api('drive/files/update', {
				fileId: 'kyoppie',
				name: 'いちごパスタ.png',
			}, alice);

			assert.strictEqual(res.status, 400);
		});
	});

	describe('drive/folders/create', () => {
		test('フォルダを作成できる', async () => {
			const res = await api('drive/folders/create', {
				name: 'test',
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.name, 'test');
		});
	});

	describe('drive/folders/update', () => {
		test('名前を更新できる', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				name: 'new name',
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.name, 'new name');
		});

		test('他人のフォルダを更新できない', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, bob)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				name: 'new name',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('親フォルダを更新できる', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const parentFolder = (await api('drive/folders/create', {
				name: 'parent',
			}, alice)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: parentFolder.id,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.parentId, parentFolder.id);
		});

		test('親フォルダを無しに更新できる', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const parentFolder = (await api('drive/folders/create', {
				name: 'parent',
			}, alice)).body;
			await api('drive/folders/update', {
				folderId: folder.id,
				parentId: parentFolder.id,
			}, alice);

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: null,
			}, alice);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(typeof res.body === 'object' && !Array.isArray(res.body), true);
			assert.strictEqual(res.body.parentId, null);
		});

		test('他人のフォルダを親フォルダに設定できない', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const parentFolder = (await api('drive/folders/create', {
				name: 'parent',
			}, bob)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: parentFolder.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('フォルダが循環するような構造にできない', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const parentFolder = (await api('drive/folders/create', {
				name: 'parent',
			}, alice)).body;
			await api('drive/folders/update', {
				folderId: parentFolder.id,
				parentId: folder.id,
			}, alice);

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: parentFolder.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('フォルダが循環するような構造にできない(再帰的)', async () => {
			const folderA = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const folderB = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			const folderC = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;
			await api('drive/folders/update', {
				folderId: folderB.id,
				parentId: folderA.id,
			}, alice);
			await api('drive/folders/update', {
				folderId: folderC.id,
				parentId: folderB.id,
			}, alice);

			const res = await api('drive/folders/update', {
				folderId: folderA.id,
				parentId: folderC.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('フォルダが循環するような構造にできない(自身)', async () => {
			const folderA = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			const res = await api('drive/folders/update', {
				folderId: folderA.id,
				parentId: folderA.id,
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('存在しない親フォルダを設定できない', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: '000000000000000000000000',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('不正な親フォルダIDで怒られる', async () => {
			const folder = (await api('drive/folders/create', {
				name: 'test',
			}, alice)).body;

			const res = await api('drive/folders/update', {
				folderId: folder.id,
				parentId: 'foo',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('存在しないフォルダを更新できない', async () => {
			const res = await api('drive/folders/update', {
				folderId: '000000000000000000000000',
			}, alice);

			assert.strictEqual(res.status, 400);
		});

		test('不正なフォルダIDで怒られる', async () => {
			const res = await api('drive/folders/update', {
				folderId: 'foo',
			}, alice);

			assert.strictEqual(res.status, 400);
		});
	});

	describe('notes/replies', () => {
		test('自分に閲覧権限のない投稿は含まれない', async () => {
			const alicePost = await post(alice, {
				text: 'foo',
			});

			await post(bob, {
				replyId: alicePost.id,
				text: 'bar',
				visibility: 'specified',
				visibleUserIds: [alice.id],
			});

			const res = await api('notes/replies', {
				noteId: alicePost.id,
			}, carol);

			assert.strictEqual(res.status, 200);
			assert.strictEqual(Array.isArray(res.body), true);
			assert.strictEqual(res.body.length, 0);
		});
	});

	describe('notes/timeline', () => {
		test('フォロワー限定投稿が含まれる', async () => {
			await api('following/create', {
				userId: carol.id,
			}, dave);

			const carolPost = await post(carol, {
				text: 'foo',
				visibility: 'followers',
			});

			await vi.waitFor(async () => {
				const res = await api('notes/timeline', {}, dave);

				assert.strictEqual(res.status, 200);
				assert.strictEqual(Array.isArray(res.body), true);
				assert.strictEqual(res.body.length, 1);
				assert.strictEqual(res.body[0].id, carolPost.id);
			}, waitForPushToTlOptions);
		});
	});

	describe('URL preview', () => {
		test('Error from summaly becomes HTTP 422', async () => {
			const res = await simpleGet('/url?url=https://e:xample.com');
			assert.strictEqual(res.status, 422);
			assert.strictEqual(res.body.error.code, 'URL_PREVIEW_FAILED');
		});
	});

	describe('パーソナルメモ機能のテスト', () => {
		test('他者に関するメモを更新できる', async () => {
			const memo = '10月まで低浮上とのこと。';

			const res1 = await api('users/update-memo', {
				memo,
				userId: bob.id,
			}, alice);

			const res2 = await api('users/show', {
				userId: bob.id,
			}, alice);
			assert.strictEqual(res1.status, 204);
			assert.strictEqual((res2.body as unknown as { memo: string })?.memo, memo);
		});

		test('自分に関するメモを更新できる', async () => {
			const memo = 'チケットを月末までに買う。';

			const res1 = await api('users/update-memo', {
				memo,
				userId: alice.id,
			}, alice);

			const res2 = await api('users/show', {
				userId: alice.id,
			}, alice);
			assert.strictEqual(res1.status, 204);
			assert.strictEqual((res2.body as unknown as { memo: string })?.memo, memo);
		});

		test('メモを削除できる', async () => {
			const memo = '10月まで低浮上とのこと。';

			await api('users/update-memo', {
				memo,
				userId: bob.id,
			}, alice);

			await api('users/update-memo', {
				memo: '',
				userId: bob.id,
			}, alice);

			const res = await api('users/show', {
				userId: bob.id,
			}, alice);

			// memoには常に文字列かnullが入っている(5cac151)
			assert.strictEqual((res.body as unknown as { memo: string | null }).memo, null);
		});

		test('メモは個人ごとに独立して保存される', async () => {
			const memoAliceToBob = '10月まで低浮上とのこと。';
			const memoCarolToBob = '例の件について今度問いただす。';

			await Promise.all([
				api('users/update-memo', {
					memo: memoAliceToBob,
					userId: bob.id,
				}, alice),
				api('users/update-memo', {
					memo: memoCarolToBob,
					userId: bob.id,
				}, carol),
			]);

			const [resAlice, resCarol] = await Promise.all([
				api('users/show', {
					userId: bob.id,
				}, alice),
				api('users/show', {
					userId: bob.id,
				}, carol),
			]);

			assert.strictEqual((resAlice.body as unknown as { memo: string }).memo, memoAliceToBob);
			assert.strictEqual((resCarol.body as unknown as { memo: string }).memo, memoCarolToBob);
		});
	});
});
