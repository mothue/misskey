/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { copyFile, chmod, mkdir, rename, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { execa } from 'execa';

export async function buildNative({ root = resolve(import.meta.dirname, '../../packages/backend-rust'), run = (command, args, options) => execa(command, args, options) } = {}) {
 // Fail before Cargo/publication when the authoritative TS policy contract
 // no longer matches the generated Rust aggregation descriptor.
 await run(process.execPath, [resolve(import.meta.dirname, '../../packages/backend-rust/scripts/generate-policy-aggregation.ts'), '--check'], { cwd: root, stdio: 'inherit' });
 const target = join(root, 'built/target');
 await run('cargo', ['build', '--locked', '--release', '--manifest-path', join(root, 'Cargo.toml'), '--target-dir', target], { cwd: root, stdio: 'inherit' });
 const name = `misskey-backend-rust${process.platform === 'win32' ? '.exe' : ''}`;
 const destination = join(root, 'built/bin', name);
 const temporary = `${destination}.${process.pid}.tmp`;
 await mkdir(join(root, 'built/bin'), { recursive: true });
 try {
  await copyFile(join(target, 'release', name), temporary);
  await chmod(temporary, 0o755);
  await rename(temporary, destination);
 } finally { await rm(temporary, { force: true }); }
}

if (import.meta.main) await buildNative();
