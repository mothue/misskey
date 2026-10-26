/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test } from 'bun:test';
import { businessNames, classify, coreNames, decodeHolds, validateCallbackGuard, registryNames } from './native-status.js';
import type { Hold } from './native-status.js';

const hold: Hold = { endpoint: 'held', kind: 'compatibility', reason: 'Boundaries differ.', evidence: 'evidence.md' };

test('registry order determines every row and counts are derived', () => {
	const names = registryNames(`
		export * as 'z-native' from './z.js';
		export * as 'unported' from './u.js';
		export * as 'a-native' from './a.js';
		export * as 'held' from './h.js';
	`);
	const inventory = classify(names, ['a-native'], ['z-native'], [hold]);
	expect(inventory.counts).toEqual({ total: 4, native: 2, ts: 1, held: 1 });
	expect(inventory.rows.map(row => [row.endpoint, row.status])).toEqual([
		['z-native', 'Native'], ['unported', 'TS'], ['a-native', 'Native'], ['held', 'Held (TS)'],
	]);
	expect(inventory.rows[3]?.hold).toEqual(hold);
	expect(classify([], [], [], []).counts).toEqual({ total: 0, native: 0, ts: 0, held: 0 });
});

test('Rust core names come from the authoritative dispatch selection table', () => {
	const source = `const CORE_ENDPOINTS: &[(&str, CoreEndpoint)] = &[
		("first", CoreEndpoint::First), /* nested /* comment */ */
		("second", CoreEndpoint::Second),
	];`;
	expect(coreNames(source)).toEqual(['first', 'second']);
	expect(coreNames('const CORE_ENDPOINTS: &[(&str, CoreEndpoint)] = &[];')).toEqual([]);
	expect(() => coreNames('const CORE_ENDPOINTS: &[(&str, CoreEndpoint)] = &[("a", other)];')).toThrow('Unsupported');
	expect(() => coreNames('const CORE_ENDPOINTS: &[(&str, CoreEndpoint)] = &[("a", CoreEndpoint::A), ("a", CoreEndpoint::B)];')).toThrow('Duplicate');
});

test('business allowlist is literal source data, not a second endpoint list', () => {
	expect(businessNames('const ACTIVE_READ_ENDPOINTS: &[&str] = &["a", /* hidden */ "b",];')).toEqual(['a', 'b']);
	expect(businessNames('const ACTIVE_READ_ENDPOINTS: &[&str] = &[];')).toEqual([]);
	expect(() => businessNames('const ACTIVE_READ_ENDPOINTS: &[&str] = &["a", other];')).toThrow('string literals');
	expect(() => businessNames('const ACTIVE_READ_ENDPOINTS: &[&str] = &["a", "a"];')).toThrow('Duplicate');
});

test('callback guard consumes bootstrap names only inside executeLegacy', () => {
	const guard = "if (this.rustBackendService.isNativeEndpoint(invocation.endpoint)) throw new Error('Native API cannot execute through TS');";
	expect(() => validateCallbackGuard(`class Service { executeLegacy() { ${guard} } }`)).not.toThrow();
	for (const source of [
		`class Service { other() { ${guard} } executeLegacy() {} }`,
		`class Service { executeLegacy() { ${guard.replace('invocation.endpoint', 'other')} } }`,
		`class Service { executeLegacy() { ${guard.replace('throw', 'return')} } }`,
		`class Service { executeLegacy() { ${guard} ${guard} } }`,
		"class Service { executeLegacy() { if (['a'].includes(invocation.endpoint)) throw new Error('Native API cannot execute through TS'); } }",
	]) expect(() => validateCallbackGuard(source)).toThrow('exactly one');
});

test('unknown, duplicate and overlapping classifications fail closed', () => {
	expect(() => classify(['a'], ['unknown'], [], [])).toThrow('Unknown');
	expect(() => classify(['a'], ['a'], ['a'], [])).toThrow('Duplicate');
	expect(() => classify(['a', 'a'], [], [], [])).toThrow('Duplicate');
	expect(() => classify(['held'], ['held'], [], [hold])).toThrow('Held endpoint is active');
	expect(() => classify([], [], [], [hold])).toThrow('Unknown');
	expect(() => classify(['held'], [], [], [hold, hold])).toThrow('Duplicate');
});

test('hold manifest validates runtime JSON and registry export forms are explicit', () => {
	expect(decodeHolds({ version: 1, holds: [hold] })).toEqual([hold]);
	for (const value of [null, {}, { version: 2, holds: [] }, { version: 1, holds: [{ ...hold, kind: 'other' }] }, { version: 1, holds: [{ ...hold, reason: '' }] }]) {
		expect(() => decodeHolds(value)).toThrow('Invalid native hold');
	}
	expect(() => registryNames("export { thing } from './thing.js';")).toThrow('Unsupported');
	expect(() => registryNames("export * as 'a' from './a.js'; export * as 'a' from './b.js';")).toThrow('Duplicate');
});
