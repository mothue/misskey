/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { phase, summary } from '../backend/runner.js';
import { completedOrder } from './summary.js';
import { ports } from '../backend/ports.js';
import { scenarios as registry } from '../backend/registry.js';
import { waitForHealth } from '../backend/lifecycle.js';
const root = resolve(import.meta.dirname, '../../..');
const work = resolve(root, 'built/ts-to-rs');
const options = { requests: 1000, warmup: 100, concurrency: 8, timeout: 10000 };
const manifest = await Bun.file(resolve(root, '.benchmark/manifest.json')).json();
const results: { label: 'baseline' | 'candidate'; pair: number; startupMs: number; scenarios: unknown[] }[] = [];
const mode = process.argv[2];
assert(['measure', 'candidate-http', 'smoke'].includes(mode), 'usage: paired.ts measure|candidate-http|smoke');

async function memory(pid: number) {
 const ps = Bun.spawn(['ps', '-eo', 'pid=,ppid=,rss=,comm='], { stdout: 'pipe' });
 const rows = (await new Response(ps.stdout).text()).trim().split('\n').map(row => row.trim().split(/\s+/));
 await ps.exited;
 return { tsPid: pid, rustPids: rows.filter(row => Number(row[1]) === pid && row.slice(3).join(' ').includes('misskey-backend-rust')).map(row => Number(row[0])), tsKiB: Number(rows.find(row => Number(row[0]) === pid)?.[2] ?? 0), rustKiB: rows.filter(row => Number(row[1]) === pid && row.slice(3).join(' ').includes('misskey-backend-rust')).reduce((sum, row) => sum + Number(row[2]), 0) };
}

async function run(label: 'baseline' | 'candidate', pair: number) {
 const codeRoot = label === 'baseline' ? resolve(work, 'baseline') : root;
 let ready = false;
 const start = performance.now();
 const child = Bun.spawn([process.execPath, resolve(codeRoot, 'packages/backend/built-benchmark/entry.js'), 'server'], {
  cwd: resolve(codeRoot, 'packages/backend'), stdout: Bun.file(resolve(work, `${label}-${pair}.log`)), stderr: 'inherit',
  env: { ...process.env, NODE_ENV: 'benchmark', MISSKEY_CONFIG_JSON: resolve(root, '.benchmark/config.json'), MISSKEY_STORAGE_ROOT: resolve(codeRoot, '.benchmark/storage'), MK_NO_DAEMONS: '1', MISSKEY_BENCH_MIGRATIONS: resolve(root, '.benchmark/migration-repo/packages/backend/migration') },
  ipc(message: { type?: string }) { if (message.type === 'ready') ready = true; },
 });
 try {
  await waitForHealth(child, 'http://127.0.0.1:61912/healthz/', () => ready, new AbortController().signal);
  const startupMs = performance.now() - start;
  if (mode === 'candidate-http') {
   const capture = Bun.spawn([process.execPath, resolve(root, 'scripts/benchmarks/ports/capture.ts'), 'http://127.0.0.1:61912', resolve(work, 'candidate-http-expanded.json'), resolve(work, 'baseline-http-expanded.json')], { stdout: 'inherit', stderr: 'inherit' });
   assert.equal(await capture.exited, 0, 'candidate differential'); return;
  }
  const scenarios = [];
  for (const scenario of mode === 'smoke' ? registry.filter(item => ['timeline/global', 'note/show', 'user/show'].includes(item.name)) : ports) {
   const warmup = summary(await phase(scenario, manifest, 'http://127.0.0.1:61912', options, 100, 'warmup', new AbortController().signal));
   const measure = summary(await phase(scenario, manifest, 'http://127.0.0.1:61912', options, 1000, 'measure', new AbortController().signal));
   assert.equal(warmup.counts.success, 100); assert.equal(measure.counts.success, 1000);
   scenarios.push({ name: scenario.name, warmup, measure, memory: await memory(child.pid) });
  }
  results.push({ label, pair, startupMs, scenarios });
  await Bun.write(resolve(work, mode === 'smoke' ? 'smoke-results.json' : 'paired-results.json'), JSON.stringify({ options, order: completedOrder(results), memory: 'ps RSS KiB snapshots, not peak; Rust direct-child sum', results }, null, 2));
 } finally {
  child.kill('SIGTERM'); await Promise.race([child.exited, Bun.sleep(8000)]);
  if (child.exitCode === null) child.kill('SIGKILL'); await child.exited;
 }
}

if (mode === 'candidate-http') await run('candidate', 0);
else if (mode === 'smoke') { await run('baseline', 0); await run('candidate', 0); } else for (let pair = 0; pair < 3; pair++) for (const label of pair === 1 ? ['candidate', 'baseline'] as const : ['baseline', 'candidate'] as const) await run(label, pair + 1);
