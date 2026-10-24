/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { test, expect } from 'bun:test';
import { comparable } from './http.js';
test('normalization retains all nonvolatile headers and exact body order', () => {
 expect(comparable({ status: 200, headers: { date: 'today', 'content-type': 'application/json' }, body: '{"b":1,"a":2}' })).toEqual({ status: 200, headers: { 'content-type': 'application/json' }, body: '{"b":1,"a":2}' });
});
import { ports } from '../backend/ports.js';
test('port scenarios reject malformed successful responses', () => {
 for (const scenario of ports) expect(() => scenario.validate(null, {} as never)).toThrow();
});
test('header normalization ignores casing and insertion order only', () => {
 expect(comparable({ status: 200, headers: { Z: '1', Date: 'today', A: '2' }, body: 'a' })).toEqual(comparable({ status: 200, headers: { a: '2', z: '1' }, body: 'a' }));
});
