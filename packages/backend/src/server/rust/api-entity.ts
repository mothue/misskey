/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import type { EntityMetadata } from 'typeorm';

export type ApiEntityColumn = { propertyName: string; databaseName: string; kind: 'date' | 'simple-array' | 'json' };

function isDate(type: unknown): boolean {
	return type === Date || ['date', 'datetime', 'timestamp', 'timestamp with time zone', 'timestamp without time zone', 'timestamptz'].includes(String(type));
}

export function apiEntityLayout(metadata: EntityMetadata): ApiEntityColumn[] {
	return metadata.columns.filter(column => column.isSelect).map(column => ({
		propertyName: column.propertyName,
		databaseName: column.databaseName,
		kind: isDate(column.type) ? 'date' : column.type === 'simple-array' ? 'simple-array' : 'json',
	}));
}

function object(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Restore the authentication-time entity snapshot, without a second DB lookup. */
export function hydrateApiEntity(metadata: EntityMetadata, snapshot: unknown, reducedToken = false): Record<string, unknown> {
	if (!object(snapshot)) throw new Error('Invalid API entity snapshot');
	const columns = metadata.columns.filter(column => column.isSelect);
	const allowed = new Map(columns.map(column => [column.propertyName, column]));
	const names = Object.keys(snapshot);
	if (reducedToken) {
		if (names.length !== 2 || !names.includes('id') || !names.includes('permission')) throw new Error('Invalid reduced API token');
	} else if (columns.some(column => !Object.hasOwn(snapshot, column.propertyName))) {
		throw new Error('Incomplete API entity snapshot');
	}
	const restored: Record<string, unknown> = reducedToken ? {} : metadata.create();
	for (const name of names) {
		const column = allowed.get(name);
		if (!column) throw new Error('Unexpected API entity column');
		let value = snapshot[name];
		if (value === null) {
			if (!column.isNullable) throw new Error('Null API entity column');
		} else if (isDate(column.type)) {
			if (typeof value !== 'number' || !Number.isInteger(value)) throw new Error('Invalid API entity date');
			value = new Date(value);
		} else if (column.type === 'simple-array' || column.isArray) {
			if (!Array.isArray(value) || value.some(item => typeof item !== 'string')) throw new Error('Invalid API entity array');
		} else if (column.type === Boolean || column.type === 'boolean' || column.type === 'bool') {
			if (typeof value !== 'boolean') throw new Error('Invalid API entity boolean');
		} else if (column.type === Number || ['integer', 'int', 'int2', 'int4', 'smallint', 'real', 'float4', 'float8', 'double precision', 'bigint'].includes(String(column.type))) {
			if (typeof value !== 'number' || !Number.isFinite(value)) throw new Error('Invalid API entity number');
		} else if (column.type !== 'json' && column.type !== 'jsonb' && column.type !== 'simple-json') {
			if (typeof value !== 'string') throw new Error('Invalid API entity string');
		}
		Object.defineProperty(restored, name, { value, enumerable: true, configurable: true, writable: true });
	}
	return restored;
}
