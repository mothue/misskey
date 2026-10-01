/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { post, rows, target, timeline, viewer } from './scenarios.js';
import type { BenchmarkScenario, RequestContext } from './types.js';
import { timelineEligible } from '../../fixtures/benchmark/definition.js';
import type { Manifest, FixtureUser } from '../../fixtures/benchmark/definition.js';
const ordinal = (context: RequestContext) => context.requestOrdinal ?? context.requestIndex;
const extra = (context: RequestContext) => context.manifest.extension;
const actors = new WeakMap<Manifest, Map<string, FixtureUser>>();
const actor = (context: RequestContext, userId: string): FixtureUser => {
	let users = actors.get(context.manifest);
	if (!users) { users = new Map([...context.manifest.users, ...extra(context).heavyUsers, ...extra(context).notificationReaders].map(user => [user.id, user])); actors.set(context.manifest, users); }
	const user = users.get(userId); assert(user, 'fixture actor missing'); return user;
};
const asUser = (context: RequestContext, user: FixtureUser): RequestContext => ({ ...context, manifest: { ...context.manifest, users: [user] }, workerId: 0, requestIndex: 0 });
const postAs = (context: RequestContext, userId: string, endpoint: string, body: Record<string, unknown>) => post(asUser(context, actor(context, userId)), endpoint, body);

export const local = timeline('timeline/local', 'notes/local-timeline', 'global');
const heavyBase = timeline('timeline/home-heavy-mute-block', 'notes/timeline', 'home');
const heavyManifests = new WeakMap<Manifest, Manifest>();
const heavyContext = (context: RequestContext) => {
	let manifest = heavyManifests.get(context.manifest);
	if (!manifest) { manifest = { ...context.manifest, users: extra(context).heavyUsers }; heavyManifests.set(context.manifest, manifest); }
	return { ...context, manifest };
};
export const heavy: BenchmarkScenario = { ...heavyBase, shape: { excludedFollowRatio: 0.875, homeCache: 'absent; DB fallback' }, request: context => heavyBase.request(heavyContext(context)), validate: (body, context) => heavyBase.validate(body, heavyContext(context)) };
export const list: BenchmarkScenario = {
	name: 'timeline/list',
	request: context => { const list = extra(context).lists.find(item => item.userId === viewer(context).id); assert(list, 'fixture list missing for viewer'); return post(context, 'notes/user-list-timeline', { listId: list.id, limit: 20 }); },
	validate(body, context) {
		for (const note of rows(body)) {
			const user = viewer(context);
			assert(user.follows.includes(note.userId));
			assert(!note.channelId && !note.channel, 'unexpected list channel');
			assert(timelineEligible({ ...note, replyUserId: note.reply?.userId } as any, 'list', user.id), 'unexpected list reply');
			assert(!actor(context, note.userId).blocks.includes(user.id), 'list author blocks viewer');
			assert(!user.mutes.includes(note.userId) && !user.blocks.includes(note.userId));
			assert(['public', 'home', 'followers'].includes(note.visibility));
		}
	},
};
export const channel: BenchmarkScenario = {
	name: 'timeline/channel', request: context => post(context, 'channels/timeline', { channelId: extra(context).channelId, limit: 20 }),
	validate(body, context) { for (const note of rows(body)) { assert(extra(context).channelNotes.includes(note.id)); assert.equal(note.channelId ?? note.channel?.id, extra(context).channelId); } },
};
const cursor = (context: RequestContext) => context.manifest.noteIds[Math.floor(context.manifest.noteIds.length / 2)];
const userTimeline = timeline('timeline/user-page', 'users/notes', 'user');
export const userPage: BenchmarkScenario = { ...userTimeline,
	request: context => post(context, 'users/notes', { userId: target(context).id, untilId: cursor(context), limit: 20 }),
	validate(body, context) { userTimeline.validate(body, context); for (const note of rows(body)) assert(note.id < cursor(context)); },
};
const richNote = (context: RequestContext) => extra(context).richNotes[context.requestIndex % extra(context).richNotes.length];
export const show: BenchmarkScenario = {
	name: 'note/show', request: context => postAs(context, extra(context).plainNote.userId, 'notes/show', { noteId: extra(context).plainNote.id }),
	validate(body, context) { const note = body as Record<string, any>; assert.equal(note.id, extra(context).plainNote.id); assert.equal(note.text, extra(context).plainNote.text); assert.equal(note.userId, extra(context).plainNote.userId); },
};
export const rich: BenchmarkScenario = {
	name: 'note/show-rich', request: context => postAs(context, richNote(context).userId, 'notes/show', { noteId: richNote(context).id }),
	validate(body, context) {
		const note = body as Record<string, any>; const expected = richNote(context);
		assert.equal(note.id, expected.id); assert.equal(note.userId, expected.userId);
		if (expected.replyId) assert.equal(note.reply?.id, expected.replyId);
		if (expected.renoteId) assert.equal(note.renote?.id, expected.renoteId);
		if (expected.fileIds) assert.deepEqual(note.files.map((file: { id: string }) => file.id), expected.fileIds);
		if (expected.hasPoll) assert.deepEqual(note.poll?.choices.map((choice: { text: string }) => choice.text), ['yes', 'no']);
	},
};
const threadManifests = new WeakMap<Manifest, Manifest>();
const threadContext = (context: RequestContext) => {
	let manifest = threadManifests.get(context.manifest);
	if (!manifest) {
		const author = extra(context).plainNote.userId;
		const authorBlocks = actor(context, author).blocks;
		const users = context.manifest.users.filter(user => ![...user.mutes, ...user.blocks].includes(author) && !authorBlocks.includes(user.id));
		manifest = { ...context.manifest, users };
		threadManifests.set(context.manifest, manifest);
	}
	return { ...context, manifest };
};
export const thread: BenchmarkScenario = {
	name: 'note/thread', shape: { parent: 'fixed public root', viewers: 'parent not muted or blocked' }, request: context => post(threadContext(context), 'notes/children', { noteId: extra(context).threadRoot, limit: 20 }),
	validate(body, context) { for (const note of rows(body)) assert(note.replyId === extra(context).threadRoot || note.renoteId === extra(context).threadRoot); },
};
export const notifications: BenchmarkScenario = {
	name: 'notification/list', request: context => post(context, 'i/notifications', { limit: 20, markAsRead: false }),
	validate(body, context) {
		const expected = extra(context).notifications.filter(item => item.userId === viewer(context).id).toReversed().slice(0, 20);
		assert(expected.length, 'fixture notifications missing for viewer');
		const actual = rows(body);
		assert.deepEqual(actual.map(item => item.id), expected.map(item => item.id), 'notification order/page mismatch');
		for (const notification of actual) {
			const original = expected.find(item => item.id === notification.id); assert(original); assert.equal(notification.type, original.type); assert.equal(notification.user?.id, original.notifierId);
			if (original.noteId) assert.equal(notification.note?.id, original.noteId);
		}
	},
};
export const userShow: BenchmarkScenario = {
	name: 'user/show', request: context => post(context, 'users/show', { userId: target(context).id }),
	validate(body, context) { const user = body as Record<string, any>; assert.equal(user.id, target(context).id); assert.equal(user.host, null); assert.equal(typeof user.username, 'string'); },
};

