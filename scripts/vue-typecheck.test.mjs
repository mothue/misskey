/*
 * SPDX-FileCopyrightText: misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

test('Vue typecheck checks templates and accepts valid components under Bun', () => {
	const fixture = mkdtempSync(join(tmpdir(), 'misskey-vue-typecheck-'));
	try {
		symlinkSync(fileURLToPath(new URL('../packages/frontend/node_modules/', import.meta.url)), join(fixture, 'node_modules'), 'dir');
		writeFileSync(join(fixture, 'tsconfig.json'), JSON.stringify({
			compilerOptions: { strict: true, skipLibCheck: true, types: [], target: 'ESNext', module: 'ESNext', moduleResolution: 'Bundler' },
			include: ['*.vue'],
		}));
		const check = () => spawnSync(process.execPath, [
			fileURLToPath(new URL('./vue-typecheck.cjs', import.meta.url)),
			'--noEmit', '--project', join(fixture, 'tsconfig.json'),
		], { cwd: fileURLToPath(new URL('../packages/frontend/', import.meta.url)), encoding: 'utf8', timeout: 60000 });
		writeFileSync(join(fixture, 'Probe.vue'), '<script setup lang="ts">const value: string = "hello";</script><template>{{ value.toFixed() }}</template>');
		const invalid = check();
		assert.equal(invalid.status, 2, invalid.stdout + invalid.stderr);
		assert.match(invalid.stdout + invalid.stderr, /toFixed/);
		writeFileSync(join(fixture, 'Probe.vue'), '<script setup lang="ts">const value: string = "hello";</script><template>{{ value.toUpperCase() }}</template>');
		const valid = check();
		assert.equal(valid.status, 0, valid.stdout + valid.stderr);
	} finally {
		rmSync(fixture, { recursive: true, force: true });
	}
});
