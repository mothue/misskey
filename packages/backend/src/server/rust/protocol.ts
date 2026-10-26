/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import type { Schema } from '@/misc/json-schema.js';

export interface CatalogEndpoint {
	readonly nameUtf16: readonly number[];
	readonly properties: readonly {
		readonly nameUtf16: readonly number[];
		readonly type: NonNullable<Schema['type']> | null;
	}[];
}

export function object(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function envelope(value: unknown, op: string): Record<string, unknown> {
	if (!object(value) || value.v !== 1 || value.op !== op) throw new Error('Invalid RPC envelope');
	return value;
}

export function decodePing(value: unknown): { pong: number } {
	const result = envelope(value, 'ping');
	if (typeof result.pong !== 'number' || !Number.isSafeInteger(result.pong)) throw new Error('Invalid clock');
	return { pong: result.pong };
}

export function decodeOnlineUsersCount(value: unknown): { count: number } {
	const { count } = envelope(value, 'get-online-users-count');
	// PostgreSQL COUNT is a nonnegative int8. Existing pg/TypeORM converts it to
	// Number, including rounding outside the safe integer range; preserve that.
	if (typeof count !== 'number' || !Number.isInteger(count) || count < 0 || count > Number(9223372036854775807n)) throw new Error('Invalid count');
	return { count };
}
export type TestResult = Record<string, unknown> & { required: boolean; nullableDefault: string | null; id?: string; string?: string; default?: string };

function testObject(value: unknown): value is TestResult {
	return object(value) && typeof value.required === 'boolean' && (value.nullableDefault === null || typeof value.nullableDefault === 'string') && ['id', 'string', 'default'].every(key => !(key in value) || typeof value[key] === 'string');
}

export function decodeTest(value: unknown, sent: string): TestResult {
	const result = envelope(value, 'test');
	if (result.resultJson !== sent) throw new Error('Invalid echo');
	const restored: unknown = JSON.parse(sent);
	if (!testObject(restored)) throw new Error('Invalid test object');
	return restored;
}
export function encodeUtf16(value: string): number[] {
	return Array.from({ length: value.length }, (_, i) => value.charCodeAt(i));
}

function decodeUtf16(value: unknown): string {
	if (!Array.isArray(value)) throw new Error('Invalid UTF16');
	return Array.from(value, (unit: unknown) => {
		if (typeof unit !== 'number' || !Number.isInteger(unit) || unit < 0 || unit > 65535) throw new Error('Invalid UTF16');
		return String.fromCharCode(unit);
	}).join('');
}

export function decodeEndpoints(value: unknown): string[] {
	const result = envelope(value, 'endpoints');
	if (!Array.isArray(result.endpointsUtf16)) throw new Error('Invalid endpoints');
	return Array.from(result.endpointsUtf16, decodeUtf16);
}

export function decodeEndpoint(value: unknown): { params: { name: string; type: string }[] } | null {
	const result = envelope(value, 'endpoint').result;
	if (result === null) return null;
	if (!object(result) || !Array.isArray(result.params)) throw new Error('Invalid endpoint');
	return { params: result.params.map((param: unknown) => {
		if (!object(param) || typeof param.type !== 'string' || !/^[A-Za-z]+$/.test(param.type)) throw new Error('Invalid parameter');
		return { name: decodeUtf16(param.nameUtf16), type: param.type };
	}) };
}

/** Readiness is trusted only after every advertised name belongs to our catalog. */
export function decodeReady(value: unknown, catalog: readonly CatalogEndpoint[]): { port: number; nativeEndpoints: ReadonlySet<string> } {
	if (!object(value) || value.v !== 1 || typeof value.port !== 'number'
		|| !Number.isInteger(value.port) || value.port < 1 || value.port > 65535
		|| !Array.isArray(value.nativeEndpoints)
		|| Object.keys(value).some(key => !['v', 'port', 'nativeEndpoints'].includes(key))) {
		throw new Error('Invalid Rust readiness');
	}
	const known = new Set(catalog.map(endpoint => decodeUtf16(endpoint.nameUtf16)));
	const nativeEndpoints = new Set<string>();
	const selected: readonly unknown[] = value.nativeEndpoints;
	for (const name of selected) {
		if (typeof name !== 'string' || !known.has(name) || nativeEndpoints.has(name)) {
			throw new Error('Invalid native endpoint selection');
		}
		nativeEndpoints.add(name);
	}
	return { port: value.port, nativeEndpoints };
}
