/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import console from 'node:console';
import { writeFile } from 'node:fs/promises';
import { cpus, tmpdir } from 'node:os';
import { performance } from 'node:perf_hooks';
import process from 'node:process';
import { parseArgs } from 'node:util';
import * as Bun from 'bun';
import { storageCases } from './bench-storage.mjs';

const { values, positionals } = parseArgs({
	args: process.argv.slice(2), allowPositionals: true,
	options: {
		list: { type: 'boolean' }, help: { type: 'boolean' },
		iterations: { type: 'string', default: '30' }, warmup: { type: 'string', default: '5' },
		directory: { type: 'string', default: tmpdir() }, output: { type: 'string' },
	},
});
if (values.help) {
	console.log('bun run bench:storage [FILTER ...] [--list] [--iterations 30] [--warmup 5]\n  [--directory PATH] [--output report.json]');
	process.exit(0);
}
const iterations = Number(values.iterations);
const warmup = Number(values.warmup);
assert(Number.isSafeInteger(iterations) && iterations >= 2, 'iterations must be an integer >= 2');
assert(Number.isSafeInteger(warmup) && warmup >= 0, 'warmup must be an integer >= 0');
const cases = storageCases(values.directory);
assert.equal(new Set(cases.map((item) => item.name)).size, cases.length, 'duplicate benchmark names');
const selected = cases.filter((item) => positionals.length
	? positionals.some((filter) => item.name.includes(filter))
	: values.list || item.name.startsWith('storage/'));
assert(selected.length, 'no benchmark cases match the filter');
if (values.list) {
	for (const item of selected) console.log(item.name);
	process.exit(0);
}
const report = { createdAt: new Date().toISOString(), bun: Bun.version, platform: process.platform,
	arch: process.arch, cpu: cpus()[0]?.model, iterations, warmup, results: [] };
let failed = false;
for (const item of selected) {
	console.log(`bench ${item.name} ...`);
	let context;
	try {
		context = await item.setup();
		for (let i = 0; i < warmup; i++) await context.run();
		const samplesMs = [];
		for (let i = 0; i < iterations; i++) {
			const start = performance.now();
			await context.run();
			samplesMs.push(performance.now() - start);
		}
		await context.verify();
		const sorted = samplesMs.toSorted((a, b) => a - b);
		const meanMs = samplesMs.reduce((sum, value) => sum + value, 0) / iterations;
		const medianMs = (sorted[Math.floor((iterations - 1) / 2)] + sorted[Math.floor(iterations / 2)]) / 2;
		const p95Ms = sorted[Math.ceil(iterations * 0.95) - 1];
		report.results.push({ name: item.name, status: 'pass', medianMs, p95Ms, meanMs, opsPerSecond: 1000 / meanMs,
			samplesMs, metadata: context.metadata });
		console.log(`  median ${medianMs.toFixed(3)} ms | p95 ${p95Ms.toFixed(3)} ms | ${(1000 / meanMs).toFixed(1)} ops/s`);
	} catch (error) {
		failed = true;
		// Do not print arbitrary driver errors that may expose connection strings or API tokens.
		console.error(`  FAILED (${error.name})${error.name === 'AssertionError' ? `: ${error.message}` : '; check connectivity, case inputs, and fixture data'}`);
		report.results.push({ name: item.name, status: 'fail', errorType: error.name });
	} finally {
		if (context) {
			try { await context.close(); } catch {
				failed = true;
				console.error(`  FAILED to clean up ${item.name}`);
			}
		}
	}
}
if (values.output) await writeFile(values.output, `${JSON.stringify(report, null, 2)}\n`);
if (failed) process.exitCode = 1;
