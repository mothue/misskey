/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { describe, expect, test } from 'bun:test';
import { compareCaptures, decodeCaptures } from './differential.js';
import { createResponseValidator } from './response-validation.js';

const validator = createResponseValidator();
const capture = (body: unknown) => decodeCaptures([{ id: 'test', endpoint: 'test', method: 'post', status: 200, body, rawBody: JSON.stringify(body) }]);

describe('captured native differential', () => {
	test('detects omitted null in both the schema and semantic comparison', () => {
		const baseline = capture({ required: true, nullableDefault: null });
		const candidate = capture({ required: true });
		const [left] = baseline, [right] = candidate;
		if (!left || !right) throw new Error('Missing test capture');
		expect(validator.validate(left.sample).status).toBe('PASS');
		expect(validator.validate(right.sample).status).toBe('FAIL');
		const result = compareCaptures(validator, baseline, candidate, ['test']);
		expect(result.status).toBe('FAIL');
		expect(result.rows[0]?.parity).toBe('FAIL');
	});
	test('also detects omission of an optional value allowed by the SDK', () => {
		const baseline = capture({ required: true, nullableDefault: null, string: 'hello' });
		const candidate = capture({ required: true, nullableDefault: null });
		const [left] = baseline, [right] = candidate;
		if (!left || !right) throw new Error('Missing test capture');
		expect(validator.validate(left.sample).status).toBe('PASS');
		expect(validator.validate(right.sample).status).toBe('PASS');
		expect(compareCaptures(validator, baseline, candidate, ['test']).status).toBe('FAIL');
	});
	test('requires a success pair for every selected native endpoint', () => {
		const rows = capture({ required: true });
		expect(compareCaptures(validator, rows, rows, ['test', 'endpoints']).coverage).toEqual({ status: 'FAIL', required: 2, missing: ['endpoints'] });
	});
	test('does not trust body values that disagree with actual response bytes', () => {
		expect(() => decodeCaptures([{ id: 'test', endpoint: 'test', status: 200, body: { required: true }, rawBody: '{"required":false}' }])).toThrow('disagrees');
	});
	test('validates actual clock intervals and all other fields', () => {
		const rows = (pong: number, extra = false) => decodeCaptures([{ id: 'clock', endpoint: 'ping', status: 200, body: { pong, extra }, rawBody: JSON.stringify({ pong, extra }), clockWindowMs: [100, 200] }]);
		expect(compareCaptures(validator, rows(110), rows(180), ['ping']).status).toBe('PASS');
		expect(compareCaptures(validator, rows(110), rows(180, true), ['ping']).status).toBe('FAIL');
		expect(() => compareCaptures(validator, rows(110), rows(500), ['ping'])).toThrow('outside');
	});
	test('reports identical SDK violations as BASELINE without accepting a semantic change', () => {
		const rows = capture({ required: 'wrong' });
		expect(compareCaptures(validator, rows, rows, ['test']).status).toBe('BASELINE');
		expect(compareCaptures(validator, rows, capture({ required: 'different' }), ['test']).status).toBe('FAIL');
	});
});
