import { test } from 'node:test';
import assert from 'node:assert/strict';
import { closeSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { publishFile } from './publish.mjs';

test('publishes a new addon without modifying the file held by a running process', t => {
	const root = mkdtempSync(join(tmpdir(), 'slacc-publish-'));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const source = join(root, 'new.node');
	const destination = join(root, 'slacc.node');
	writeFileSync(source, 'new compiled addon');
	writeFileSync(destination, 'previous compiled addon');
	const loadedAddon = openSync(destination, 'r');
	try {
		publishFile(source, destination);
		assert.equal(readFileSync(destination, 'utf8'), 'new compiled addon');
		assert.equal(readFileSync(loadedAddon, 'utf8'), 'previous compiled addon');
		assert.deepEqual(readdirSync(root).sort(), ['new.node', 'slacc.node']);
	} finally {
		closeSync(loadedAddon);
	}
});

test('preserves the deployed addon and cleans temporary files if copying fails', t => {
	const root = mkdtempSync(join(tmpdir(), 'slacc-publish-'));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	const destination = join(root, 'slacc.node');
	writeFileSync(destination, 'previous compiled addon');
	assert.throws(() => publishFile(join(root, 'missing.node'), destination));
	assert.equal(readFileSync(destination, 'utf8'), 'previous compiled addon');
	assert.deepEqual(readdirSync(root), ['slacc.node']);
});
