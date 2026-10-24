/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

process.env.NODE_ENV = 'test';

import * as assert from 'assert';
import { beforeAll, beforeEach, describe, test } from 'vitest';
import { inspect } from 'node:util';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { api, createAppToken, initTestDb, post, relativeFetch, role, signup, successfulApiCall, uploadFile } from '../utils.js';
import type * as misskey from 'misskey-js';
import { DEFAULT_POLICIES } from '@/core/RoleService.js';
import { MiUserProfile } from '@/models/UserProfile.js';
import { MiFollowing } from '@/models/Following.js';
import { MiFollowRequest } from '@/models/FollowRequest.js';
import { MiBlocking } from '@/models/Blocking.js';
import { MiMuting } from '@/models/Muting.js';
import { MiRenoteMuting } from '@/models/RenoteMuting.js';

describe('ユーザー', () => {
	// エンティティとしてのユーザーを主眼においたテストを記述する
	// (Userを返すエンドポイントとUserエンティティを書き換えるエンドポイントをテストする)

	const stripUndefined = <T extends { [key: string]: any } >(orig: T): Partial<T> => {
		return Object.entries({ ...orig })
			.filter(([, value]) => value !== undefined)
			.reduce((obj: Partial<T>, [key, value]) => {
				obj[key as keyof T] = value;
				return obj;
			}, {});
	};

	const show = async (id: string, me = root): Promise<misskey.entities.UserDetailed> => {
		return successfulApiCall({ endpoint: 'users/show', parameters: { userId: id }, user: me });
	};

	// UserLiteのキーが過不足なく入っている？
	const userLite = (user: misskey.entities.UserLite): Partial<misskey.entities.UserLite> => {
		return stripUndefined({
			id: user.id,
			name: user.name,
			username: user.username,
			host: user.host,
			avatarUrl: user.avatarUrl,
			avatarBlurhash: user.avatarBlurhash,
			avatarDecorations: user.avatarDecorations,
			isBot: user.isBot,
			isCat: user.isCat,
			instance: user.instance,
			emojis: user.emojis,
			onlineStatus: user.onlineStatus,
			badgeRoles: user.badgeRoles,

			// BUG isAdmin/isModeratorはUserLiteではなくMeDetailedOnlyに含まれる。
			isAdmin: undefined,
			isModerator: undefined,
		});
	};

	// UserDetailedNotMeのキーが過不足なく入っている？
	const userDetailedNotMe = (user: misskey.entities.SignupResponse): Partial<misskey.entities.UserDetailedNotMe> => {
		return stripUndefined({
			...userLite(user),
			url: user.url,
			uri: user.uri,
			movedTo: user.movedTo,
			alsoKnownAs: user.alsoKnownAs,
			createdAt: user.createdAt,
			updatedAt: user.updatedAt,
			lastFetchedAt: user.lastFetchedAt,
			bannerUrl: user.bannerUrl,
			bannerBlurhash: user.bannerBlurhash,
			isLocked: user.isLocked,
			isSilenced: user.isSilenced,
			isSuspended: user.isSuspended,
			description: user.description,
			location: user.location,
			birthday: user.birthday,
			lang: user.lang,
			fields: user.fields,
			verifiedLinks: user.verifiedLinks,
			followersCount: user.followersCount,
			followingCount: user.followingCount,
			notesCount: user.notesCount,
			pinnedNoteIds: user.pinnedNoteIds,
			pinnedNotes: user.pinnedNotes,
			pinnedPageId: user.pinnedPageId,
			pinnedPage: user.pinnedPage,
			publicReactions: user.publicReactions,
			followingVisibility: user.followingVisibility,
			followersVisibility: user.followersVisibility,
			chatScope: user.chatScope,
			canChat: user.canChat,
			roles: user.roles,
			memo: user.memo,
		});
	};

	// Relations関連のキーが過不足なく入っている？
	const userDetailedNotMeWithRelations = (user: misskey.entities.SignupResponse): Partial<misskey.entities.UserDetailedNotMe> => {
		return stripUndefined({
			...userDetailedNotMe(user),
			isFollowing: user.isFollowing ?? false,
			isFollowed: user.isFollowed ?? false,
			hasPendingFollowRequestFromYou: user.hasPendingFollowRequestFromYou ?? false,
			hasPendingFollowRequestToYou: user.hasPendingFollowRequestToYou ?? false,
			isBlocking: user.isBlocking ?? false,
			isBlocked: user.isBlocked ?? false,
			isMuted: user.isMuted ?? false,
			isRenoteMuted: user.isRenoteMuted ?? false,
			notify: user.notify ?? 'none',
			withReplies: user.withReplies ?? false,
			followedMessage: user.isFollowing ? (user.followedMessage ?? null) : undefined,
		});
	};

	// MeDetailedのキーが過不足なく入っている？
	const meDetailed = (user: misskey.entities.SignupResponse, security = false): Partial<misskey.entities.MeDetailed> => {
		return stripUndefined({
			...userDetailedNotMe(user),
			avatarId: user.avatarId,
			bannerId: user.bannerId,
			followedMessage: user.followedMessage,
			isModerator: user.isModerator,
			isAdmin: user.isAdmin,
			injectFeaturedNote: user.injectFeaturedNote,
			receiveAnnouncementEmail: user.receiveAnnouncementEmail,
			alwaysMarkNsfw: user.alwaysMarkNsfw,
			autoSensitive: user.autoSensitive,
			carefulBot: user.carefulBot,
			autoAcceptFollowed: user.autoAcceptFollowed,
			noCrawle: user.noCrawle,
			preventAiLearning: user.preventAiLearning,
			isExplorable: user.isExplorable,
			isDeleted: user.isDeleted,
			twoFactorBackupCodesStock: user.twoFactorBackupCodesStock,
			hideOnlineStatus: user.hideOnlineStatus,
			hasUnreadSpecifiedNotes: user.hasUnreadSpecifiedNotes,
			hasUnreadMentions: user.hasUnreadMentions,
			hasUnreadAnnouncement: user.hasUnreadAnnouncement,
			hasUnreadAntenna: user.hasUnreadAntenna,
			hasUnreadChannel: user.hasUnreadChannel,
			hasUnreadChatMessages: user.hasUnreadChatMessages,
			hasUnreadNotification: user.hasUnreadNotification,
			unreadNotificationsCount: user.unreadNotificationsCount,
			hasPendingReceivedFollowRequest: user.hasPendingReceivedFollowRequest,
			unreadAnnouncements: user.unreadAnnouncements,
			mutedWords: user.mutedWords,
			hardMutedWords: user.hardMutedWords,
			mutedInstances: user.mutedInstances,
			// @ts-expect-error 後方互換性
			mutingNotificationTypes: user.mutingNotificationTypes,
			notificationRecieveConfig: user.notificationRecieveConfig,
			emailNotificationTypes: user.emailNotificationTypes,
			achievements: user.achievements,
			loggedInDays: user.loggedInDays,
			policies: user.policies,
			twoFactorEnabled: user.twoFactorEnabled,
			usePasswordLessLogin: user.usePasswordLessLogin,
			securityKeys: user.securityKeys,
			...(security ? {
				email: user.email,
				emailVerified: user.emailVerified,
				securityKeysList: user.securityKeysList,
			} : {}),
		});
	};

	let root: misskey.entities.SignupResponse;
	let alice: misskey.entities.SignupResponse;
	let aliceNote: misskey.entities.Note;

	let bob: misskey.entities.SignupResponse;

	// NOTE: これがないと落ちる（bob の updatedAt が null になってしまうため？）
	let bobNote: misskey.entities.Note; // eslint-disable-line @typescript-eslint/no-unused-vars

	let carol: misskey.entities.SignupResponse;

	let usersReplying: misskey.entities.SignupResponse[];

	let userNoNote: misskey.entities.SignupResponse;
	let userNotExplorable: misskey.entities.SignupResponse;
	let userLocking: misskey.entities.SignupResponse;
	let userAdmin: misskey.entities.SignupResponse;
	let roleAdmin: misskey.entities.Role;
	let userModerator: misskey.entities.SignupResponse;
	let roleModerator: misskey.entities.Role;
	let userRolePublic: misskey.entities.SignupResponse;
	let rolePublic: misskey.entities.Role;
	let userRoleBadge: misskey.entities.SignupResponse;
	let roleBadge: misskey.entities.Role;
	let userSilenced: misskey.entities.SignupResponse;
	let roleSilenced: misskey.entities.Role;
	let userSuspended: misskey.entities.SignupResponse;
	let userDeletedBySelf: misskey.entities.SignupResponse;
	let userDeletedByAdmin: misskey.entities.SignupResponse;
	let userFollowingAlice: misskey.entities.SignupResponse;
	let userFollowedByAlice: misskey.entities.SignupResponse;
	let userBlockingAlice: misskey.entities.SignupResponse;
	let userBlockedByAlice: misskey.entities.SignupResponse;
	let userMutingAlice: misskey.entities.SignupResponse;
	let userMutedByAlice: misskey.entities.SignupResponse;
	let userRnMutingAlice: misskey.entities.SignupResponse;
	let userRnMutedByAlice: misskey.entities.SignupResponse;
	let userFollowRequesting: misskey.entities.SignupResponse;
	let userFollowRequested: misskey.entities.SignupResponse;

	beforeAll(async () => {
		root = await signup({ username: 'root' });
		alice = await signup({ username: 'alice' });
		aliceNote = await post(alice, { text: 'test' });
		bob = await signup({ username: 'bob' });
		bobNote = await post(bob, { text: 'test' });
		carol = await signup({ username: 'carol' });

		// @alice -> @replyingへのリプライ。Promise.allで一気に作るとtimeoutしてしまうのでreduceで一つ一つawaitする
		usersReplying = await [...Array(10)].map((_, i) => i).reduce(async (acc, i) => {
			const u = await signup({ username: `replying${i}` });
			for (let j = 0; j < 10 - i; j++) {
				const p = await post(u, { text: `test${j}` });
				await post(alice, { text: `@${u.username} test${j}`, replyId: p.id });
			}

			return (await acc).concat(u);
		}, Promise.resolve([] as misskey.entities.SignupResponse[]));

		userNoNote = await signup({ username: 'userNoNote' });
		userNotExplorable = await signup({ username: 'userNotExplorable' });
		await post(userNotExplorable, { text: 'test' });
		await api('i/update', { isExplorable: false }, userNotExplorable);
		userLocking = await signup({ username: 'userLocking' });
		await post(userLocking, { text: 'test' });
		await api('i/update', { isLocked: true }, userLocking);
		userAdmin = await signup({ username: 'userAdmin' });
		roleAdmin = await role(root, { isAdministrator: true, name: 'Admin Role' });
		await api('admin/roles/assign', { userId: userAdmin.id, roleId: roleAdmin.id }, root);
		userModerator = await signup({ username: 'userModerator' });
		roleModerator = await role(root, { isModerator: true, name: 'Moderator Role' });
		await api('admin/roles/assign', { userId: userModerator.id, roleId: roleModerator.id }, root);
		userRolePublic = await signup({ username: 'userRolePublic' });
		rolePublic = await role(root, { isPublic: true, name: 'Public Role' });
		await api('admin/roles/assign', { userId: userRolePublic.id, roleId: rolePublic.id }, root);
		userRoleBadge = await signup({ username: 'userRoleBadge' });
		roleBadge = await role(root, { asBadge: true, name: 'Badge Role', isPublic: true });
		await api('admin/roles/assign', { userId: userRoleBadge.id, roleId: roleBadge.id }, root);
		userSilenced = await signup({ username: 'userSilenced' });
		await post(userSilenced, { text: 'test' });
		roleSilenced = await role(root, {}, { canPublicNote: { priority: 0, useDefault: false, value: false } });
		await api('admin/roles/assign', { userId: userSilenced.id, roleId: roleSilenced.id }, root);
		userSuspended = await signup({ username: 'userSuspended' });
		await post(userSuspended, { text: 'test' });
		await successfulApiCall({ endpoint: 'i/update', parameters: { description: '#user_testuserSuspended' }, user: userSuspended });
		await api('admin/suspend-user', { userId: userSuspended.id }, root);
		userDeletedBySelf = await signup({ username: 'userDeletedBySelf', password: 'userDeletedBySelf' });
		await post(userDeletedBySelf, { text: 'test' });
		await api('i/delete-account', { password: 'userDeletedBySelf' }, userDeletedBySelf);
		userDeletedByAdmin = await signup({ username: 'userDeletedByAdmin' });
		await post(userDeletedByAdmin, { text: 'test' });
		await api('admin/delete-account', { userId: userDeletedByAdmin.id }, root);
		userFollowingAlice = await signup({ username: 'userFollowingAlice' });
		await post(userFollowingAlice, { text: 'test' });
		await api('following/create', { userId: alice.id }, userFollowingAlice);
		userFollowedByAlice = await signup({ username: 'userFollowedByAlice' });
		await post(userFollowedByAlice, { text: 'test' });
		await api('following/create', { userId: userFollowedByAlice.id }, alice);
		userBlockingAlice = await signup({ username: 'userBlockingAlice' });
		await post(userBlockingAlice, { text: 'test' });
		await api('blocking/create', { userId: alice.id }, userBlockingAlice);
		userBlockedByAlice = await signup({ username: 'userBlockedByAlice' });
		await post(userBlockedByAlice, { text: 'test' });
		await api('blocking/create', { userId: userBlockedByAlice.id }, alice);
		userMutingAlice = await signup({ username: 'userMutingAlice' });
		await post(userMutingAlice, { text: 'test' });
		await api('mute/create', { userId: alice.id }, userMutingAlice);
		userMutedByAlice = await signup({ username: 'userMutedByAlice' });
		await post(userMutedByAlice, { text: 'test' });
		await api('mute/create', { userId: userMutedByAlice.id }, alice);
		userRnMutingAlice = await signup({ username: 'userRnMutingAlice' });
		await post(userRnMutingAlice, { text: 'test' });
		await api('renote-mute/create', { userId: alice.id }, userRnMutingAlice);
		userRnMutedByAlice = await signup({ username: 'userRnMutedByAlice' });
		await post(userRnMutedByAlice, { text: 'test' });
		await api('renote-mute/create', { userId: userRnMutedByAlice.id }, alice);
		userFollowRequesting = await signup({ username: 'userFollowRequesting' });
		await post(userFollowRequesting, { text: 'test' });
		userFollowRequested = userLocking;
		await api('following/create', { userId: userFollowRequested.id }, userFollowRequesting);
	}, 1000 * 60 * 10);

	beforeEach(async () => {
		alice = {
			...alice,
			...await successfulApiCall({ endpoint: 'i', parameters: {}, user: alice }),
		};
		aliceNote = await successfulApiCall({ endpoint: 'notes/show', parameters: { noteId: aliceNote.id }, user: alice });
	});

	//#region サインアップ(signup)

	test('が作れる。（作りたての状態で自分のユーザー情報が取れる）', async () => {
		// SignupApiService.ts
		const response = await successfulApiCall({
			endpoint: 'signup',
			parameters: { username: 'zoe', password: 'password' },
			user: undefined,
		}) as unknown as misskey.entities.SignupResponse; // BUG MeDetailedに足りないキーがある

		// signupの時はtokenが含まれる特別なMeDetailedが返ってくる
		assert.match(response.token, /[a-zA-Z0-9]{16}/);

		// UserLite
		assert.match(response.id, /[0-9a-z]{10}/);
		assert.strictEqual(response.name, null);
		assert.strictEqual(response.username, 'zoe');
		assert.strictEqual(response.host, null);
		response.avatarUrl && assert.match(response.avatarUrl, /^[-a-zA-Z0-9@:%._\+~#&?=\/]+$/);
		assert.strictEqual(response.avatarBlurhash, null);
		assert.deepStrictEqual(response.avatarDecorations, []);
		assert.strictEqual(response.isBot, false);
		assert.strictEqual(response.isCat, false);
		assert.strictEqual(response.instance, undefined);
		assert.deepStrictEqual(response.emojis, {});
		assert.strictEqual(response.onlineStatus, 'unknown');
		assert.deepStrictEqual(response.badgeRoles, []);
		// UserDetailedNotMeOnly
		assert.strictEqual(response.url, null);
		assert.strictEqual(response.uri, null);
		assert.strictEqual(response.movedTo, null);
		assert.strictEqual(response.alsoKnownAs, null);
		assert.strictEqual(response.createdAt, new Date(response.createdAt).toISOString());
		assert.strictEqual(response.updatedAt, null);
		assert.strictEqual(response.lastFetchedAt, null);
		assert.strictEqual(response.bannerUrl, null);
		assert.strictEqual(response.bannerBlurhash, null);
		assert.strictEqual(response.isLocked, false);
		assert.strictEqual(response.isSilenced, false);
		assert.strictEqual(response.isSuspended, false);
		assert.strictEqual(response.description, null);
		assert.strictEqual(response.location, null);
		assert.strictEqual(response.birthday, null);
		assert.strictEqual(response.lang, null);
		assert.deepStrictEqual(response.fields, []);
		assert.deepStrictEqual(response.verifiedLinks, []);
		assert.strictEqual(response.followersCount, 0);
		assert.strictEqual(response.followingCount, 0);
		assert.strictEqual(response.notesCount, 0);
		assert.deepStrictEqual(response.pinnedNoteIds, []);
		assert.deepStrictEqual(response.pinnedNotes, []);
		assert.strictEqual(response.pinnedPageId, null);
		assert.strictEqual(response.pinnedPage, null);
		assert.strictEqual(response.publicReactions, true);
		assert.strictEqual(response.followingVisibility, 'public');
		assert.strictEqual(response.followersVisibility, 'public');
		assert.strictEqual(response.chatScope, 'mutual');
		assert.strictEqual(response.canChat, true);
		assert.deepStrictEqual(response.roles, []);
		assert.strictEqual(response.memo, null);

		// MeDetailedOnly
		assert.strictEqual(response.avatarId, null);
		assert.strictEqual(response.bannerId, null);
		assert.strictEqual(response.followedMessage, null);
		assert.strictEqual(response.isModerator, false);
		assert.strictEqual(response.isAdmin, false);
		assert.strictEqual(response.injectFeaturedNote, true);
		assert.strictEqual(response.receiveAnnouncementEmail, true);
		assert.strictEqual(response.alwaysMarkNsfw, false);
		assert.strictEqual(response.autoSensitive, false);
		assert.strictEqual(response.carefulBot, false);
		assert.strictEqual(response.autoAcceptFollowed, true);
		assert.strictEqual(response.noCrawle, false);
		assert.strictEqual(response.preventAiLearning, true);
		assert.strictEqual(response.isExplorable, true);
		assert.strictEqual(response.isDeleted, false);
		assert.strictEqual(response.twoFactorBackupCodesStock, 'none');
		assert.strictEqual(response.hideOnlineStatus, false);
		assert.strictEqual(response.hasUnreadSpecifiedNotes, false);
		assert.strictEqual(response.hasUnreadMentions, false);
		assert.strictEqual(response.hasUnreadAnnouncement, false);
		assert.strictEqual(response.hasUnreadAntenna, false);
		assert.strictEqual(response.hasUnreadChannel, false);
		assert.strictEqual(response.hasUnreadChatMessages, false);
		assert.strictEqual(response.hasUnreadNotification, false);
		assert.strictEqual(response.unreadNotificationsCount, 0);
		assert.strictEqual(response.hasPendingReceivedFollowRequest, false);
		assert.deepStrictEqual(response.unreadAnnouncements, []);
		assert.deepStrictEqual(response.mutedWords, []);
		assert.deepStrictEqual(response.mutedInstances, []);
		// @ts-expect-error 後方互換のため
		assert.deepStrictEqual(response.mutingNotificationTypes, []);
		assert.deepStrictEqual(response.notificationRecieveConfig, {});
		assert.deepStrictEqual(response.emailNotificationTypes, ['follow', 'receiveFollowRequest']);
		assert.deepStrictEqual(response.achievements, []);
		assert.deepStrictEqual(response.loggedInDays, 0);
		assert.deepStrictEqual(response.policies, DEFAULT_POLICIES);
		assert.strictEqual(response.twoFactorEnabled, false);
		assert.strictEqual(response.usePasswordLessLogin, false);
		assert.strictEqual(response.securityKeys, false);
		assert.notStrictEqual(response.email, undefined);
		assert.strictEqual(response.emailVerified, false);
		assert.deepStrictEqual(response.securityKeysList, []);
	});

	//#endregion
	//#region 自分の情報(i)

	test('を読み取ることができること（自分）、キーが過不足なく入っていること。', async () => {
		const response = await successfulApiCall({
			endpoint: 'i',
			parameters: {},
			user: userNoNote,
		});
		const expected = meDetailed(userNoNote, true);
		expected.loggedInDays = 1; // iはloggedInDaysを更新する
		assert.deepStrictEqual(response, expected);
	});

	//#endregion
	//#region 自分の情報の更新(i/update)

	test.each([
		{ parameters: () => ({ name: null }) },
		{ parameters: () => ({ name: 'x'.repeat(50) }) },
		{ parameters: () => ({ name: 'x' }) },
		{ parameters: () => ({ name: 'My name' }) },
		{ parameters: () => ({ description: null }) },
		{ parameters: () => ({ description: 'x'.repeat(1500) }) },
		{ parameters: () => ({ description: 'x' }) },
		{ parameters: () => ({ description: 'My description' }) },
		{ parameters: () => ({ followedMessage: null }) },
		{ parameters: () => ({ followedMessage: 'Thank you' }) },
		{ parameters: () => ({ location: null }) },
		{ parameters: () => ({ location: 'x'.repeat(50) }) },
		{ parameters: () => ({ location: 'x' }) },
		{ parameters: () => ({ location: 'My location' }) },
		{ parameters: () => ({ birthday: '0000-00-00' }) },
		{ parameters: () => ({ birthday: '9999-99-99' }) },
		{ parameters: () => ({ lang: 'en-US' as const }) },
		{ parameters: () => ({ fields: [] }) },
		{ parameters: () => ({ fields: [{ name: 'x', value: 'x' }] }) },
		{ parameters: () => ({ fields: [{ name: 'x'.repeat(3000), value: 'x'.repeat(3000) }] }) }, // BUG? fieldには制限がない
		{ parameters: () => ({ fields: Array(16).fill({ name: 'x', value: 'y' }) }) },
		{ parameters: () => ({ isLocked: true }) },
		{ parameters: () => ({ isLocked: false }) },
		{ parameters: () => ({ isExplorable: false }) },
		{ parameters: () => ({ isExplorable: true }) },
		{ parameters: () => ({ hideOnlineStatus: true }) },
		{ parameters: () => ({ hideOnlineStatus: false }) },
		{ parameters: () => ({ publicReactions: false }) },
		{ parameters: () => ({ publicReactions: true }) },
		{ parameters: () => ({ autoAcceptFollowed: true }) },
		{ parameters: () => ({ autoAcceptFollowed: false }) },
		{ parameters: () => ({ noCrawle: true }) },
		{ parameters: () => ({ noCrawle: false }) },
		{ parameters: () => ({ preventAiLearning: false }) },
		{ parameters: () => ({ preventAiLearning: true }) },
		{ parameters: () => ({ isBot: true }) },
		{ parameters: () => ({ isBot: false }) },
		{ parameters: () => ({ isCat: true }) },
		{ parameters: () => ({ isCat: false }) },
		{ parameters: () => ({ injectFeaturedNote: true }) },
		{ parameters: () => ({ injectFeaturedNote: false }) },
		{ parameters: () => ({ receiveAnnouncementEmail: true }) },
		{ parameters: () => ({ receiveAnnouncementEmail: false }) },
		{ parameters: () => ({ alwaysMarkNsfw: true }) },
		{ parameters: () => ({ alwaysMarkNsfw: false }) },
		{ parameters: () => ({ autoSensitive: true }) },
		{ parameters: () => ({ autoSensitive: false }) },
		{ parameters: () => ({ followingVisibility: 'private' as const }) },
		{ parameters: () => ({ followingVisibility: 'followers' as const }) },
		{ parameters: () => ({ followingVisibility: 'public' as const }) },
		{ parameters: () => ({ followersVisibility: 'private' as const }) },
		{ parameters: () => ({ followersVisibility: 'followers' as const }) },
		{ parameters: () => ({ followersVisibility: 'public' as const }) },
		{ parameters: () => ({ mutedWords: Array(19).fill(['xxxxx']) }) },
		{ parameters: () => ({ mutedWords: [['x'.repeat(194)]] }) },
		{ parameters: () => ({ mutedWords: [] }) },
		{ parameters: () => ({ mutedInstances: ['xxxx.xxxxx'] }) },
		{ parameters: () => ({ mutedInstances: [] }) },
		{ parameters: () => ({ notificationRecieveConfig: { mention: { type: 'following' } } }) },
		{ parameters: () => ({ notificationRecieveConfig: {} }) },
		{ parameters: () => ({ emailNotificationTypes: ['mention', 'reply', 'quote', 'follow', 'receiveFollowRequest'] }) },
		{ parameters: () => ({ emailNotificationTypes: [] }) },
	] as const)('を書き換えることができる($#)', async ({ parameters }) => {
		const response = await successfulApiCall({ endpoint: 'i/update', parameters: parameters(), user: alice });
		const expected = { ...meDetailed(alice, true), ...parameters() };
		assert.deepStrictEqual(response, expected, inspect(parameters()));
	});

	test('を書き換えることができる(Avatar)', async () => {
		const aliceFile = (await uploadFile(alice)).body;
		const parameters = { avatarId: aliceFile!.id };
		const response = await successfulApiCall({ endpoint: 'i/update', parameters: parameters, user: alice });
		assert.match(response.avatarUrl ?? '.', /^[-a-zA-Z0-9@:%._\+~#&?=\/]+$/);
		assert.match(response.avatarBlurhash ?? '.', /[ -~]{54}/);
		const expected = {
			...meDetailed(alice, true),
			avatarId: aliceFile!.id,
			avatarBlurhash: response.avatarBlurhash,
			avatarUrl: response.avatarUrl,
		};
		assert.deepStrictEqual(response, expected, inspect(parameters));

		const parameters2 = { avatarId: null };
		const response2 = await successfulApiCall({ endpoint: 'i/update', parameters: parameters2, user: alice });
		const expected2 = {
			...meDetailed(alice, true),
			avatarId: null,
			avatarBlurhash: null,
			avatarUrl: alice.avatarUrl, // 解除した場合、identiconになる
		};
		assert.deepStrictEqual(response2, expected2, inspect(parameters));
	});

	test('を書き換えることができる(Banner)', async () => {
		const aliceFile = (await uploadFile(alice)).body;
		const parameters = { bannerId: aliceFile!.id };
		const response = await successfulApiCall({ endpoint: 'i/update', parameters: parameters, user: alice });
		assert.match(response.bannerUrl ?? '.', /^[-a-zA-Z0-9@:%._\+~#&?=\/]+$/);
		assert.match(response.bannerBlurhash ?? '.', /[ -~]{54}/);
		const expected = {
			...meDetailed(alice, true),
			bannerId: aliceFile!.id,
			bannerBlurhash: response.bannerBlurhash,
			bannerUrl: response.bannerUrl,
		};
		assert.deepStrictEqual(response, expected, inspect(parameters));

		const parameters2 = { bannerId: null };
		const response2 = await successfulApiCall({ endpoint: 'i/update', parameters: parameters2, user: alice });
		const expected2 = {
			...meDetailed(alice, true),
			bannerId: null,
			bannerBlurhash: null,
			bannerUrl: null,
		};
		assert.deepStrictEqual(response2, expected2, inspect(parameters));
	});

	//#endregion
	//#region 自分の情報の更新(i/pin, i/unpin)

	test('を書き換えることができる(ピン止めノート)', async () => {
		const parameters = { noteId: aliceNote.id };
		const response = await successfulApiCall({ endpoint: 'i/pin', parameters, user: alice });
		const expected = { ...meDetailed(alice, false), pinnedNoteIds: [aliceNote.id], pinnedNotes: [aliceNote] };
		assert.deepStrictEqual(response, expected);

		const response2 = await successfulApiCall({ endpoint: 'i/unpin', parameters, user: alice });
		const expected2 = meDetailed(alice, false);
		assert.deepStrictEqual(response2, expected2);
	});

	//#endregion
	//#region メモの更新(users/update-memo)

	test.each([
		{ label: '最大長', memo: 'x'.repeat(2048) },
		{ label: '空文字', memo: '', expects: null },
		{ label: 'null', memo: null },
	])('を書き換えることができる(メモを$labelに)', async ({ memo, expects }) => {
		const expected = { ...await show(bob.id, alice), memo: expects === undefined ? memo : expects };
		const parameters = { userId: bob.id, memo };
		await successfulApiCall({ endpoint: 'users/update-memo', parameters, user: alice });
		const response = await show(bob.id, alice);
		assert.deepStrictEqual(response, expected);
	});

	//#endregion
	//#region ユーザー(users)

	test.each([
		{ label: 'ID昇順', parameters: { limit: 5 }, selector: (u: misskey.entities.UserLite): string => u.id },
		{ label: 'フォロワー昇順', parameters: { sort: '+follower' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.followersCount) },
		{ label: 'フォロワー降順', parameters: { sort: '-follower' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.followersCount) },
		{ label: '登録日時昇順', parameters: { sort: '+createdAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => u.createdAt },
		{ label: '登録日時降順', parameters: { sort: '-createdAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => u.createdAt },
		{ label: '投稿日時昇順', parameters: { sort: '+updatedAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.updatedAt) },
		{ label: '投稿日時降順', parameters: { sort: '-updatedAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.updatedAt) },
	] as const)('をリスト形式で取得することができる（$label）', async ({ parameters, selector }) => {
		const response = await successfulApiCall({ endpoint: 'users', parameters, user: alice });

		// 結果の並びを事前にアサートするのは困難なので返ってきたidに対応するユーザーが返っており、ソート順が正しいことだけを検証する
		const users = await Promise.all(response.map(u => show(u.id, alice)));
		const expected = users.sort((x, y) => {
			const index = (selector(x) < selector(y)) ? -1 : (selector(x) > selector(y)) ? 1 : 0;
			return index * (parameters.sort?.startsWith('+') ? -1 : 1);
		});
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれない', user: () => userNotExplorable, excluded: true },
		{ label: 'ミュートユーザーが含まれない', user: () => userMutedByAlice, excluded: true },
		{ label: 'ブロックされているユーザーが含まれない', user: () => userBlockedByAlice, excluded: true },
		{ label: 'ブロックしてきているユーザーが含まれる', user: () => userBlockingAlice, excluded: true },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		{ label: 'サスペンドユーザーが含まれない', user: () => userSuspended, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
	] as const)('をリスト形式で取得することができ、結果に$label', async ({ user, excluded }) => {
		const parameters = { limit: 100 };
		const response = await successfulApiCall({ endpoint: 'users', parameters, user: alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, alice)];
		assert.deepStrictEqual(response.filter((u) => u.id === user().id), expected);
	});
	test.todo('をリスト形式で取得することができる（リモート, hostname指定）');
	test.todo('をリスト形式で取得することができる（pagenation）');

	//#endregion
	//#region ユーザー情報(users/show)

	test.each([
		{ label: 'ID指定で自分自身を', parameters: () => ({ userId: alice.id }), user: () => alice, type: meDetailed },
		{ label: 'ID指定で他人を', parameters: () => ({ userId: alice.id }), user: () => bob, type: userDetailedNotMeWithRelations },
		{ label: 'ID指定かつ未認証', parameters: () => ({ userId: alice.id }), user: undefined, type: userDetailedNotMe },
		{ label: '@指定で自分自身を', parameters: () => ({ username: alice.username }), user: () => alice, type: meDetailed },
		{ label: '@指定で他人を', parameters: () => ({ username: alice.username }), user: () => bob, type: userDetailedNotMeWithRelations },
		{ label: '@指定かつ未認証', parameters: () => ({ username: alice.username }), user: undefined, type: userDetailedNotMe },
	] as const)('を取得することができる（$label）', async ({ parameters, user, type }) => {
		const response = await successfulApiCall({ endpoint: 'users/show', parameters: parameters(), user: user?.() });
		const expected = type(alice);
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: 'Administratorになっている', user: () => userAdmin, me: () => userAdmin, selector: (user: misskey.entities.MeDetailed) => user.isAdmin },
		// @ts-expect-error UserDetailedNotMe doesn't include isAdmin
		{ label: '自分以外から見たときはAdministratorか判定できない', user: () => userAdmin, selector: (user: misskey.entities.UserDetailedNotMe) => user.isAdmin, expected: () => undefined },
		{ label: 'Moderatorになっている', user: () => userModerator, me: () => userModerator, selector: (user: misskey.entities.MeDetailed) => user.isModerator },
		// @ts-expect-error UserDetailedNotMe doesn't include isModerator
		{ label: '自分以外から見たときはModeratorか判定できない', user: () => userModerator, selector: (user: misskey.entities.UserDetailedNotMe) => user.isModerator, expected: () => undefined },
		{ label: '自分から見た場合に二要素認証関連のプロパティがセットされている', user: () => alice, me: () => alice, selector: (user: misskey.entities.MeDetailed) => user.twoFactorEnabled, expected: () => false },
		{ label: '自分以外から見た場合に二要素認証関連のプロパティがセットされていない', user: () => alice, me: () => bob, selector: (user: misskey.entities.UserDetailedNotMe) => user.twoFactorEnabled, expected: () => undefined },
		{ label: 'モデレーターから見た場合に二要素認証関連のプロパティがセットされている', user: () => alice, me: () => userModerator, selector: (user: misskey.entities.UserDetailedNotMe) => user.twoFactorEnabled, expected: () => false },
		{ label: 'サイレンスになっている', user: () => userSilenced, selector: (user: misskey.entities.UserDetailed) => user.isSilenced },
		// FIXME: 落ちる
		//{ label: 'サスペンドになっている', user: () => userSuspended, selector: (user: misskey.entities.UserDetailed) => user.isSuspended },
		{ label: '削除済みになっている', user: () => userDeletedBySelf, me: () => userDeletedBySelf, selector: (user: misskey.entities.MeDetailed) => user.isDeleted },
		// @ts-expect-error UserDetailedNotMe doesn't include isDeleted
		{ label: '自分以外から見たときは削除済みか判定できない', user: () => userDeletedBySelf, selector: (user: misskey.entities.UserDetailedNotMe) => user.isDeleted, expected: () => undefined },
		{ label: '削除済み(byAdmin)になっている', user: () => userDeletedByAdmin, me: () => userDeletedByAdmin, selector: (user: misskey.entities.MeDetailed) => user.isDeleted },
		// @ts-expect-error UserDetailedNotMe doesn't include isDeleted
		{ label: '自分以外から見たときは削除済み(byAdmin)か判定できない', user: () => userDeletedByAdmin, selector: (user: misskey.entities.UserDetailedNotMe) => user.isDeleted, expected: () => undefined },
		{ label: 'フォロー中になっている', user: () => userFollowedByAlice, selector: (user: misskey.entities.UserDetailed) => user.isFollowing },
		{ label: 'フォローされている', user: () => userFollowingAlice, selector: (user: misskey.entities.UserDetailed) => user.isFollowed },
		{ label: 'ブロック中になっている', user: () => userBlockedByAlice, selector: (user: misskey.entities.UserDetailed) => user.isBlocking },
		{ label: 'ブロックされている', user: () => userBlockingAlice, selector: (user: misskey.entities.UserDetailed) => user.isBlocked },
		{ label: 'ミュート中になっている', user: () => userMutedByAlice, selector: (user: misskey.entities.UserDetailed) => user.isMuted },
		{ label: 'リノートミュート中になっている', user: () => userRnMutedByAlice, selector: (user: misskey.entities.UserDetailed) => user.isRenoteMuted },
		{ label: 'フォローリクエスト中になっている', user: () => userFollowRequested, me: () => userFollowRequesting, selector: (user: misskey.entities.UserDetailed) => user.hasPendingFollowRequestFromYou },
		{ label: 'フォローリクエストされている', user: () => userFollowRequesting, me: () => userFollowRequested, selector: (user: misskey.entities.UserDetailed) => user.hasPendingFollowRequestToYou },
	] as const)('を取得することができ、$labelこと', async ({ user, me, selector, expected }) => {
		const response = await successfulApiCall({ endpoint: 'users/show', parameters: { userId: user().id }, user: me?.() ?? alice });
		assert.strictEqual(selector(response as any), (expected ?? ((): true => true))());
	});
	test('を取得することができ、Publicなロールがセットされていること', async () => {
		const response = await successfulApiCall({ endpoint: 'users/show', parameters: { userId: userRolePublic.id }, user: alice });
		assert.deepStrictEqual(response.badgeRoles, []);
		assert.deepStrictEqual(response.roles, [{
			id: rolePublic.id,
			name: rolePublic.name,
			color: rolePublic.color,
			iconUrl: rolePublic.iconUrl,
			description: rolePublic.description,
			isModerator: rolePublic.isModerator,
			isAdministrator: rolePublic.isAdministrator,
			displayOrder: rolePublic.displayOrder,
		}]);
	});
	test('を取得することができ、バッヂロールがセットされていること', async () => {
		const response = await successfulApiCall({ endpoint: 'users/show', parameters: { userId: userRoleBadge.id }, user: alice });
		assert.deepStrictEqual(response.badgeRoles, [{
			name: roleBadge.name,
			iconUrl: roleBadge.iconUrl,
			displayOrder: roleBadge.displayOrder,
		}]);
		assert.deepStrictEqual(response.roles, [{
			id: roleBadge.id,
			name: roleBadge.name,
			color: roleBadge.color,
			iconUrl: roleBadge.iconUrl,
			description: roleBadge.description,
			isModerator: roleBadge.isModerator,
			isAdministrator: roleBadge.isAdministrator,
			displayOrder: roleBadge.displayOrder,
		}]);
	});
	test('をID指定のリスト形式で取得することができる（空）', async () => {
		const parameters = { userIds: [] };
		const response = await successfulApiCall({ endpoint: 'users/show', parameters, user: alice });
		const expected: [] = [];
		assert.deepStrictEqual(response, expected);
	});
	test('をID指定のリスト形式で取得することができる', async() => {
		const parameters = { userIds: [bob.id, alice.id, carol.id] };
		const response = await successfulApiCall({ endpoint: 'users/show', parameters, user: alice });
		const expected = [
			await successfulApiCall({ endpoint: 'users/show', parameters: { userId: bob.id }, user: alice }),
			await successfulApiCall({ endpoint: 'users/show', parameters: { userId: alice.id }, user: alice }),
			await successfulApiCall({ endpoint: 'users/show', parameters: { userId: carol.id }, user: alice }),
		];
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれる', user: () => userNotExplorable },
		{ label: 'ミュートユーザーが含まれる', user: () => userMutedByAlice },
		{ label: 'ブロックされているユーザーが含まれる', user: () => userBlockedByAlice },
		{ label: 'ブロックしてきているユーザーが含まれる', user: () => userBlockingAlice },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		{ label: 'サスペンドユーザーが（モデレーターが見るときは）含まれる', user: () => userSuspended, me: () => root },
		// BUG サスペンドユーザーを一般ユーザーから見るとrootユーザーが返ってくる
		//{ label: 'サスペンドユーザーが（一般ユーザーが見るときは）含まれない', user: () => userSuspended, me: () => bob, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
		// @ts-expect-error excluded は上でコメントアウトされているので
	] as const)('をID指定のリスト形式で取得することができ、結果に$label', async ({ user, me, excluded }) => {
		const parameters = { userIds: [user().id] };
		const response = await successfulApiCall({ endpoint: 'users/show', parameters, user: me?.() ?? alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, me?.() ?? alice)];
		assert.deepStrictEqual(response, expected);
	});
	test.todo('をID指定のリスト形式で取得することができる(リモート)');

	//#endregion
	//#region 検索(users/search)

	test('を検索することができる', async () => {
		const parameters = { query: 'carol', limit: 10 };
		const response = await successfulApiCall({ endpoint: 'users/search', parameters, user: alice });
		const expected = [await show(carol.id, alice)];
		assert.deepStrictEqual(response, expected);
	});
	test('を検索することができる(UserLite)', async () => {
		const parameters = { query: 'carol', detail: false, limit: 10 };
		const response = await successfulApiCall({ endpoint: 'users/search', parameters, user: alice });
		const expected = [userLite(await show(carol.id, alice))];
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれる', user: () => userNotExplorable },
		{ label: 'ミュートユーザーが含まれない', user: () => userMutedByAlice, excluded: true },
		{ label: 'ブロックされているユーザーが含まれる', user: () => userBlockedByAlice },
		{ label: 'ブロックしてきているユーザーが含まれる', user: () => userBlockingAlice },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		{ label: 'サスペンドユーザーが含まれない', user: () => userSuspended, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
	] as const)('を検索することができ、結果に$labelが含まれる', async ({ user, excluded }) => {
		const parameters = { query: user().username, limit: 1 };
		const response = await successfulApiCall({ endpoint: 'users/search', parameters, user: alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, alice)];
		assert.deepStrictEqual(response, expected);
	});
	test.todo('を検索することができる(リモート)');
	test.todo('を検索することができる(pagenation)');

	//#endregion
	//#region ID指定検索(users/search-by-username-and-host)

	test.each([
		{ label: '自分', parameters: { username: 'alice' }, user: () => [alice] },
		{ label: '自分かつusernameが大文字', parameters: { username: 'ALICE' }, user: () => [alice] },
		{ label: 'ローカルのフォロイーでノートなし', parameters: { username: 'userFollowedByAlice' }, user: () => [userFollowedByAlice] },
		{ label: 'ローカルでノートなしは検索に載らない', parameters: { username: 'userNoNote' }, user: () => [] },
		{ label: 'ローカルの他人1', parameters: { username: 'bob' }, user: () => [bob] },
		{ label: 'ローカルの他人2', parameters: { username: 'bob', host: null }, user: () => [bob] },
		{ label: 'ローカルの他人3', parameters: { username: 'bob', host: '.' }, user: () => [bob] },
		{ label: 'ローカル', parameters: { host: null, limit: 1 }, user: () => [userFollowedByAlice] },
		{ label: 'ローカル', parameters: { host: '.', limit: 1 }, user: () => [userFollowedByAlice] },
	])('をID&ホスト指定で検索できる($label)', async ({ parameters, user }) => {
		const response = await successfulApiCall({ endpoint: 'users/search-by-username-and-host', parameters, user: alice });
		const expected = await Promise.all(user().map(u => show(u.id, alice)));
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれる', user: () => userNotExplorable },
		{ label: 'ミュートユーザーが含まれる', user: () => userMutedByAlice },
		{ label: 'ブロックされているユーザーが含まれる', user: () => userBlockedByAlice },
		{ label: 'ブロックしてきているユーザーが含まれる', user: () => userBlockingAlice },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		{ label: 'サスペンドユーザーが含まれない', user: () => userSuspended, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
	] as const)('をID&ホスト指定で検索でき、結果に$label', async ({ user, excluded }) => {
		const parameters = { username: user().username };
		const response = await successfulApiCall({ endpoint: 'users/search-by-username-and-host', parameters, user: alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, alice)];
		assert.deepStrictEqual(response, expected);
	});
	test.todo('をID&ホスト指定で検索できる(リモート)');

	//#endregion
	//#region ID指定検索(users/get-frequently-replied-users)

	test('がよくリプライをするユーザーのリストを取得できる', async () => {
		const parameters = { userId: alice.id, limit: 5 };
		const response = await successfulApiCall({ endpoint: 'users/get-frequently-replied-users', parameters, user: alice });
		const expected = await Promise.all(usersReplying.slice(0, parameters.limit).map(async (s, i) => ({
			user: await show(s.id, alice),
			weight: (usersReplying.length - i) / usersReplying.length,
		})));
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれる', user: () => userNotExplorable },
		{ label: 'ミュートユーザーが含まれる', user: () => userMutedByAlice },
		{ label: 'ブロックされているユーザーが含まれる', user: () => userBlockedByAlice },
		{ label: 'ブロックしてきているユーザーが含まれない', user: () => userBlockingAlice, excluded: true },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		//{ label: 'サスペンドユーザーが含まれない', user: () => userSuspended, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
	] as const)('がよくリプライをするユーザーのリストを取得でき、結果に$label', async ({ user, excluded }) => {
		const replyTo = (await successfulApiCall({ endpoint: 'users/notes', parameters: { userId: user().id }, user: undefined }))[0];
		await post(alice, { text: `@${user().username} test`, replyId: replyTo.id });
		const parameters = { userId: alice.id, limit: 100 };
		const response = await successfulApiCall({ endpoint: 'users/get-frequently-replied-users', parameters, user: alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, alice)];
		assert.deepStrictEqual(response.map(s => s.user).filter((u) => u.id === user().id), expected);
	});

	//#endregion
	//#region ハッシュタグ(hashtags/users)

	test.each([
		{ label: 'フォロワー昇順', sort: { sort: '+follower' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.followersCount) },
		{ label: 'フォロワー降順', sort: { sort: '-follower' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.followersCount) },
		{ label: '登録日時昇順', sort: { sort: '+createdAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => u.createdAt },
		{ label: '登録日時降順', sort: { sort: '-createdAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => u.createdAt },
		{ label: '投稿日時昇順', sort: { sort: '+updatedAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.updatedAt) },
		{ label: '投稿日時降順', sort: { sort: '-updatedAt' }, selector: (u: misskey.entities.UserDetailedNotMe): string => String(u.updatedAt) },
	] as const)('をハッシュタグ指定で取得することができる($label)', async ({ sort, selector }) => {
		const hashtag = 'test_hashtag';
		await successfulApiCall({ endpoint: 'i/update', parameters: { description: `#${hashtag}` }, user: alice });
		const parameters = { tag: hashtag, limit: 5, ...sort };
		const response = await successfulApiCall({ endpoint: 'hashtags/users', parameters, user: alice });
		const users = await Promise.all(response.map(u => show(u.id, alice)));
		const expected = users.sort((x, y) => {
			const index = (selector(x) < selector(y)) ? -1 : (selector(x) > selector(y)) ? 1 : 0;
			return index * (parameters.sort.startsWith('+') ? -1 : 1);
		});
		assert.deepStrictEqual(response, expected);
	});
	test.each([
		{ label: '「見つけやすくする」がOFFのユーザーが含まれる', user: () => userNotExplorable },
		{ label: 'ミュートユーザーが含まれる', user: () => userMutedByAlice },
		{ label: 'ブロックされているユーザーが含まれる', user: () => userBlockedByAlice },
		{ label: 'ブロックしてきているユーザーが含まれる', user: () => userBlockingAlice },
		{ label: '承認制ユーザーが含まれる', user: () => userLocking },
		{ label: 'サイレンスユーザーが含まれる', user: () => userSilenced },
		{ label: 'サスペンドユーザーが含まれない', user: () => userSuspended, excluded: true },
		{ label: '削除済ユーザーが含まれる', user: () => userDeletedBySelf },
		{ label: '削除済(byAdmin)ユーザーが含まれる', user: () => userDeletedByAdmin },
	] as const)('をハッシュタグ指定で取得することができ、結果に$label', async ({ user, excluded }) => {
		const hashtag = `user_test${user().username}`;
		if (user() !== userSuspended) {
			// サスペンドユーザーはupdateできない。
			await successfulApiCall({ endpoint: 'i/update', parameters: { description: `#${hashtag}` }, user: user() });
		}
		const parameters = { tag: hashtag, limit: 100, sort: '-follower' } as const;
		const response = await successfulApiCall({ endpoint: 'hashtags/users', parameters, user: alice });
		const expected = (excluded ?? false) ? [] : [await show(user().id, alice)];
		assert.deepStrictEqual(response, expected);
	});
	test.todo('をハッシュタグ指定で取得することができる(リモート)');

	//#endregion
	//#region オススメユーザー(users/recommendation)

	// BUG users/recommendationは壊れている？ > QueryFailedError: missing FROM-clause entry for table "note"
	test.skip('のオススメを取得することができる', async () => {
		const parameters = {};
		const response = await successfulApiCall({ endpoint: 'users/recommendation', parameters, user: alice });
		const expected = await Promise.all(response.map(u => show(u.id)));
		assert.deepStrictEqual(response, expected);
	});

	//#endregion
	//#region ピン止めユーザー(pinned-users)

	test('のピン止めユーザーを取得することができる', async () => {
		await successfulApiCall({ endpoint: 'admin/update-meta', parameters: { pinnedUsers: [bob.username, `@${carol.username}`] }, user: root });
		const parameters = {} as const;
		const response = await successfulApiCall({ endpoint: 'pinned-users', parameters, user: alice });
		const expected = await Promise.all([bob, carol].map(u => show(u.id, alice)));
		assert.deepStrictEqual(response, expected);
	});

	//#endregion

	test.todo('を管理人として確認することができる(admin/show-user)');
	test.todo('を管理人として確認することができる(admin/show-users)');
	test.todo('をサーバー向けに取得することができる(federation/users)');
});

describe('users/lists list and show response compatibility', () => {
	let owner: misskey.entities.SignupResponse;
	let other: misskey.entities.SignupResponse;

	beforeAll(async () => {
		owner = await signup({ username: 'listOwner' });
		other = await signup({ username: 'listOther' });
	});

	function errorBody(value: unknown): { code: string; id: string } {
		assert.ok(value !== null && typeof value === 'object' && 'error' in value);
		const error = value.error;
		assert.ok(error !== null && typeof error === 'object');
		assert.ok('code' in error && typeof error.code === 'string');
		assert.ok('id' in error && typeof error.id === 'string');
		return { code: error.code, id: error.id };
	}

	test('preserves own private lists, explicit-self public filtering and member order', async () => {
		const ids: string[] = [];
		try {
			const publicList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'public 😀 e\u0301' }, user: owner });
			ids.push(publicList.id);
			const privateList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'private' }, user: owner });
			ids.push(privateList.id);
			await successfulApiCall({ endpoint: 'users/lists/update', parameters: { listId: publicList.id, isPublic: true }, user: owner });
			await successfulApiCall({ endpoint: 'users/lists/push', parameters: { listId: publicList.id, userId: other.id }, user: owner });
			await successfulApiCall({ endpoint: 'users/lists/push', parameters: { listId: publicList.id, userId: owner.id }, user: owner });
			const mine = await successfulApiCall({ endpoint: 'users/lists/list', parameters: {}, user: owner });
			assert.deepStrictEqual(new Set(mine.map(list => list.id)), new Set(ids));
			const packed = mine.find(list => list.id === publicList.id);
			assert.ok(packed);
			assert.deepStrictEqual(packed, { id: publicList.id, createdAt: publicList.createdAt, name: publicList.name, userIds: [other.id, owner.id], isPublic: true });
			assert.deepStrictEqual(mine.find(list => list.id === privateList.id), privateList);
			const explicit = await successfulApiCall({ endpoint: 'users/lists/list', parameters: { userId: owner.id }, user: owner });
			const anonymous = await api('users/lists/list', { userId: owner.id });
			assert.strictEqual(anonymous.status, 200);
			assert.deepStrictEqual(explicit, [packed]);
			assert.deepStrictEqual(anonymous.body, [packed]);
			const allowed = await api('miauth/gen-token', { session: randomUUID(), permission: ['read:account'] }, owner);
			const denied = await api('miauth/gen-token', { session: randomUUID(), permission: ['read:notifications'] }, owner);
			const scoped = await api('users/lists/list', {}, { token: allowed.body.token });
			assert.strictEqual(scoped.status, 200);
			assert.deepStrictEqual(scoped.body, mine);
			assert.strictEqual((await api('users/lists/list', { userId: owner.id }, { token: denied.body.token })).status, 403);
		} finally {
			for (const listId of ids) await api('users/lists/delete', { listId }, owner);
		}
		assert.deepStrictEqual((await api('users/lists/list', {}, owner)).body, []);
	});

	test('shows own private lists and includes favorite fields only forPublic', async () => {
		const ids: string[] = [];
		try {
			const privateList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'private detail' }, user: owner });
			ids.push(privateList.id);
			const publicList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'public detail 😀' }, user: owner });
			ids.push(publicList.id);
			const base = await successfulApiCall({ endpoint: 'users/lists/update', parameters: { listId: publicList.id, isPublic: true }, user: owner });
			assert.deepStrictEqual(await successfulApiCall({ endpoint: 'users/lists/show', parameters: { listId: privateList.id }, user: owner }), privateList);
			const anonymousDefault = await api('users/lists/show', { listId: publicList.id });
			assert.strictEqual(anonymousDefault.status, 200);
			assert.deepStrictEqual(anonymousDefault.body, base);
			const zero = await api('users/lists/show', { listId: publicList.id, forPublic: true });
			assert.strictEqual(zero.status, 200);
			assert.deepStrictEqual(zero.body, { ...base, likedCount: 0, isLiked: false });
			await successfulApiCall({ endpoint: 'users/lists/favorite', parameters: { listId: publicList.id }, user: other });
			const liked = await successfulApiCall({ endpoint: 'users/lists/show', parameters: { listId: publicList.id, forPublic: true }, user: other });
			assert.deepStrictEqual(liked, { ...base, likedCount: 1, isLiked: true });
			assert.deepStrictEqual(await successfulApiCall({ endpoint: 'users/lists/show', parameters: { listId: publicList.id }, user: owner }), base);
			const allowed = await api('miauth/gen-token', { session: randomUUID(), permission: ['read:account'] }, other);
			const denied = await api('miauth/gen-token', { session: randomUUID(), permission: ['read:notifications'] }, other);
			const scoped = await api('users/lists/show', { listId: publicList.id, forPublic: true }, { token: allowed.body.token });
			assert.strictEqual(scoped.status, 200);
			assert.deepStrictEqual(scoped.body, liked);
			assert.strictEqual((await api('users/lists/show', { listId: publicList.id, forPublic: true }, { token: denied.body.token })).status, 403);
		} finally {
			for (const listId of ids) await api('users/lists/delete', { listId }, owner);
		}
	});

	test('uses the same missing-list error for inaccessible lists and missing IDs', async () => {
		const privateList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'hidden detail' }, user: owner });
		const publicList = await successfulApiCall({ endpoint: 'users/lists/create', parameters: { name: 'other public detail' }, user: owner });
		try {
			await successfulApiCall({ endpoint: 'users/lists/update', parameters: { listId: publicList.id, isPublic: true }, user: owner });
			for (const [params, viewer] of [
				[{ listId: privateList.id }, null],
				[{ listId: privateList.id }, other],
				[{ listId: privateList.id, forPublic: true }, owner],
				[{ listId: publicList.id }, other],
				[{ listId: '0000000000000000' }, owner],
			] as const) {
				const response = await api('users/lists/show', params, viewer ?? undefined);
				assert.strictEqual(response.status, 400);
				assert.deepStrictEqual(errorBody(response.body), { code: 'NO_SUCH_LIST', id: '7bc05c21-1d7a-41ae-88f1-66820f4dc686' });
			}
		} finally {
			for (const listId of [privateList.id, publicList.id]) await api('users/lists/delete', { listId }, owner);
		}
	});

	test('keeps anonymous, missing-user and remote-user errors distinct', async () => {
		const remote = await signup({ username: 'listRemote', host: 'example.com' });
		for (const [params, expected] of [
			[{}, { code: 'INVALID_PARAM', id: 'ab36de0e-29e9-48cb-9732-d82f1281620d' }],
			[{ userId: '0000000000000000' }, { code: 'NO_SUCH_USER', id: 'a8af4a82-0980-4cc4-a6af-8b0ffd54465e' }],
			[{ userId: remote.id }, { code: 'REMOTE_USER_NOT_ALLOWED', id: '53858f1b-3315-4a01-81b7-db9b48d4b79a' }],
		] as const) {
			const response = await api('users/lists/list', params);
			assert.strictEqual(response.status, 400);
			assert.deepStrictEqual(errorBody(response.body), expected);
		}
	});
});

describe('users/relation response compatibility', () => {
	let viewer: misskey.entities.SignupResponse;
	let outgoing: misskey.entities.SignupResponse;
	let incoming: misskey.entities.SignupResponse;
	let unrelated: misskey.entities.SignupResponse;
	const absent = {
		isFollowing: false, isFollowed: false,
		hasPendingFollowRequestFromYou: false, hasPendingFollowRequestToYou: false,
		isBlocking: false, isBlocked: false, isMuted: false, isRenoteMuted: false,
	};

	beforeAll(async () => {
		viewer = await signup({ username: 'relationViewer' });
		outgoing = await signup({ username: 'relationOutgoing' });
		incoming = await signup({ username: 'relationIncoming' });
		unrelated = await signup({ username: 'relationUnrelated' });
	});

	function runtimeObject(value: unknown): asserts value is Record<string, unknown> {
		assert.ok(value !== null && typeof value === 'object' && !Array.isArray(value));
	}

	test('preserves the raw following, all eight directions and an expired mute row', async () => {
		const connection = await initTestDb(true);
		const followings = connection.getRepository(MiFollowing);
		const requests = connection.getRepository(MiFollowRequest);
		const blocks = connection.getRepository(MiBlocking);
		const mutes = connection.getRepository(MiMuting);
		const renoteMutes = connection.getRepository(MiRenoteMuting);
		const ids = Array.from({ length: 8 }, () => randomUUID().replaceAll('-', ''));
		const following = {
			id: ids[0], followeeId: outgoing.id, followerId: viewer.id,
			isFollowerHibernated: false, withReplies: true, notify: null,
			followerHost: null, followerInbox: null, followerSharedInbox: null,
			followeeHost: null, followeeInbox: null, followeeSharedInbox: null,
		};
		try {
			// Insert isolated rows directly: API mutations would also start
			// notifications and remove other edges when blocking these users.
			await followings.insert([following, { ...following, id: ids[1], followerId: incoming.id, followeeId: viewer.id }]);
			await requests.insert([
				{ id: ids[2], followerId: viewer.id, followeeId: outgoing.id, requestId: null, withReplies: false },
				{ id: ids[3], followerId: incoming.id, followeeId: viewer.id, requestId: null, withReplies: false },
			]);
			await blocks.insert([
				{ id: ids[4], blockerId: viewer.id, blockeeId: outgoing.id },
				{ id: ids[5], blockerId: incoming.id, blockeeId: viewer.id },
			]);
			await mutes.insert({ id: ids[6], muterId: viewer.id, muteeId: outgoing.id, expiresAt: new Date(0) });
			await renoteMutes.insert({ id: ids[7], muterId: viewer.id, muteeId: outgoing.id });
			const expectedOutgoing = {
				id: outgoing.id, following, ...absent, isFollowing: true,
				hasPendingFollowRequestFromYou: true, isBlocking: true, isMuted: true, isRenoteMuted: true,
			};
			const scalar = await api('users/relation', { userId: outgoing.id }, viewer);
			assert.strictEqual(scalar.status, 200);
			assert.ok(Array.isArray(scalar.body));
			const raw: unknown = scalar.body[0];
			runtimeObject(raw);
			runtimeObject(raw.following);
			assert.deepStrictEqual(raw.following, following);
			assert.deepStrictEqual(scalar.body, [expectedOutgoing]);
			const batch = await api('users/relation', { userId: [incoming.id, outgoing.id] }, viewer);
			assert.strictEqual(batch.status, 200);
			assert.ok(Array.isArray(batch.body));
			assert.deepStrictEqual(batch.body, [{
				id: incoming.id, following: null, ...absent, isFollowed: true,
				hasPendingFollowRequestToYou: true, isBlocked: true,
			}, expectedOutgoing]);
			const isolated = await api('users/relation', { userId: outgoing.id }, unrelated);
			assert.strictEqual(isolated.status, 200);
			assert.deepStrictEqual(isolated.body, [{ id: outgoing.id, following: null, ...absent }]);
		} finally {
			try {
				await renoteMutes.delete(ids[7]);
				await mutes.delete(ids[6]);
				await blocks.delete([ids[4], ids[5]]);
				await requests.delete([ids[2], ids[3]]);
				await followings.delete([ids[0], ids[1]]);
			} finally { await connection.destroy(); }
		}
	});

	test('keeps scalar responses as arrays and deduplicates IDs in first appearance order', async () => {
		const missing = '0000000000000000';
		for (const id of [viewer.id, missing]) {
			const scalar = await api('users/relation', { userId: id }, viewer);
			assert.strictEqual(scalar.status, 200);
			assert.ok(Array.isArray(scalar.body));
			assert.deepStrictEqual(scalar.body, [{ id, following: null, ...absent }]);
		}
		const empty = await api('users/relation', { userId: [] }, viewer);
		assert.strictEqual(empty.status, 200);
		assert.deepStrictEqual(empty.body, []);
		const repeated = await api('users/relation', { userId: [missing, viewer.id, missing] }, viewer);
		assert.strictEqual(repeated.status, 200);
		assert.ok(Array.isArray(repeated.body));
		assert.deepStrictEqual(repeated.body, [missing, viewer.id].map(id => ({ id, following: null, ...absent })));
	});

	test('requires credentials and read:account for scoped application tokens', async () => {
		const allowed = await createAppToken(viewer, ['read:account']);
		const denied = await createAppToken(viewer, ['read:notifications']);
		const scoped = await api('users/relation', { userId: [] }, { token: allowed, bearer: true });
		assert.strictEqual(scoped.status, 200);
		assert.deepStrictEqual(scoped.body, []);
		const anonymous = await api('users/relation', { userId: [] });
		const forbidden = await api('users/relation', { userId: [] }, { token: denied, bearer: true });
		for (const [response, status, code] of [[anonymous, 401, 'CREDENTIAL_REQUIRED'], [forbidden, 403, 'PERMISSION_DENIED']] as const) {
			assert.strictEqual(response.status, status);
			const body: unknown = response.body;
			runtimeObject(body);
			runtimeObject(body.error);
			assert.strictEqual(body.error.code, code);
		}
	});
});

describe('users/achievements response compatibility', () => {
	let viewer: misskey.entities.SignupResponse;
	let target: misskey.entities.SignupResponse;

	beforeAll(async () => {
		viewer = await signup({ username: 'achievementViewer' });
		target = await signup({ username: 'achievementTarget' });
	});

	function runtimeObject(value: unknown): asserts value is Record<string, unknown> {
		assert.ok(value !== null && typeof value === 'object' && !Array.isArray(value));
	}

	test('returns the stored order and duplicates to anonymous, account and unrelated-scope viewers', async () => {
		const connection = await initTestDb(true);
		const profiles = connection.getRepository(MiUserProfile);
		let original: MiUserProfile | undefined;
		const achievements = [
			{ name: 'login3', unlockedAt: 1700000000000 },
			{ name: 'notes1', unlockedAt: 0 },
			{ name: 'login3', unlockedAt: 1.5 },
		] satisfies MiUserProfile['achievements'];
		try {
			original = await profiles.findOneByOrFail({ userId: target.id });
			await profiles.update(target.id, { achievements });
			const token = await createAppToken(viewer, ['read:notifications']);
			for (const user of [undefined, viewer, { token, bearer: true }]) {
				const response = await api('users/achievements', { userId: target.id }, user);
				assert.strictEqual(response.status, 200);
				const body: unknown = response.body;
				assert.ok(Array.isArray(body));
				assert.deepStrictEqual(body, achievements);
			}
		} finally {
			try { if (original) await profiles.update(target.id, { achievements: original.achievements }); }
			finally { await connection.destroy(); }
		}
	});

	test('retains legacy JSON types and keeps JSON null distinct from a missing profile', async () => {
		const connection = await initTestDb(true);
		const profiles = connection.getRepository(MiUserProfile);
		let original: MiUserProfile | undefined;
		const legacy = [
			{ name: 'unknown-achievement-😀', unlockedAt: '1700000000000', extra: { text: 'e\u0301' } },
			{ name: 'notes1', unlockedAt: null },
		];
		try {
			original = await profiles.findOneByOrFail({ userId: target.id });
			await profiles.query('UPDATE user_profile SET achievements=$1::jsonb WHERE "userId"=$2', [JSON.stringify(legacy), target.id]);
			const response = await api('users/achievements', { userId: target.id });
			assert.strictEqual(response.status, 200);
			const body: unknown = response.body;
			assert.ok(Array.isArray(body));
			runtimeObject(body[0]);
			assert.strictEqual(body[0].unlockedAt, '1700000000000');
			assert.deepStrictEqual(body, legacy);
			const scalar = 'legacy scalar 😀';
			await profiles.query('UPDATE user_profile SET achievements=$1::jsonb WHERE "userId"=$2', [JSON.stringify(scalar), target.id]);
			const text = await relativeFetch('api/users/achievements', {
				method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ userId: target.id }),
			});
			assert.strictEqual(text.status, 200);
			assert.strictEqual(text.headers.get('content-type'), 'text/plain; charset=utf-8');
			assert.strictEqual(await text.text(), JSON.stringify(scalar));
			await profiles.query('UPDATE user_profile SET achievements=$1::jsonb WHERE "userId"=$2', ['null', target.id]);
			const empty = await relativeFetch('api/users/achievements', {
				method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ userId: target.id }),
			});
			assert.strictEqual(empty.status, 204);
			assert.strictEqual(await empty.text(), '');
		} finally {
			try { if (original) await profiles.update(target.id, { achievements: original.achievements }); }
			finally { await connection.destroy(); }
		}
	});

	test('preserves the ordinary missing-profile error instead of returning an empty list', async () => {
		const id = '0000000000000000';
		const response = await api('users/achievements', { userId: id });
		assert.strictEqual(response.status, 500);
		const body: unknown = response.body;
		runtimeObject(body);
		runtimeObject(body.error);
		const error = body.error;
		assert.strictEqual(error.code, 'INTERNAL_ERROR');
		assert.strictEqual(error.id, '5d37dbcb-891e-41ca-a3d6-e690c97775ac');
		runtimeObject(error.info);
		runtimeObject(error.info.e);
		assert.strictEqual(error.info.e.code, 'EntityNotFoundError');
		assert.strictEqual(error.info.e.message, `Could not find any entity of type "MiUserProfile" matching: {\n    "userId": "${id}"\n}`);
		assert.ok(typeof error.info.e.id === 'string');
		assert.match(error.info.e.id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
	});
});

describe('followed-user birthday response compatibility', () => {
	let viewer: misskey.entities.SignupResponse;
	let january: misskey.entities.SignupResponse;
	let december: misskey.entities.SignupResponse;
	let unrelated: misskey.entities.SignupResponse;

	beforeAll(async () => {
		viewer = await signup({ username: 'birthdayViewer' });
		january = await signup({ username: 'birthdayJanuary' });
		december = await signup({ username: 'birthdayDecember' });
		unrelated = await signup({ username: 'birthdayUnrelated' });
	});

	function runtimeObject(value: unknown): asserts value is Record<string, unknown> {
		assert.ok(value !== null && typeof value === 'object' && !Array.isArray(value));
	}

	async function withBirthdays(check: () => Promise<void>) {
		const connection = await initTestDb(true);
		const profiles = connection.getRepository(MiUserProfile);
		const followings = connection.getRepository(MiFollowing);
		const ids = [randomUUID().replaceAll('-', ''), randomUUID().replaceAll('-', '')];
		let originals: MiUserProfile[] = [];
		try {
			// Synchronization omits SQL functions. Provision only the existing
			// migration's exact static definition in this dedicated test DB.
			const migration = await readFile(new URL('../../migration/1767169026317-birthday-index.js', import.meta.url), 'utf8');
			const definitions = [...migration.matchAll(/queryRunner\.query\(`(CREATE OR REPLACE FUNCTION get_birthday_date\(birthday TEXT\)[^`]+)`\)/g)];
			assert.strictEqual(definitions.length, 1);
			assert.ok(!definitions[0][1].includes('${'));
			await connection.query(definitions[0][1]);
			originals = await profiles.findBy([{ userId: january.id }, { userId: december.id }]);
			assert.strictEqual(originals.length, 2);
			await profiles.update(january.id, { birthday: '2000-01-01' });
			await profiles.update(december.id, { birthday: '2000-12-31' });
			// Direct isolated edges avoid follow notifications and queue activity.
			await followings.insert([
				{ id: ids[0], followerId: viewer.id, followeeId: december.id, isFollowerHibernated: false, withReplies: false, notify: null },
				{ id: ids[1], followerId: viewer.id, followeeId: january.id, isFollowerHibernated: false, withReplies: false, notify: null },
			]);
			await check();
		} finally {
			try {
				const cleanup = await Promise.allSettled([
					followings.delete(ids),
					...originals.map(profile => profiles.update(profile.userId, { birthday: profile.birthday })),
				]);
				for (const result of cleanup) assert.strictEqual(result.status, 'fulfilled', 'birthday fixture cleanup failed');
			} finally { await connection.destroy(); }
		}
	}

	test('keeps wrapped month/day ordering, single-day selection and offset pages', async () => {
		await withBirthdays(async () => {
			const birthday = { begin: { month: 12, day: 31 }, end: { month: 1, day: 1 } };
			const result = await api('users/get-following-users-by-birthday', { birthday }, viewer);
			assert.strictEqual(result.status, 200);
			assert.ok(Array.isArray(result.body));
			assert.deepStrictEqual(result.body.map(row => row.id), [january.id, december.id]);
			for (const row of result.body) {
				assert.match(row.birthday, /^\d{4}-\d{2}-\d{2}$/);
				assert.strictEqual(row.user.id, row.id);
				assert.strictEqual(row.user.username, row.id === january.id ? january.username : december.username);
			}
			const explicitDefaults = await api('users/get-following-users-by-birthday', { birthday, limit: 10, offset: 0 }, viewer);
			assert.strictEqual(explicitDefaults.status, 200);
			assert.deepStrictEqual(explicitDefaults.body, result.body);
			for (const [offset, expected] of [[0, [january.id]], [1, [december.id]], [2, []]] as const) {
				const page = await api('users/get-following-users-by-birthday', { birthday, limit: 1, offset }, viewer);
				assert.strictEqual(page.status, 200);
				assert.deepStrictEqual(page.body.map(row => row.id), expected);
			}
			const single = await api('users/get-following-users-by-birthday', { birthday: { month: 1, day: 1 } }, viewer);
			assert.strictEqual(single.status, 200);
			assert.deepStrictEqual(single.body.map(row => row.id), [january.id]);
			// These extra fields pass the single-day schema branch, but the TS
			// handler selects a range whenever both own keys exist. Keep this
			// regression check before considering native activation again.
			for (const begin of [{ month: '1', day: 1 }, { month: 1, day: 0 }]) {
				const acceptedExtraRange = await api('users/get-following-users-by-birthday', {
					birthday: { month: 10, day: 24, begin, end: { month: 12, day: 31 } },
				}, viewer);
				assert.strictEqual(acceptedExtraRange.status, 200);
				assert.deepStrictEqual(acceptedExtraRange.body.map(row => row.id), [january.id, december.id]);
			}
		});
	});

	test('uses the authenticated following graph and preserves credential and app-scope gates', async () => {
		await withBirthdays(async () => {
			const params = { birthday: { month: 1, day: 1 } };
			const allowed = await createAppToken(viewer, ['read:account']);
			const denied = await createAppToken(viewer, ['read:notifications']);
			const account = await api('users/get-following-users-by-birthday', params, viewer);
			const scoped = await api('users/get-following-users-by-birthday', params, { token: allowed, bearer: true });
			assert.strictEqual(account.status, 200);
			assert.strictEqual(scoped.status, 200);
			assert.deepStrictEqual(scoped.body, account.body);
			assert.deepStrictEqual(account.body.map(row => row.id), [january.id]);
			const other = await api('users/get-following-users-by-birthday', params, unrelated);
			assert.strictEqual(other.status, 200);
			assert.deepStrictEqual(other.body, []);
			const anonymous = await api('users/get-following-users-by-birthday', params);
			const forbidden = await api('users/get-following-users-by-birthday', params, { token: denied, bearer: true });
			for (const [response, status, code] of [[anonymous, 401, 'CREDENTIAL_REQUIRED'], [forbidden, 403, 'PERMISSION_DENIED']] as const) {
				assert.strictEqual(response.status, status);
				const body: unknown = response.body;
				runtimeObject(body);
				runtimeObject(body.error);
				assert.strictEqual(body.error.code, code);
			}
		});
	});
});
