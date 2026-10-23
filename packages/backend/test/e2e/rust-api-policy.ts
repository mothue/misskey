/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';
import { Redis } from 'ioredis';
import { loadConfig } from '@/config.js';
import { MiMeta } from '@/models/Meta.js';
import { api, initTestDb, relativeFetch, role, signup } from '../utils.js';
import type { UserToken } from '../utils.js';
import type { DataSource } from 'typeorm';

type Account = UserToken & { id: string };
const searchRequest = { query: 'missing-rust-policy-user', detail: false };
const denied = { error: { message: 'You are not assigned to a required role.', code: 'ROLE_PERMISSION_DENIED', id: '7f86f06f-7e15-4057-8561-f4b6d4ac755a', kind: 'permission' } };

/** Every request is bounded: a regression must fail instead of waiting indefinitely. */
function post(path: string, body: unknown, actor?: UserToken) {
	return relativeFetch(`api/${path}`, {
		method: 'POST', headers: { 'content-type': 'application/json', ...(actor ? { authorization: `Bearer ${actor.token}` } : {}) },
		body: JSON.stringify(body), signal: AbortSignal.timeout(5000),
	});
}

async function statusOf(path: string, body: unknown, actor?: UserToken): Promise<number> {
	const response = await post(path, body, actor);
	await response.arrayBuffer();
	return response.status;
}

async function waitForStatus(path: string, body: unknown, actor: UserToken, expected: number): Promise<void> {
	const deadline = Date.now() + 5000;
	let actual: number;
	do {
		actual = await statusOf(path, body, actor);
		if (actual === expected) return;
		await new Promise(resolve => setTimeout(resolve, 25));
	} while (Date.now() < deadline);
	assert.equal(actual, expected, `waiting for ${path} fixture access`);
}

function errorIdentity(value: unknown): { code: string; id: string; kind: string } {
	assert.ok(value !== null && typeof value === 'object' && 'error' in value);
	const error = value.error;
	assert.ok(error !== null && typeof error === 'object');
	assert.ok('code' in error && typeof error.code === 'string');
	assert.ok('id' in error && typeof error.id === 'string');
	assert.ok('kind' in error && typeof error.kind === 'string');
	return { code: error.code, id: error.id, kind: error.kind };
}

