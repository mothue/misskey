/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { buffer } from 'node:stream/consumers';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { mockDeep } from 'vitest-mock-extended';
import { InternalStorageService } from '@/core/InternalStorageService.js';
import type { Config } from '@/config.js';

describe('InternalStorageService', () => {
	let rootDir: string;
	let storage: InternalStorageService;
	const url = 'https://misskey.example';
	const binary = Buffer.from([0x00, 0xff, 0x80, 0x0a, 0x0d, 0x41]);

	beforeEach(async () => {
		rootDir = await mkdtemp(join(tmpdir(), 'misskey-internal-storage-'));
		storage = new InternalStorageService(mockDeep<Config>({ rootDir, url }));
	});

	afterEach(async () => {
		await rm(rootDir, { recursive: true, force: true });
	});

	test('resolves keys under the configured files directory', () => {
		expect(storage.resolvePath('file.bin')).toBe(join(rootDir, 'files', 'file.bin'));
	});

	// Await saves so these contracts also apply to a future asynchronous implementation.
	describe.each(['path', 'buffer'] as const)('save from %s', (source) => {
		async function save(key: string, data: Buffer) {
			if (source === 'buffer') return await storage.saveFromBuffer(key, data);
			const srcPath = join(rootDir, 'source.bin');
			await writeFile(srcPath, data);
			const result = await storage.saveFromPath(key, srcPath);
			expect(await readFile(srcPath)).toEqual(data);
			return result;
		}

		test('creates the storage directory and returns a URL after binary data is readable', async () => {
			expect(await save('file.bin', binary)).toBe(`${url}/files/file.bin`);
			expect(await readFile(storage.resolvePath('file.bin'))).toEqual(binary);
			expect(await buffer(storage.read('file.bin'))).toEqual(binary);
		});

		test('overwrites a longer file without leaving trailing bytes', async () => {
			await save('file.bin', Buffer.alloc(1024, 0xff));
			await save('file.bin', binary);
			expect(await readFile(storage.resolvePath('file.bin'))).toEqual(binary);
		});

		test('saves empty files and truncates existing files to zero bytes', async () => {
			await save('empty.bin', Buffer.alloc(0));
			expect(await buffer(storage.read('empty.bin'))).toEqual(Buffer.alloc(0));
			await save('file.bin', binary);
			await save('file.bin', Buffer.alloc(0));
			expect(await readFile(storage.resolvePath('file.bin'))).toEqual(Buffer.alloc(0));
		});

		test('reports failure when the storage directory is occupied by a file', async () => {
			await writeFile(join(rootDir, 'files'), 'keep');
			await expect(save('file.bin', binary)).rejects.toThrow();
			expect(await readFile(join(rootDir, 'files'), 'utf8')).toBe('keep');
		});

		test('reports failure when the destination is a directory', async () => {
			await mkdir(storage.resolvePath('file.bin'), { recursive: true });
			await expect(save('file.bin', binary)).rejects.toThrow();
		});
	});

	test('reports a missing source without changing an existing destination', async () => {
		await storage.saveFromBuffer('file.bin', binary);
		await expect((async () => await storage.saveFromPath('file.bin', join(rootDir, 'missing.bin')))()).rejects.toThrow();
		expect(await readFile(storage.resolvePath('file.bin'))).toEqual(binary);
	});

	test('reports a missing file when consuming the read stream', async () => {
		await expect(buffer(storage.read('missing.bin'))).rejects.toThrow();
	});
});
