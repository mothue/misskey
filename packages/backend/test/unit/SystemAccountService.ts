/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { afterAll, beforeAll, describe, expect, test, vi } from 'vitest';
import { Test } from '@nestjs/testing';
import type { TestingModule } from '@nestjs/testing';
import { DataSource } from 'typeorm';
import { GlobalModule } from '@/GlobalModule.js';
import { DI } from '@/di-symbols.js';
import { IdService } from '@/core/IdService.js';
import { SystemAccountService, SYSTEM_ACCOUNT_TYPES } from '@/core/SystemAccountService.js';
import { MiSystemAccount, MiUsedUsername, MiUser, MiUserKeypair, MiUserProfile } from '@/models/_.js';

describe('SystemAccountService concurrent creation', () => {
	const apps: TestingModule[] = [];

	beforeAll(async () => {
		// Separate containers and connection pools reproduce HTTP/queue contexts.
		for (let i = 0; i < 2; i++) {
			apps.push(await Test.createTestingModule({
				imports: [GlobalModule],
				providers: [SystemAccountService, IdService],
			}).compile());
		}
	});

	afterAll(async () => {
		await Promise.all(apps.map(app => app.close()));
	});

	test.each(SYSTEM_ACCOUNT_TYPES)('shares one complete %s account across contexts', async (type) => {
		const db = apps[0].get<DataSource>(DI.db);
		const services = apps.map(app => app.get(SystemAccountService));
		const username = `system.${type}`;
		const clean = async () => {
			await db.getRepository(MiUser).delete({ usernameLower: username });
			await db.getRepository(MiUsedUsername).delete({ username });
			for (const service of services) service.clearCache();
		};
		await clean();
		const blocker = db.createQueryRunner();
		await blocker.connect();
		await blocker.startTransaction();
		await blocker.query('SELECT pg_advisory_xact_lock($1, $2)', [0x4d4b5341, SYSTEM_ACCOUNT_TYPES.indexOf(type)]);
		const pending = Promise.allSettled(services.map(service => service.fetch(type)));
		try {
			// Observe two real blocked PostgreSQL sessions, without timing sleeps.
			await vi.waitFor(async () => {
				const rows = await db.query('SELECT count(*)::int AS count FROM pg_locks WHERE locktype = $1 AND classid = $2::oid AND objid = $3::oid AND NOT granted', ['advisory', 0x4d4b5341, SYSTEM_ACCOUNT_TYPES.indexOf(type)]);
				expect(rows[0].count).toBe(2);
			}, { timeout: 5000 });
			await blocker.rollbackTransaction();
			const results = await pending;
			expect(results.map(result => result.status)).toEqual(['fulfilled', 'fulfilled']);
			if (results[0].status !== 'fulfilled' || results[1].status !== 'fulfilled') throw new Error('Concurrent account creation failed');
			const userId = results[0].value.id;
			expect(results[1].value.id).toBe(userId);
			expect(await db.getRepository(MiUser).countBy({ usernameLower: username })).toBe(1);
			expect(await db.getRepository(MiSystemAccount).countBy({ type, userId })).toBe(1);
			expect(await db.getRepository(MiUsedUsername).countBy({ username })).toBe(1);
			expect(await db.getRepository(MiUserProfile).countBy({ userId })).toBe(1);
			expect(await db.getRepository(MiUserKeypair).countBy({ userId })).toBe(1);
		} finally {
			if (blocker.isTransactionActive) await blocker.rollbackTransaction();
			await pending;
			await blocker.release();
			await clean();
		}
	});
});
