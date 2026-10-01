/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import process from 'node:process';
import * as Bun from 'bun';
import { expect, test } from 'bun:test';

const script = new URL('./bench.mjs', import.meta.url).pathname;

async function run(args, env = {}) {
	const child = Bun.spawn([process.execPath, script, ...args], {
		stdout: 'pipe', stderr: 'pipe', env: { ...process.env, ...env },
	});
	const [stdout, stderr, code] = await Promise.all([
		new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
	]);
	return { stdout, stderr, code };
}

test('lists only matching cases and rejects an unknown filter', async () => {
	const list = await run(['storage/copy/4KiB', '--list']);
	expect(list.code).toBe(0);
	expect(list.stdout.trim().split('\n')).toEqual(['storage/copy/4KiB/fs-sync', 'storage/copy/4KiB/fs-async', 'storage/copy/4KiB/bun-write']);
	expect((await run(['does-not-exist', '--list'])).code).not.toBe(0);
});

test('storage smoke run saves raw measurements and cleans temporary fixtures', async () => {
	const root = await mkdtemp(join(tmpdir(), 'bench-test-'));
	try {
		const output = join(root, 'report.json');
		const result = await run(['storage/buffer/4KiB', '--iterations', '2', '--warmup', '0', '--directory', root, '--output', output]);
		expect(result.code).toBe(0);
		const report = JSON.parse(await readFile(output, 'utf8'));
		expect(report.results).toHaveLength(3);
		for (const item of report.results) {
			expect(item.status).toBe('pass');
			expect(item.samplesMs).toHaveLength(2);
			expect(item.medianMs).toBeGreaterThan(0);
		}
		expect(Array.from(new Bun.Glob('misskey-bench-*').scanSync(root))).toEqual([]);
	} finally { await rm(root, { recursive: true, force: true }); }
});
