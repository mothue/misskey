/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import * as Bun from 'bun';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';
import { FileWriterStream } from '@/misc/FileWriterStream.js';
import { JsonArrayStream } from '@/misc/JsonArrayStream.js';

describe('Bun native runtime contracts', () => {
	let directory: string;
	beforeEach(async () => { directory = await mkdtemp(join(tmpdir(), 'misskey-native-')); });
	afterEach(async () => { await rm(directory, { recursive: true, force: true }); });

	test.each(['md5', 'sha256', 'sha512'] as const)('%s preserves digest bytes and encodings', algorithm => {
		for (const data of ['', '日本語\u0000😀', Buffer.from([0, 255, 128]), Buffer.alloc(1024 * 1024, 37)]) {
			for (const encoding of ['hex', 'base64', 'base64url'] as const) {
				expect(new Bun.CryptoHasher(algorithm).update(data).digest(encoding)).toBe(createHash(algorithm).update(data).digest(encoding));
			}
		}
	});

	test('writer appends sliced and large binary chunks, flushes on close and accepts URL paths', async () => {
		const path = join(directory, 'out');
		await writeFile(path, 'prefix');
		const writer = new FileWriterStream(pathToFileURL(path)).getWriter();
		const bytes = Uint8Array.from([11, 0, 255, 128, 22]);
		await writer.write(bytes.subarray(1, 4));
		await writer.write(Buffer.alloc(1024 * 1024, 17));
		await writer.close();
		expect(await readFile(path)).toEqual(Buffer.concat([Buffer.from('prefix'), Buffer.from([0, 255, 128]), Buffer.alloc(1024 * 1024, 17)]));
	});

	test('abort retains completed writes, closes the descriptor and rejects further writes', async () => {
		const path = join(directory, 'out');
		const writer = new FileWriterStream(Buffer.from(path)).getWriter();
		await writer.write(Buffer.from('completed'));
		await writer.abort();
		await expect(writer.write(Buffer.from('later'))).rejects.toThrow();
		expect(await readFile(path, 'utf8')).toBe('completed');
		await writeFile(path, 'reopened');
	});

	test('writer reports a missing parent directory', async () => {
		const writer = new FileWriterStream(join(directory, 'missing', 'out')).getWriter();
		await expect(writer.write(Buffer.from('x'))).rejects.toThrow();
		await expect(writer.closed).rejects.toThrow();
	});

	test('global stream pipeline writes valid empty and nonempty JSON arrays', async () => {
		for (const values of [[], [{ text: '日本語' }, 42, null]]) {
			const path = join(directory, 'array');
			await writeFile(path, '');
			await new ReadableStream({ start(controller) { values.forEach(value => controller.enqueue(value)); controller.close(); } })
				.pipeThrough(new JsonArrayStream()).pipeThrough(new TextEncoderStream()).pipeTo(new FileWriterStream(path));
			expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(values);
		}
	});
});
