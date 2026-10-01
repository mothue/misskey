/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import 'reflect-metadata';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { DataSource } from 'typeorm';
import { Redis } from 'ioredis';
import { NestFactory } from '@nestjs/core';
import { loadConfig } from '@/config.js';
import { entities } from '@/postgres.js';
import { MainModule } from '@/MainModule.js';
import { QueueProcessorModule } from '@/queue/QueueProcessorModule.js';
import { QueueProcessorService } from '@/queue/QueueProcessorService.js';
import { HttpRequestService } from '@/core/HttpRequestService.js';
import { QueueService } from '@/core/QueueService.js';
import { ServerService } from '@/server/ServerService.js';
import { readyRef } from '@/boot/ready.js';
import { configureLogging } from '@/logging/logging-runtime.js';
import { initExtraThreadPool } from '@/boot/common.js';
import { MiUser } from '@/models/User.js';
import { MiUserProfile } from '@/models/UserProfile.js';
import { MiNote } from '@/models/Note.js';
import { MiFollowing } from '@/models/Following.js';
import { MiMuting } from '@/models/Muting.js';
import { MiBlocking } from '@/models/Blocking.js';
import { MiNoteReaction } from '@/models/NoteReaction.js';
import { MiUserList } from '@/models/UserList.js';
import { MiUserListMembership } from '@/models/UserListMembership.js';
import { MiChannel } from '@/models/Channel.js';
import { MiPoll } from '@/models/Poll.js';
import { MiDriveFile } from '@/models/DriveFile.js';
import { MiMeta } from '@/models/Meta.js';
import { fixtureImage } from './extension.js';
import { definition, epoch, id, timelineEligible, notificationStreamId } from './definition.js';
import type { FixtureSize } from './definition.js';

const config = loadConfig();
configureLogging(config.logging);
assert(config.db.host === '127.0.0.1' && config.db.port === 54412 && config.db.db === 'misskey-benchmark');
assert(config.redis.host === '127.0.0.1' && config.redis.port === 56412 && config.port === 61912);
assert(process.env.NODE_ENV === 'benchmark' && process.env.MISSKEY_STORAGE_ROOT === resolve(import.meta.dirname, '../../../.benchmark/storage'));
const [mode, size, manifestPath] = process.argv.slice(2);

