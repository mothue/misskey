/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { describe, expect, test } from 'bun:test';
import RE2 from '../src/index.js';

describe('RE2 compatibility wrapper', () => {
	test('matches using the node-re2 constructor and test API', () => {
		expect(new RE2('bar').test('foobar')).toBe(true);
		expect(new RE2('bar').test('foo')).toBe(false);
	});

	test('converts case-insensitive, multiline, and dot-all flags', () => {
		expect(new RE2('Test', 'i').test('hogetesthuge')).toBe(true);
		expect(new RE2('^bar$', 'm').test('foo\nbar\nbaz')).toBe(true);
		expect(new RE2('foo.bar', 's').test('foo\nbar')).toBe(true);
	});

	test('implements the sticky flag as a match at the start of the input', () => {
		expect(new RE2('foo', 'y').test('foobar')).toBe(true);
		expect(new RE2('bar', 'y').test('foobar')).toBe(false);
	});

	test('accepts flags that do not affect one-shot boolean matching', () => {
		expect(new RE2('😀+', 'dgu').test('x😀😀y')).toBe(true);
	});

	test('rejects duplicate and unsupported flags', () => {
		expect(() => new RE2('a', 'ii')).toThrow(SyntaxError);
		expect(() => new RE2('a', 'v')).toThrow(SyntaxError);
	});

	test('retains RE2 syntax validation', () => {
		expect(() => new RE2('(')).toThrow();
		expect(() => new RE2('(?=a)')).toThrow();
		expect(() => new RE2('(a)\\1')).toThrow();
	});
});