function relationships(direction: 'followers' | 'following'): BenchmarkScenario {
	return { name: `user/${direction}`, request: context => post(context, `users/${direction}`, { userId: target(context).id, limit: 20 }),
		validate(body, context) {
			for (const relation of rows(body)) {
				if (direction === 'followers') { assert.equal(relation.followeeId, target(context).id); assert.equal(relation.follower.id, relation.followerId); assert(actor(context, relation.followerId).follows.includes(relation.followeeId)); } else { assert.equal(relation.followerId, target(context).id); assert.equal(relation.followee.id, relation.followeeId); assert(target(context).follows.includes(relation.followeeId)); }
			}
		} };
}

export const followers = relationships('followers');
export const following = relationships('following');
export const remove: BenchmarkScenario = {
	name: 'note/delete', write: true, writeDelta: { field: 'notes', direction: -1 }, emptyResponse: true, capacity: manifest => manifest.extension.deleteNotes.length,
	request: context => { const note = extra(context).deleteNotes[ordinal(context)]; return postAs(context, note.userId, 'notes/delete', { noteId: note.id }); },
	validate(body) { assert.equal(body, null); },
};

function reaction(direction: 'create' | 'delete'): BenchmarkScenario {
	const targets = (context: RequestContext) => direction === 'create' ? extra(context).reactionCreateTargets : extra(context).reactionDeleteTargets;
	return { name: `reaction/${direction}`, write: true, writeDelta: { field: 'reactions', direction: direction === 'create' ? 1 : -1 }, emptyResponse: true, capacity: manifest => direction === 'create' ? manifest.extension.reactionCreateTargets.length : manifest.extension.reactionDeleteTargets.length,
		request: context => { const item = targets(context)[ordinal(context)]; return postAs(context, item.userId, `notes/reactions/${direction}`, { noteId: item.noteId, ...(direction === 'create' ? { reaction: '👍' } : {}) }); },
		validate(body) { assert.equal(body, null); } };
}

export const reactionCreate = reaction('create');
export const reactionDelete = reaction('delete');
export const followCreate: BenchmarkScenario = {
	name: 'relationship/follow-create', write: true, writeDelta: { field: 'follows', direction: 1 }, capacity: manifest => manifest.extension.followTargets.length,
	request: context => { const pair = extra(context).followTargets[ordinal(context)]; return postAs(context, pair.userId, 'following/create', { userId: pair.targetId }); },
	validate(body, context) { const user = body as Record<string, any>; assert.equal(user.id, extra(context).followTargets[ordinal(context)].targetId); /* Following/create returns UserLite; relation is checked after measurement. */ },
};
export const markRead: BenchmarkScenario = {
	name: 'notification/mark-read', write: true, writeDelta: { field: 'readMarkers', direction: 1 }, emptyResponse: true, capacity: manifest => manifest.extension.notificationReaders.length,
	request: context => postAs(context, extra(context).notificationReaders[ordinal(context)].id, 'notifications/mark-all-as-read', {}),
	validate(body) { assert.equal(body, null); },
};