if (mode === 'seed') {
	assert(size === 'small' || size === 'standard');
	const db = new DataSource({ type: 'postgres', host: config.db.host, port: config.db.port, database: config.db.db,
		username: config.db.user, password: config.db.pass, entities,
		migrations: [resolve(process.env.MISSKEY_BENCH_MIGRATIONS!, '*.js')], migrationsTransactionMode: 'all' });
	const redis = new Redis(config.redis);
	try {
		await db.initialize();
		await db.runMigrations();
		const { manifest, notes } = definition(size as FixtureSize);
		await db.getRepository(MiMeta).insert({ id: 'benchmark', disableRegistration: true, enableFanoutTimeline: true, enableFanoutTimelineDbFallback: true, enableReactionsBuffering: true });
		const allUsers = [...manifest.users, ...manifest.extension.heavyUsers, ...manifest.extension.notificationReaders];
		for (let offset = 0; offset < allUsers.length; offset += 100) {
			const users = allUsers.slice(offset, offset + 100);
			await db.getRepository(MiUser).insert(users.map((user, i) => ({ id: user.id, username: `bench${offset + i}`, usernameLower: `bench${offset + i}`, token: user.token, host: null })));
			await db.getRepository(MiUserProfile).insert(users.map(user => ({ userId: user.id, emailNotificationTypes: [], password: null })));
		}
		let relation = 0;
		for (const user of allUsers) {
			if (!user.follows.length) continue;
			await db.getRepository(MiFollowing).insert(user.follows.map(followeeId => ({ id: id(3, relation++), followerId: user.id, followeeId })));
			await db.getRepository(MiMuting).insert(user.mutes.map(muteeId => ({ id: id(4, relation++), muterId: user.id, muteeId })));
			await db.getRepository(MiBlocking).insert(user.blocks.map(blockeeId => ({ id: id(5, relation++), blockerId: user.id, blockeeId })));
		}
		const extra = manifest.extension;
		await db.getRepository(MiChannel).insert({ id: extra.channelId, userId: manifest.users[0].id, name: 'benchmark', notesCount: extra.channelNotes.length });
		await db.getRepository(MiUserList).insert(extra.lists.map(list => ({ id: list.id, userId: list.userId, name: 'benchmark' })));
		const memberships = extra.lists.flatMap(list => list.members.map(userId => ({ id: id(8, relation++), userId, userListId: list.id, userListUserId: list.userId })));
		for (let offset = 0; offset < memberships.length; offset += 500) await db.getRepository(MiUserListMembership).insert(memberships.slice(offset, offset + 500));
		const png = Buffer.from(fixtureImage, 'base64');
		await mkdir(resolve(config.rootDir, 'files'), { recursive: true });
		for (const note of extra.richNotes.filter(note => note.fileIds?.length)) {
			const fileId = note.fileIds![0];
			const key = `benchmark-${fileId}.png`;
			await db.getRepository(MiDriveFile).insert({ id: fileId, userId: note.userId, md5: createHash('md5').update(png).digest('hex'), name: key, type: 'image/png', size: png.length, storedInternal: true, accessKey: key, url: `${config.url}/files/${key}`, properties: { width: 1, height: 1 } });
			await writeFile(resolve(config.rootDir, 'files', key), png);
		}
		for (let offset = 0; offset < notes.length; offset += 500) {
			await db.getRepository(MiNote).insert(notes.slice(offset, offset + 500).map(note => ({ ...note, localOnly: true })));
		}
		await db.getRepository(MiPoll).insert(extra.richNotes.filter(note => note.hasPoll).map(note => ({ noteId: note.id, userId: note.userId, multiple: false, choices: ['yes', 'no'], votes: [0, 0], noteVisibility: 'public' as const })));
		let reaction = 0;
		for (const noteId of manifest.reactionNotes) {
			await db.getRepository(MiNoteReaction).insert(manifest.users.slice(0, manifest.counts.reactionsPerNote).map(user => ({ id: id(6, reaction++), userId: user.id, noteId, reaction: '👍' })));
			await db.getRepository(MiNote).update(noteId, { reactions: { '👍': manifest.counts.reactionsPerNote } });
		}
		for (let offset = 0; offset < extra.reactionDeleteTargets.length; offset += 500) {
			await db.getRepository(MiNoteReaction).insert(extra.reactionDeleteTargets.slice(offset, offset + 500).map(target => ({ id: id(16, reaction++), ...target, reaction: '👍' })));
		}
		await db.query(`UPDATE note SET reactions = '{"👍":1}'::jsonb WHERE id = ANY($1)`, [extra.deleteNotes.map(note => note.id)]);
		await db.query('UPDATE "user" u SET "notesCount" = (SELECT count(*) FROM note n WHERE n."userId" = u.id), "followingCount" = (SELECT count(*) FROM following f WHERE f."followerId" = u.id), "followersCount" = (SELECT count(*) FROM following f WHERE f."followeeId" = u.id)');
		const prefix = '';
		const pipeline = redis.pipeline();
		const descending = notes.toReversed();
		pipeline.rpush(`${prefix}list:localTimeline`, ...descending.filter(note => timelineEligible(note, 'local')).slice(0, 1000).map(note => note.id));
		const byAuthor = new Map<string, typeof notes>();
		for (const note of descending) {
			const authored = byAuthor.get(note.userId) ?? [];
			authored.push(note);
			byAuthor.set(note.userId, authored);
		}
		for (const user of manifest.users) {
			const excluded = new Set([...user.mutes, ...user.blocks]);
			const home = [user.id, ...user.follows].filter(author => !excluded.has(author))
				.flatMap(author => (byAuthor.get(author) ?? []).filter(note => timelineEligible(note, 'home', user.id)).slice(0, 1000))
				.sort((a, b) => b.id.localeCompare(a.id)).slice(0, 1000);
			pipeline.rpush(`${prefix}list:homeTimeline:${user.id}`, ...home.map(note => note.id));
			const timeline = (byAuthor.get(user.id) ?? []).filter(note => timelineEligible(note, 'user')).slice(0, 1000);
			if (timeline.length) pipeline.rpush(`${prefix}list:userTimeline:${user.id}`, ...timeline.map(note => note.id));
		}
		for (const list of extra.lists) {
			const listNotes = list.members.flatMap(author => (byAuthor.get(author) ?? []).slice(0, 1000))
				.filter(note => timelineEligible(note, 'list', list.userId)).sort((a, b) => b.id.localeCompare(a.id)).slice(0, 1000);
			pipeline.rpush(`list:userListTimeline:${list.id}`, ...listNotes.map(note => note.id));
		}
		pipeline.rpush(`list:channelTimeline:${extra.channelId}`, ...extra.channelNotes.toReversed());
		const notificationOffsets = new Map<string, number>();
		const coreUsers = new Set(manifest.users.map(user => user.id));
		for (const notification of extra.notifications) {
			const { userId, ...data } = notification;
			const streamId = notificationStreamId(notification.id);
			pipeline.xadd(`notificationTimeline:${userId}`, streamId, 'data', JSON.stringify(data));
			const offset = notificationOffsets.get(userId) ?? 0;
			notificationOffsets.set(userId, offset + 1);
			if (coreUsers.has(userId) && offset === 15) pipeline.set(`latestReadNotification:${userId}`, streamId);
		}
		const responses = await pipeline.exec();
		assert(responses?.every(([error]) => error === null), 'Redis fixture initialization failed');
		await db.query('ANALYZE');
		await writeFile(manifestPath, JSON.stringify(manifest), { mode: 0o600 });
	} finally {
		await redis.quit();
		if (db.isInitialized) await db.destroy();
	}
} else if (mode === 'server') {
	initExtraThreadPool(config);
	const app = await NestFactory.createApplicationContext(MainModule, { logger: false });
	const queues = await NestFactory.createApplicationContext(QueueProcessorModule, { logger: false });
	// Keep the benchmark isolated even if an unexpected preview/webhook job tries HTTP.
	for (const context of [app, queues]) {
		Object.defineProperty(context.get(HttpRequestService), 'send', {
			value: async () => { throw new Error('Outbound HTTP is disabled in benchmarks'); },
		});
	}
	void queues.get(QueueProcessorService).start();
	const server = app.get(ServerService);
	await server.launch();
	readyRef.value = true;
	process.send?.({ type: 'ready' });
	const respond = async () => {
		const counts = await app.get(QueueService).queueGetQueues();
		process.send?.({ type: 'queues', counts: counts.map(queue => ({ name: queue.name, counts: queue.counts })) });
	};
	process.on('message', message => {
		if ((message as { type?: string }).type === 'queues') void respond();
	});
	let closing = false;
	const close = async () => {
		if (closing) return;
		closing = true;
		await server.dispose();
		await queues.close();
		await app.close();
		process.disconnect?.();
	};
	process.on('SIGTERM', () => void close());
	process.on('SIGINT', () => void close());
} else {
	throw new Error('Unknown benchmark runtime mode');
}
