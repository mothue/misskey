/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { test, expect } from 'bun:test';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const binaryName = `misskey-backend-rust${process.platform === 'win32' ? '.exe' : ''}`;

test('publishes a completed build atomically and preserves the last executable on failure', async () => {
 const { buildNative } = await import('./backend-rust.mjs');
 const root = await mkdtemp(join(tmpdir(), 'misskey-rust-build-'));
 try {
  const executable = join(root, 'built/bin', binaryName);
  await mkdir(join(root, 'built/bin'), { recursive: true });
  await writeFile(executable, 'previous');
  let invocation;
  await expect(buildNative({ root, run: async (command, args) => { invocation = { command, args }; throw new Error('compile failed'); } })).rejects.toThrow('compile failed');
  expect(await readFile(executable, 'utf8')).toBe('previous');
  await buildNative({ root, run: async () => {
   await mkdir(join(root, 'built/target/release'), { recursive: true });
   await writeFile(join(root, 'built/target/release', binaryName), 'new');
  } });
  expect(await readFile(executable, 'utf8')).toBe('new');
  expect(invocation.command).toBe('cargo');
  expect(invocation.args).toContain('--locked');
  expect(invocation.args).toContain('--release');
  expect(invocation.args).toContain(join(root, 'built/target'));
 } finally { await rm(root, { recursive: true, force: true }); }
});

test('does not publish when Cargo succeeds without producing the expected executable', async () => {
 const { buildNative } = await import('./backend-rust.mjs');
 const root = await mkdtemp(join(tmpdir(), 'misskey-rust-missing-'));
 try {
  await expect(buildNative({ root, run: async () => {} })).rejects.toThrow();
  await expect(readFile(join(root, 'built/bin', binaryName))).rejects.toThrow();
 } finally { await rm(root, { recursive: true, force: true }); }
});
