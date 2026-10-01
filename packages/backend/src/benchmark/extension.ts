/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { createHash } from 'node:crypto';
import { epoch, id, seed } from './definition.js';
import type { FixtureNote, FixtureSize, FixtureUser } from './definition.js';

export const fixtureImage = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=';

export function extension(size: FixtureSize, users: FixtureUser[], notes: FixtureNote[], reactionNotes: string[]) {
	const makeUser = (kind: number, index: number): FixtureUser => ({ key: `extra/${kind}/${index}`, id: id(kind, index),
		token: createHash('sha256').update(`${seed}:${size}:${kind}:${index}`).digest('hex').slice(0, 16), follows: [], mutes: [], blocks: [] });
	const heavyUsers = Array.from({ length: users.length / 4 }, (_, index) => {
		const user = makeUser(14, index);
		user.follows = Array.from({ length: users.length / 2 }, (_, offset) => users[(index + offset) % users.length].id);
		user.mutes = user.follows.slice(0, user.follows.length * 7 / 8);
		user.blocks = Array.from({ length: user.follows.length / 8 }, (_, offset) => users[(index + user.follows.length + offset) % users.length].id);
		return user;
	});
	// Unique accounts ensure every mark-read request changes unread state, including warmup.
	const notificationReaders = Array.from({ length: size === 'small' ? 2048 : 16384 }, (_, index) => makeUser(15, index));
	const publicNotes = notes.filter(note => note.visibility === 'public' && !reactionNotes.includes(note.id));
	const channelId = id(10, 0);
	const channelNotes = publicNotes.slice(0, 64);
	for (const note of channelNotes) note.channelId = channelId;
	const plain = publicNotes[64];
	const richNotes = publicNotes.slice(65, 129);
	for (const [index, note] of richNotes.entries()) {
		if (index % 4 === 0) { note.replyId = plain.id; note.replyUserId = plain.userId; }
		if (index % 4 === 1) { note.renoteId = plain.id; note.renoteUserId = plain.userId; }
		if (index % 4 === 2) note.fileIds = [id(11, index)];
		if (index % 4 === 3) note.hasPoll = true;
	}
	plain.repliesCount = richNotes.filter(note => note.replyId).length;
	plain.renoteCount = richNotes.filter(note => note.renoteId).length;
	const deleteNotes = publicNotes.slice(129).map(note => ({ id: note.id, userId: note.userId }));
	const lists = users.map((user, index) => ({ id: id(7, index), userId: user.id, members: user.follows }));
	const followTargets: { userId: string; targetId: string }[] = [];
	const capacity = size === 'small' ? 8192 : 65536;
	for (let offset = 1; offset < users.length && followTargets.length < capacity; offset++) {
		for (let index = 0; index < users.length && followTargets.length < capacity; index++) {
			const user = users[index];
			const target = users[(index + offset) % users.length];
			if (![...user.follows, ...user.mutes, ...user.blocks].includes(target.id) && !target.blocks.includes(user.id)) {
				followTargets.push({ userId: user.id, targetId: target.id });
			}
		}
	}
	const publicByAuthor = new Map<string, FixtureNote>();
	for (const note of publicNotes) if (!note.channelId && !publicByAuthor.has(note.userId)) publicByAuthor.set(note.userId, note);
	let sequence = 0;
	const notifications = [...users, ...notificationReaders].flatMap((user, index) => {
		const count = index < users.length ? 32 : 1;
		return Array.from({ length: count }, (_, offset) => {
			const key = sequence++;
			const type = offset % 3 === 0 ? 'follow' : offset % 3 === 1 ? 'mention' : 'reaction';
			const notifierId = index < users.length ? user.follows[offset % user.follows.length] : plain.userId;
			const notifiedNote = publicByAuthor.get(notifierId) ?? plain;
			return { userId: user.id, id: id(13, key), createdAt: new Date(epoch + key).toISOString(), type,
				notifierId, ...(type !== 'follow' ? { noteId: notifiedNote.id } : {}), ...(type === 'reaction' ? { reaction: '👍' } : {}) };
		});
	});
	return { imageChecksum: createHash('sha256').update(Buffer.from(fixtureImage, 'base64')).digest('hex'), heavyUsers, notificationReaders, notifications, lists, channelId, channelNotes: channelNotes.map(note => note.id),
		plainNote: plain, richNotes, fileIds: richNotes.flatMap(note => note.fileIds ?? []), threadRoot: plain.id, deleteNotes, followTargets,
		reactionCreateTargets: deleteNotes.map(note => ({ noteId: note.id, userId: users.find(user => user.id === note.userId)!.follows[0] })),
		reactionDeleteTargets: deleteNotes.map(note => ({ noteId: note.id, userId: note.userId })) };
}
export type FixtureExtension = ReturnType<typeof extension>;
