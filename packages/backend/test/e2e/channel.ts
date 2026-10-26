/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

process.env.NODE_ENV = 'test';

import * as assert from 'assert';
import { describe, beforeAll, test } from 'vitest';
import { api, castAsError, signup, successfulApiCall } from '../utils.js';
import type * as misskey from 'misskey-js';

describe('Channel', () => {
	let alice: misskey.entities.SignupResponse;
	beforeAll(async () => {
		alice = await signup({ username: 'alice' });
	});

	describe('Follow', () => {
		let channel: misskey.entities.ChannelsCreateResponse;

		beforeAll(async () => {
			const res = await api('channels/create', { name: 'follow-test-channel' }, alice);
			channel = res.body;
		});

		test('フォローしているチャンネルを再度フォローするとALREADY_FOLLOWINGエラーになる', async () => {
			const res1 = await api('channels/follow', { channelId: channel.id }, alice);
			assert.strictEqual(res1.status, 204);

			const res2 = await api('channels/follow', { channelId: channel.id }, alice);
			assert.strictEqual(res2.status, 400);
			assert.strictEqual(castAsError(res2.body as any).error.code, 'ALREADY_FOLLOWING');
		});
	});

	describe('My favorites', () => {
		test('returns an empty list when the viewer only owns or follows channels', async () => {
			const viewer = await signup();
			const channel = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'not-favorited' }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/follow', parameters: { channelId: channel.id }, user: viewer });

			const favorites = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: viewer });
			assert.deepStrictEqual(favorites, []);
		});

		test('selects only the actual viewer favorites, including channels shared with another viewer', async () => {
			const owner = await signup();
			const viewer = await signup();
			const otherViewer = await signup();
			const shared = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'shared-favorite' }, user: owner });
			const otherOnly = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'other-favorite' }, user: owner });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: shared.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: shared.id }, user: otherViewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: otherOnly.id }, user: otherViewer });

			const favorites = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: viewer });
			assert.deepStrictEqual(favorites.map(channel => channel.id), [shared.id]);
			assert.strictEqual(favorites[0].userId, owner.id);
			const ownerFavorites = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: owner });
			assert.deepStrictEqual(ownerFavorites, []);
		});

		test('returns each favorite once when added in reverse channel creation order', async () => {
			const viewer = await signup();
			const first = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'created-first' }, user: viewer });
			const second = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'created-second' }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: second.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: first.id }, user: viewer });

			const favorites = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: viewer });
			// The source query has no ORDER BY; favorite insertion does not establish a response order.
			assert.strictEqual(favorites.length, 2);
			assert.deepStrictEqual(new Set(favorites.map(channel => channel.id)), new Set([first.id, second.id]));
		});

		test('packs viewer flags and archived sensitive channels without detailed pinned notes', async () => {
			const owner = await signup();
			const viewer = await signup();
			const channel = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'packed-favorite' }, user: owner });
			const plain = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'plain-favorite' }, user: owner });
			const { createdNote: note } = await successfulApiCall({ endpoint: 'notes/create', parameters: { channelId: channel.id, text: 'pinned' }, user: owner });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: channel.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: plain.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/follow', parameters: { channelId: channel.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/mute/create', parameters: { channelId: channel.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/follow', parameters: { channelId: plain.id }, user: owner });
			await successfulApiCall({ endpoint: 'channels/mute/create', parameters: { channelId: plain.id }, user: owner });
			await successfulApiCall({
				endpoint: 'channels/update',
				parameters: {
					channelId: channel.id,
					pinnedNoteIds: [note.id, note.id],
					isArchived: true,
					isSensitive: true,
					allowRenoteToExternal: false,
				},
				user: owner,
			});

			const favorites = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: viewer });
			assert.strictEqual(favorites.length, 2);
			const packed = favorites.find(favorite => favorite.id === channel.id);
			assert.ok(packed);
			assert.strictEqual(packed.isFollowing, true);
			assert.strictEqual(packed.isMuting, true);
			assert.strictEqual(packed.isArchived, true);
			assert.strictEqual(packed.isSensitive, true);
			assert.strictEqual(packed.allowRenoteToExternal, false);
			assert.deepStrictEqual(packed.pinnedNoteIds, [note.id, note.id]);
			const packedPlain = favorites.find(favorite => favorite.id === plain.id);
			assert.ok(packedPlain);
			assert.strictEqual(packedPlain.isFollowing, false);
			assert.strictEqual(packedPlain.isMuting, false);
			for (const favorite of favorites) {
				assert.strictEqual(favorite.isFavorited, true);
				assert.ok('hasUnreadNote' in favorite);
				assert.strictEqual(favorite.hasUnreadNote, false);
				assert.strictEqual(Object.hasOwn(favorite, 'pinnedNotes'), false);
			}
		});

		test('ignores extra limit, date, viewer and detailed parameters', async () => {
			const viewer = await signup();
			const otherViewer = await signup();
			const first = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'unbounded-first' }, user: viewer });
			const second = await successfulApiCall({ endpoint: 'channels/create', parameters: { name: 'unbounded-second' }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: first.id }, user: viewer });
			await successfulApiCall({ endpoint: 'channels/favorite', parameters: { channelId: second.id }, user: viewer });

			const expected = await successfulApiCall({ endpoint: 'channels/my-favorites', parameters: {}, user: viewer });
			assert.strictEqual(expected.length, 2);
			const favorites = await successfulApiCall({
				endpoint: 'channels/my-favorites',
				parameters: { limit: 1, sinceDate: 0, untilDate: 1, userId: otherViewer.id, detailed: true },
				user: viewer,
			});
			assert.deepStrictEqual(favorites, expected);
		});
	});
});