describe('Rust API required-role policies', () => {
	let root: Account;
	let operator: Account;
	let regular: Account;
	let connection: DataSource;
	let publisher: Redis;
	let saved: Pick<MiMeta, 'rootUserId' | 'policies'> | undefined;
	let metaId: string;
	let searchRoleId: string;
	const config = loadConfig();

	beforeAll(async () => {
		root = await signup({ username: 'policyroot' });
		operator = await signup({ username: 'policyadmin' });
		regular = await signup({ username: 'policyuser' });
		for (const user of [root, operator, regular]) {
			assert.equal(typeof user.id, 'string', 'fixture signup must succeed');
			assert.equal(typeof user.token, 'string', 'fixture must have a native token');
		}
		// Keep the root free of roles, so its bypass cannot be confused with an
		// administrator-role bypass. A separate operator can restore null-root fixtures.
		const adminRole = await role(root, { name: 'Policy fixture administrator', isAdministrator: true });
		assert.equal(typeof adminRole.id, 'string');
		assert.equal((await api('admin/roles/assign', { userId: operator.id, roleId: adminRole.id }, root)).status, 204);
		await waitForStatus('admin/meta', {}, operator, 200);
		const searchRole = await role(root, { name: 'Search policy override' }, { canSearchUsers: { useDefault: false, priority: 1, value: true } });
		assert.equal(typeof searchRole.id, 'string');
		searchRoleId = searchRole.id;
		connection = await initTestDb(true);
		const rows = await connection.getRepository(MiMeta).find({ order: { id: 'DESC' }, take: 1 });
		assert.equal(rows.length, 1);
		metaId = rows[0].id;
		assert.equal(rows[0].rootUserId, root.id);
		saved = { rootUserId: rows[0].rootUserId, policies: structuredClone(rows[0].policies) };
		publisher = new Redis(config.redisForPubsub);
	});

	async function metaFixture(changes: Partial<Pick<MiMeta, 'rootUserId' | 'policies'>>): Promise<void> {
		const repository = connection.getRepository(MiMeta);
		const before = await repository.findOneByOrFail({ id: metaId });
		await repository.update(metaId, changes);
		const after = await repository.findOneByOrFail({ id: metaId });
		await publisher.publish(config.host, JSON.stringify({ channel: 'internal', message: { type: 'metaUpdated', body: { before, after } } }));
		if ('policies' in changes) await publisher.publish(config.host, JSON.stringify({ channel: 'internal', message: { type: 'policiesUpdated', body: after.policies } }));
	}

	async function restore(): Promise<void> {
		if (saved && publisher && connection?.isInitialized) await metaFixture(saved);
	}

	afterAll(async () => {
		try { await restore(); } finally {
			publisher?.disconnect();
			await connection?.destroy();
		}
	});

	async function policies(changes: Record<string, unknown>): Promise<void> {
		const response = await post('admin/roles/update-default-policies', { policies: { ...saved!.policies, ...changes } }, operator);
		assert.equal(response.status, 204);
		assert.equal(await response.text(), '');
	}

	async function expectSearch(status: number, actor?: UserToken): Promise<void> {
		await expect.poll(() => statusOf('users/search', searchRequest, actor), { timeout: 5000, interval: 25 }).toBe(status);
		const response = await post('users/search', searchRequest, actor);
		assert.equal(response.status, status);
		if (status === 200) assert.ok(Array.isArray(await response.json()));
		else if (status === 403) assert.deepEqual(await response.json(), denied);
	}

	test.each(['unset', 'configured'] as const)('anonymous access uses base policies with root %s', async rootState => {
		try {
			await metaFixture({ rootUserId: rootState === 'unset' ? null : root.id });
			await policies({ canSearchUsers: true });
			await expectSearch(200);
			await policies({ canSearchUsers: false });
			// Observe the root change through HTTP too, so a stale configured root
			// cannot make the null-root case pass without testing that state.
			await expectSearch(rootState === 'unset' ? 403 : 200, root);
			await expectSearch(403);
			// Required policy precedes parameter validation.
			const missing = await post('users/search', {});
			assert.equal(missing.status, 403);
			assert.deepEqual(await missing.json(), denied);
			await policies({ canSearchUsers: true });
			await expectSearch(200);
			const invalid = await post('users/search', {});
			assert.equal(invalid.status, 400);
			assert.deepEqual(await invalid.json(), { error: {
				message: 'Invalid param.', code: 'INVALID_PARAM', id: '3d81ceae-475f-4600-b2a8-2bc116157532', kind: 'client', info: { param: '#/required', reason: "must have required property 'query'" },
			} });
		} finally { await restore(); }
	});

	test('only the actual root bypasses a denied policy; a regular user needs its role policy', async () => {
		let assigned = false;
		try {
			await metaFixture({ rootUserId: root.id });
			await policies({ canSearchUsers: false });
			await expectSearch(403, regular);
			await expectSearch(200, root);
			const invalidRoot = await post('users/search', {}, root);
			assert.equal(invalidRoot.status, 400);
			assert.deepEqual(errorIdentity(await invalidRoot.json()), { code: 'INVALID_PARAM', id: '3d81ceae-475f-4600-b2a8-2bc116157532', kind: 'client' });
			assert.equal((await post('admin/roles/assign', { userId: regular.id, roleId: searchRoleId }, operator)).status, 204);
			assigned = true;
			await expectSearch(200, regular);
			assert.equal((await post('admin/roles/unassign', { userId: regular.id, roleId: searchRoleId }, operator)).status, 204);
			assigned = false;
			await expectSearch(403, regular);
		} finally {
			try { if (assigned) await post('admin/roles/unassign', { userId: regular.id, roleId: searchRoleId }, operator); } finally { await restore(); }
		}
	});

	test.each([
		{ name: 'number', value: 17 },
		{ name: 'non-string array element', value: [false] },
		{ name: 'non-iterable object', value: {} },
	])('malformed uploadableFileTypes ($name) is accepted as settings but fails policy evaluation', async ({ value }) => {
		try {
			await policies({ canSearchUsers: true, uploadableFileTypes: value });
			await expectSearch(500, regular);
			const response = await post('users/search', searchRequest, regular);
			assert.equal(response.status, 500);
			assert.deepEqual(errorIdentity(await response.json()), { code: 'INTERNAL_ERROR', id: '5d37dbcb-891e-41ca-a3d6-e690c97775ac', kind: 'server' });
		} finally { await restore(); }
	});

	test.each([
		{ name: 'object with non-callable toString', value: { toString: 1 } },
		{ name: 'nested array with non-callable toString', value: [[{ toString: false }]] },
		{ name: 'throwing element after a NaN element', value: ['x', { toString: null }] },
	])('unevaluable numeric policy ($name) fails with structured500', async ({ value }) => {
		try {
			await policies({ rateLimitFactor: value, uploadableFileTypes: ['image/*'] });
			await waitForStatus('notes/delete', {}, regular, 500);
			const response = await post('notes/delete', {}, regular);
			assert.equal(response.status, 500);
			assert.deepEqual(errorIdentity(await response.json()), { code: 'INTERNAL_ERROR', id: '5d37dbcb-891e-41ca-a3d6-e690c97775ac', kind: 'server' });
		} finally { await restore(); }
	});

	test.each([
		{ name: 'plain object', value: {} },
		{ name: 'object with only a nested toString property', value: { nested: { toString: 1 } } },
	])('valid numeric coercion to NaN ($name) still reaches input validation', async ({ value }) => {
		try {
			await policies({ rateLimitFactor: value, uploadableFileTypes: ['image/*'] });
			await waitForStatus('notes/delete', {}, regular, 400);
			const response = await post('notes/delete', {}, regular);
			assert.equal(response.status, 400);
			assert.deepEqual(errorIdentity(await response.json()), { code: 'INVALID_PARAM', id: '3d81ceae-475f-4600-b2a8-2bc116157532', kind: 'client' });
		} finally { await restore(); }
	});

	test.each([
		{ name: 'array', value: ['image/png', 'image/jpeg'] },
		{ name: 'iterable string', value: 'xy' },
	])('valid iterable uploadableFileTypes ($name) keeps policy evaluation working', async ({ value }) => {
		try {
			await policies({ canSearchUsers: true, uploadableFileTypes: value });
			await expectSearch(200, regular);
		} finally { await restore(); }
	});
});
