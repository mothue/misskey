/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { afterAll, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { compareResponseConformance, createResponseValidator, type ResponseSample } from './response-validation.js';

const directory = mkdtempSync(join(tmpdir(), 'misskey-response-types-'));
const typesPath = join(directory, 'types.ts');
writeFileSync(typesPath, `
type Envelope<T> = { 200: { content: { 'application/json': T } }; 204: { headers: {} } };
type Response<T> = { post: { responses: Envelope<T> } };
type Child = { id: string; next?: Child | null };
export type paths = {
 '/object': Response<{ required: string; nullable: number | null; optional?: boolean; readonly children: readonly Child[] }>;
 '/union': Response<({ kind: 'a'; value: number } | { kind: 'b'; value: string }) & { extra: true }>;
 '/index': Response<Record<string, number>>;
 '/empty-record': Response<Record<string, never>>;
 '/tuple': Response<readonly [string, number?, ...boolean[]]>;
 '/literal': Response<'a' | 1 | true | null>;
 '/opaque': Response<{ any: any; unknown: unknown; object: object; empty: {} }>;
 '/unsupported': Response<Date>;
 '/template-index': Response<{ [key: \`data-\${string}\`]: number }>;
 '/never': Response<never>;
};
`);
afterAll(() => rmSync(directory, { recursive: true, force: true }));

const validator = createResponseValidator({ typesPath });
const generated = createResponseValidator();
const sample = (endpoint: string, body: unknown, extra: Partial<ResponseSample> = {}): ResponseSample => ({ endpoint, status: 200, body, ...extra });
const object = { required: 'ok', nullable: null, children: [{ id: 'first', next: { id: 'second', next: null } }] };

describe('generated response type validation', () => {
	test('uses the actual SDK endpoint/status response types', () => {
		expect(generated.endpoints).toContain('notes/show-partial-bulk');
		expect(generated.validate(sample('ping', { pong: 1 })).status).toBe('PASS');
		expect(generated.validate(sample('ping', { pong: '1' })).status).toBe('FAIL');
		expect(generated.validate(sample('test', { required: true, nullableDefault: null })).status).toBe('PASS');
		expect(generated.validate(sample('test', { required: true })).status).toBe('FAIL');
		expect(generated.validate(sample('notes/show-partial-bulk', [{ id: 'note', reactions: { '👍': 1 }, reactionEmojis: {} }])).status).toBe('PASS');
		expect(generated.validate(sample('notes/show-partial-bulk', [{ id: 'note', reactions: { '👍': '01' }, reactionEmojis: {} }])).status).toBe('FAIL');
	});

	test('requires required and nullable fields, while accepting omitted optional fields', () => {
		expect(validator.validate(sample('object', object)).status).toBe('PASS');
		expect(validator.validate(sample('object', { ...object, optional: false })).status).toBe('PASS');
		const { nullable: omitted, ...missing } = object;
		expect(omitted).toBeNull();
		expect(validator.validate(sample('object', missing)).diagnostics.some(d => d.code === 'missing-property')).toBe(true);
		expect(validator.validate(sample('object', { ...object, optional: null })).status).toBe('FAIL');
		expect(validator.validate(sample('object', { ...object, optional: undefined })).status).toBe('FAIL');
	});

	test('walks recursive readonly arrays and rejects an invalid nested field', () => {
		expect(validator.validate(sample('object', { ...object, children: [{ id: 'first', next: { id: 4 } }] })).status).toBe('FAIL');
		expect(validator.validate(sample('object', { ...object, children: {} })).status).toBe('FAIL');
	});

	test('checks all intersection members and selects discriminated union branches', () => {
		expect(validator.validate(sample('union', { kind: 'b', value: 'text', extra: true })).status).toBe('PASS');
		expect(validator.validate(sample('union', { kind: 'b', value: 3, extra: true })).status).toBe('FAIL');
		expect(validator.validate(sample('union', { kind: 'a', value: 3, extra: false })).status).toBe('FAIL');
	});

	test('checks map values, including never index signatures', () => {
		expect(validator.validate(sample('index', { a: 1, b: -2 })).status).toBe('PASS');
		expect(validator.validate(sample('index', { a: '1' })).status).toBe('FAIL');
		expect(validator.validate(sample('empty-record', {})).status).toBe('PASS');
		expect(validator.validate(sample('empty-record', { unexpected: null })).status).toBe('FAIL');
	});

	test('checks tuple required/optional/rest element types and lengths', () => {
		expect(validator.validate(sample('tuple', ['first'])).status).toBe('PASS');
		expect(validator.validate(sample('tuple', ['first', 2, true, false])).status).toBe('PASS');
		expect(validator.validate(sample('tuple', [])).status).toBe('FAIL');
		expect(validator.validate(sample('tuple', ['first', 2, 'false'])).status).toBe('FAIL');
	});

	test('checks literal unions without coercion', () => {
		for (const value of ['a', 1, true, null]) expect(validator.validate(sample('literal', value)).status).toBe('PASS');
		for (const value of ['1', false, {}, []]) expect(validator.validate(sample('literal', value)).status).toBe('FAIL');
	});

	test('records SDK any/unknown/object/empty type opacity explicitly', () => {
		const result = validator.validate(sample('opaque', { any: [1, null], unknown: { a: true }, object: [], empty: 1 }));
		expect(result.status).toBe('PASS');
		expect(result.opaque).toHaveLength(4);
		expect(validator.validate(sample('opaque', { any: null, unknown: null, object: null, empty: null })).status).toBe('FAIL');
	});

	test('rejects unsupported types and never instead of treating them as any', () => {
		expect(validator.validate(sample('unsupported', {})).diagnostics.some(d => d.code === 'unsupported-type')).toBe(true);
		expect(validator.validate(sample('never', {})).status).toBe('FAIL');
	});

	test('fails closed for unsupported template-literal index signatures', () => {
		for (const body of [{ 'data-count': 'wrong' }, {}]) {
			const result = validator.validate(sample('template-index', body));
			expect(result.status).toBe('FAIL');
			expect(result.diagnostics.some(d => d.code === 'unsupported-index')).toBe(true);
		}
	});

	test('rejects undeclared endpoints, statuses and methods', () => {
		expect(validator.validate(sample('missing', {})).status).toBe('FAIL');
		expect(validator.validate(sample('object', object, { status: 201 })).status).toBe('FAIL');
		expect(validator.validate(sample('object', object, { method: 'get' })).status).toBe('FAIL');
	});

	test('distinguishes zero response bytes from a JSON null body', () => {
		expect(validator.validate(sample('object', null, { status: 204, rawBody: '' })).status).toBe('PASS');
		expect(validator.validate(sample('object', null, { status: 204, rawBody: new Uint8Array() })).status).toBe('PASS');
		expect(validator.validate(sample('object', null, { status: 204, rawBody: 'null' })).status).toBe('FAIL');
		expect(validator.validate(sample('object', null, { status: 204 })).status).toBe('FAIL');
		expect(generated.validate(sample('endpoint', null, { rawBody: 'null' })).status).toBe('PASS');
		expect(generated.validate(sample('endpoint', null, { rawBody: '' })).status).toBe('FAIL');
	});

	test('rejects non-JSON, sparse, cyclic and nonfinite inputs even behind opaque SDK types', () => {
		const cyclic: Record<string, unknown> = {}; cyclic.self = cyclic;
		for (const value of [NaN, Infinity, undefined, 1n, () => null, new Date(), Array(1), cyclic]) {
			expect(validator.validate(sample('opaque', { any: value, unknown: null, object: {}, empty: 1 })).status).toBe('FAIL');
		}
	});

	test('fails clearly when input exceeds traversal limits', () => {
		const limited = createResponseValidator({ typesPath, maxDepth: 2, maxNodes: 20 });
		expect(limited.validate(sample('object', object)).diagnostics.some(d => d.code === 'resource-limit')).toBe(true);
	});

	test('does not erase semantic differences when the SDK allows both shapes', () => {
		const baseline = sample('object', object), candidate = sample('object', { ...object, optional: true });
		const result = compareResponseConformance(validator, baseline, candidate);
		expect(result.baseline.status).toBe('PASS');
		expect(result.candidate.status).toBe('PASS');
		expect(result.parity).toBe('FAIL');
		expect(result.status).toBe('FAIL');
	});

	test('reports identical inherited SDK violations as BASELINE, with parity still separate', () => {
		const legacy = sample('empty-record', { inheritedValue: 1 });
		const inherited = compareResponseConformance(validator, legacy, structuredClone(legacy));
		expect(inherited.status).toBe('BASELINE');
		expect(inherited.parity).toBe('PASS');
		expect(inherited.baseline.status).toBe('FAIL');
		expect(inherited.candidate.status).toBe('FAIL');
		expect(compareResponseConformance(validator, sample('index', { a: 1 }), sample('index', { a: '1' })).status).toBe('FAIL');
		expect(compareResponseConformance(validator, sample('index', { a: 1 }), sample('index', { a: 1 })).status).toBe('PASS');
	});
});
