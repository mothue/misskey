/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test } from 'vitest';
import { apiEntityLayout, hydrateApiEntity } from './api-entity.js';
import type { EntityMetadata } from 'typeorm';
class Fixture { prototypeMethod(): boolean { return true; } }
const metadata = {
	columns: [
		{ propertyName: 'id', databaseName: 'id', type: 'varchar', isSelect: true, isNullable: false },
		{ propertyName: 'at', databaseName: 'db_at', type: 'timestamp with time zone', isSelect: true, isNullable: true },
		{ propertyName: 'aliases', databaseName: 'aliases', type: 'simple-array', isSelect: true, isNullable: true },
		{ propertyName: 'permission', databaseName: 'permission', type: 'varchar', isArray: true, isSelect: true, isNullable: false },
		{ propertyName: 'hidden', databaseName: 'hidden', type: 'varchar', isSelect: false, isNullable: true },
	],
	create: () => new Fixture(),
} as unknown as EntityMetadata;

test('entity layout preserves selected-column order and declared date/simple-array transforms', () => {
	expect(apiEntityLayout(metadata)).toEqual([
		{ propertyName: 'id', databaseName: 'id', kind: 'json' },
		{ propertyName: 'at', databaseName: 'db_at', kind: 'date' },
		{ propertyName: 'aliases', databaseName: 'aliases', kind: 'simple-array' },
		{ propertyName: 'permission', databaseName: 'permission', kind: 'json' },
	]);
});

test('snapshot hydration retains entity prototype and original timestamp without refetching', () => {
	const value = hydrateApiEntity(metadata, { id: 'id', at: -1, aliases: [], permission: ['read:account'] });
	expect(value).toBeInstanceOf(Fixture);
	expect(value.at).toEqual(new Date(-1));
	expect(value.aliases).toEqual([]);
	expect(Object.hasOwn(value, 'hidden')).toBe(false);
	expect(hydrateApiEntity(metadata, { id: 'id', permission: [] }, true)).toEqual({ id: 'id', permission: [] });
});

test('snapshot hydration rejects hidden, missing and mistyped columns', () => {
	const valid = { id: 'id', at: 1, aliases: [], permission: [] };
	for (const snapshot of [null, [], {}, { ...valid, hidden: 'secret' }, { ...valid, id: 1 }, { ...valid, at: '1970-01-01' }, { ...valid, permission: [1] }, { ...valid, aliases: '' }]) expect(() => hydrateApiEntity(metadata, snapshot)).toThrow();
	expect(() => hydrateApiEntity(metadata, { id: 'id', permission: [], at: 1 }, true)).toThrow();
});
