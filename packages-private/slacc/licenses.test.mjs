import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { renderNotices } from './licenses.mjs';

function fixture(t) {
	const root = mkdtempSync(join(tmpdir(), 'slacc-licenses-'));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	mkdirSync(join(root, 'third_party'), { recursive: true });
	writeFileSync(join(root, 'Cargo.toml'), '');
	const pkg = { name: 'example', version: '1.0.0', license: 'MIT', manifest_path: join(root, 'Cargo.toml'), source: 'registry+https://github.com/rust-lang/crates.io-index', repository: 'https://example.com/source' };
	return { root, pkg, policy: { licenses: { MIT: 'MIT' }, supplements: {} } };
}

test('retains original copyright text and licenses embedded by native dependencies', t => {
	const { root, pkg, policy } = fixture(t);
	writeFileSync(join(root, 'LICENSE'), 'Copyright Original Author\nMIT terms');
	writeFileSync(join(root, 'third_party', 'LICENSE'), 'Copyright Native Author\nBSD terms');
	const result = renderNotices({ packages: [pkg] }, policy, root, 'lock-hash');
	assert.match(result, /Copyright Original Author\nMIT terms/);
	assert.match(result, /third_party\/LICENSE\nCopyright Native Author\nBSD terms/);
	assert.match(result, /example 1\.0\.0/);
	assert.ok(!result.includes(root));
});

test('refuses a crate without license texts', t => {
	const { root, pkg, policy } = fixture(t);
	assert.throws(() => renderNotices({ packages: [pkg] }, policy, root, 'hash'), /No license text.*example/);
});

test('refuses unreviewed license expressions', t => {
	const { root, pkg, policy } = fixture(t);
	assert.throws(() => renderNotices({ packages: [{ ...pkg, license: 'GPL-3.0-only' }] }, policy, root, 'hash'), /Unreviewed license/);
});

test('uses version-pinned supplements with provenance and refuses stale supplements', t => {
	const { root, pkg, policy } = fixture(t);
	writeFileSync(join(root, 'upstream.txt'), 'Copyright Upstream\nMIT terms');
	policy.supplements['example@1.0.0'] = { license: 'MIT', files: [{ path: 'upstream.txt', source: 'https://example.com/pinned/LICENSE' }] };
	const result = renderNotices({ packages: [pkg] }, policy, root, 'hash');
	assert.match(result, /https:\/\/example.com\/pinned\/LICENSE/);
	assert.match(result, /Copyright Upstream/);
	assert.throws(() => renderNotices({ packages: [{ ...pkg, version: '1.0.1' }] }, policy, root, 'hash'), /No license text/);
});

test('produces identical notices regardless of Cargo metadata ordering', t => {
	const { root, pkg, policy } = fixture(t);
	writeFileSync(join(root, 'LICENSE'), 'MIT terms');
	const other = { ...pkg, name: 'another' };
	assert.equal(renderNotices({ packages: [pkg, other] }, policy, root, 'hash'), renderNotices({ packages: [other, pkg] }, policy, root, 'hash'));
});
