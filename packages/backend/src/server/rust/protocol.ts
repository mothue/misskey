/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
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
	if (!Array.isArray(value) || !value.every(x => typeof x === 'number' && Number.isInteger(x) && x >= 0 && x <= 65535)) throw new Error('Invalid UTF16');
	return value.map(x => String.fromCharCode(x)).join('');
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
