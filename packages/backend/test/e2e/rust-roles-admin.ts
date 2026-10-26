/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'vitest';
import { loadConfig } from '@/config.js';
import { DEFAULT_POLICIES } from '@/core/RoleService.js';
import { IdService } from '@/core/IdService.js';
import { MiRole } from '@/models/Role.js';
import { MiRoleAssignment } from '@/models/RoleAssignment.js';
import { MiRelay } from '@/models/Relay.js';
import { api, castAsError, createAppToken, initTestDb, signup } from '../utils.js';
import type { UserToken } from '../utils.js';
import type { DataSource } from 'typeorm';

type Account = UserToken & { id: string };

describe('Role and administrator read API compatibility', () => {
	let root: Account;
	let regular: Account;
	let third: Account;
	let connection: DataSource;
	const ids = new IdService(loadConfig());
	const now = Date.now();
	const roleIds = [ids.gen(now - 30_000), ids.gen(now - 20_000), ids.gen(now - 10_000)];
	const relayId = ids.gen(now - 1000);

	beforeAll(async () => {
		root = await signup({ username: 'rolesroot' });
		regular = await signup({ username: 'rolesuser' });
		third = await signup({ username: 'rolesother' });
		for (const user of [root, regular, third]) assert.equal(typeof user.id, 'string', 'signup fixture must succeed');
		connection = await initTestDb(true);
		for (const [index, id] of roleIds.entries()) {
			await connection.getRepository(MiRole).insert({
				id, name: `Native role ${index}`, description: '日本語 😀', updatedAt: new Date(-1),
				lastUsedAt: new Date(now + index * 1000), color: null, iconUrl: null,
				target: index === 1 ? 'conditional' : 'manual', condFormula: { id: 'local', type: 'isLocal' },
				isPublic: index !== 2, isExplorable: index === 0, isAdministrator: false,
				isModerator: false, asBadge: true, preserveAssignmentOnMoveAccount: true,
				canEditMembersByModerator: false, displayOrder: 42,
				policies: { customRule: { useDefault: false, priority: 2, value: ['custom'] } },
			});
		}
		// Null known policy entries are filled by DEFAULT_POLICIES; unknown entries
		// are retained. Direct SQL also supplies a real PG submillisecond timestamp.
		await connection.query('UPDATE role SET policies = policies || $2::jsonb, "updatedAt" = $3::timestamptz WHERE id = $1', [roleIds[0], JSON.stringify({ canSearchUsers: null }), '1969-12-31T23:59:59.999999Z']);
		for (const [index, user] of [root, regular, third].entries()) {
			await connection.getRepository(MiRoleAssignment).insert({
				id: ids.gen(), roleId: roleIds[0], userId: user.id,
				expiresAt: index === 0 ? new Date(now + 3_600_000) : index === 1 ? new Date(now - 3_600_000) : null,
			});
		}
		await connection.getRepository(MiRelay).insert({ id: relayId, inbox: 'https://relay.example.test/inbox', status: 'rejected' });
	}, 120_000);

	afterAll(async () => {
		await connection?.destroy();
	});

	test('public and administrator roles retain visibility, complete packing, policy defaults and assignment counts', async () => {
		assert.equal((await api('roles/list', {})).status, 401);
		const list = await api('roles/list', {}, regular);
		assert.equal(list.status, 200);
		assert.deepEqual(list.body.map(role => role.id), [roleIds[0]]);
		const shown = await api('roles/show', { roleId: roleIds[0] });
		assert.equal(shown.status, 200);
		assert.deepEqual(shown.body, list.body[0]);
		assert.equal(shown.body.createdAt, ids.parse(roleIds[0]).date.toISOString());
		assert.equal(shown.body.updatedAt, '1969-12-31T23:59:59.999Z');
		assert.equal(shown.body.usersCount, 2);
		assert.equal(shown.body.description, '日本語 😀');
		assert.equal(shown.body.color, null);
		assert.equal(shown.body.displayOrder, 42);
		assert.equal(shown.body.preserveAssignmentOnMoveAccount, true);
		for (const [name, value] of Object.entries(DEFAULT_POLICIES)) {
			assert.deepEqual(shown.body.policies[name], { useDefault: true, priority: 0, value });
		}
		assert.deepEqual(shown.body.policies.customRule, { useDefault: false, priority: 2, value: ['custom'] });
		// A conditional public role is showable even when it is not explorable;
		// its count remains manual assignments rather than matched users.
		const conditional = await api('roles/show', { roleId: roleIds[1] }, regular);
		assert.equal(conditional.status, 200);
		assert.equal(conditional.body.usersCount, 0);
		assert.equal(conditional.body.isExplorable, false);
		assert.deepEqual(conditional.body.condFormula, { id: 'local', type: 'isLocal' });
		const hidden = await api('roles/show', { roleId: roleIds[2] }, root);
		assert.equal(hidden.status, 400);
		assert.equal(castAsError(hidden.body).error.id, 'de5502bf-009a-4639-86c1-fec349e46dcb');
		const adminList = await api('admin/roles/list', {}, root);
		assert.equal(adminList.status, 200);
		assert.deepEqual(adminList.body.map(role => role.id), [...roleIds].reverse());
		const privateRole = await api('admin/roles/show', { roleId: roleIds[2] }, root);
		assert.equal(privateRole.status, 200);
		assert.equal(privateRole.body.isPublic, false);
		const missing = await api('admin/roles/show', { roleId: 'missingrole' }, root);
		assert.equal(missing.status, 400);
		assert.equal(castAsError(missing.body).error.id, '07dc7d34-c0d8-49b7-96c6-db3ce64ee0b3');
		assert.equal((await api('admin/roles/list', {}, regular)).status, 403);
	});

	test('relay and database statistics preserve their raw fields and administrator scopes', async () => {
		const relays = await api('admin/relays/list', {}, root);
		assert.equal(relays.status, 200);
		assert.deepEqual(relays.body, [{ id: relayId, inbox: 'https://relay.example.test/inbox', status: 'rejected' }]);
		const indexes = await api('admin/get-index-stats', {}, root);
		assert.equal(indexes.status, 200);
		assert.ok(indexes.body.length > 0);
		assert.deepEqual(Object.keys(indexes.body[0]), ['schemaname', 'tablename', 'indexname', 'tablespace', 'indexdef']);
		const tables = await api('admin/get-table-stats', {}, root);
		assert.equal(tables.status, 200);
		assert.ok('role' in tables.body);
		assert.equal(typeof tables.body.role.count, 'number');
		assert.equal(typeof tables.body.role.size, 'number');
		assert.ok(Number.isInteger(tables.body.role.count));
		assert.ok(tables.body.role.size > 0);
		assert.equal((await api('admin/get-table-stats', {}, regular)).status, 403);
		const token = await createAppToken(root, ['read:admin:roles']);
		assert.equal((await api('admin/roles/list', {}, { token })).status, 200);
		assert.equal((await api('admin/get-index-stats', {}, { token })).status, 403);
	});
});
