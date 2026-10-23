/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { expect, test } from 'vitest';
import { apiDecodeString, apiDecodeValue, apiEncodeValue, apiStringifyFrame } from './api-wire.js';

test('API framing preserves JavaScript scalar boundaries and UTF-16 code units', () => {
	for (const value of [undefined, null, true, false, 0, -0, 0.1, Number.MIN_VALUE, Number.MAX_VALUE, Infinity, -Infinity, NaN, '', '\ud800', '\udc00', '😀\u0000']) {
		expect(Object.is(apiDecodeValue(JSON.parse(JSON.stringify(apiEncodeValue(value)))), value)).toBe(true);
	}
	expect(() => apiDecodeString([0x10000])).toThrow();
	expect(() => apiDecodeString(['65'])).toThrow();
});

test('API framing retains own undefined, property ordering and prototype-sensitive names', () => {
	const input = JSON.parse('{"z":1,"__proto__":{"danger":true},"constructor":"x","0":0,"a":null,"\\ud800":[]}');
	input.missing = undefined;
	const output = apiDecodeValue(apiEncodeValue(input));
	expect(output).toEqual(input);
	expect(Object.keys(output as object)).toEqual(Object.keys(input));
	expect(Object.prototype.hasOwnProperty.call(output, '__proto__')).toBe(true);
	expect(Object.prototype.hasOwnProperty.call(output, 'missing')).toBe(true);
	expect(Object.getPrototypeOf(output)).toBe(Object.prototype);
	expect(({} as Record<string, unknown>).danger).toBeUndefined();
});

test('API framing keeps nested values without recursive stack exhaustion and rejects cycles', () => {
	let input: unknown = 'leaf';
	for (let index = 0; index < 12_000; index++) input = [input];
	let output = apiDecodeValue(JSON.parse(apiStringifyFrame(apiEncodeValue(input))));
	for (let index = 0; index < 12_000; index++) output = (output as unknown[])[0];
	expect(output).toBe('leaf');
	const cycle: unknown[] = [];
	cycle.push(cycle);
	expect(() => apiEncodeValue(cycle)).toThrow('Cyclic');
	const shared = { key: 1 };
	expect(apiDecodeValue(apiEncodeValue([shared, shared]))).toEqual([shared, shared]);
});

test('internal frame serialization retains JSON omission/order while rejecting cycles', () => {
	const value = { a: undefined, b: [undefined, -0, NaN, '\ud800'], c: { z: true, x: null } };
	expect(apiStringifyFrame(value)).toBe(JSON.stringify(value));
	const cycle: unknown[] = [];
	cycle.push(cycle);
	expect(() => apiStringifyFrame(cycle)).toThrow('Cyclic');
});

test('API decoder rejects malformed frames rather than trusting type assertions', () => {
	for (const value of [
		null, [], {}, { t: 'missing' }, { t: 'undefined', v: 0 }, { t: 'null', extra: true },
		{ t: 'boolean', v: 'true' }, { t: 'number', v: 1 }, { t: 'number', v: '' }, { t: 'number', v: '01' }, { t: 'number', v: '0x10' },
		{ t: 'string', v: [false] }, { t: 'string', v: [-1] }, { t: 'string', v: [1.1] },
		{ t: 'array', v: {} }, { t: 'array', v: [{ t: 'number', v: null }] },
		{ t: 'object', v: [[[97], { t: 'null' }], [[97], { t: 'null' }]] },
		{ t: 'object', v: [['a', { t: 'null' }]] }, { t: 'object', v: [[[97]]] },
	]) expect(() => apiDecodeValue(value)).toThrow();
	for (const value of [1n, Symbol('x'), () => {}]) expect(() => apiEncodeValue(value)).toThrow();
});
