/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { createReadStream } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { createHash, randomUUID } from 'node:crypto';
import { mkdtemp, open, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import * as Bun from 'bun';
import { FileWriterStream } from '../../../packages/backend/src/misc/FileWriterStream.ts';

// Comparators execute the old and new implementations on the same Bun runtime.
export function runtimeCases(directory) {
	const cases = [];
	for (const algorithm of ['md5', 'sha256', 'sha512']) {
		for (const size of [4096, 1048576]) {
			for (const variant of ['node-crypto', 'bun-hasher']) {
				cases.push({ name: `runtime/hash/${algorithm}/${size / 1024}KiB/${variant}`, async setup() {
					const data = Buffer.alloc(size, 37);
					let digest;
					return { metadata: { sizeBytes: size, algorithm, dataSource: 'deterministic in-memory bytes' },
						async run() { digest = variant === 'node-crypto' ? createHash(algorithm).update(data).digest('hex') : new Bun.CryptoHasher(algorithm).update(data).digest('hex'); },
						async verify() { assert.equal(digest, createHash(algorithm).update(data).digest('hex')); }, async close() {} };
				} });
			}
		}
	}
	for (const variant of ['file-handle', 'file-sink']) {
		cases.push({ name: `runtime/writer/1024KiB/${variant}`, async setup() {
			const root = await mkdtemp(join(directory, 'misskey-bench-'));
			const path = join(root, 'out');
			const chunk = Buffer.alloc(16384, 37);
			return { metadata: { sizeBytes: 1048576, chunks: 64, cache: 'OS cache enabled; no fsync' }, async run() {
				await writeFile(path, '');
				let handle;
				const legacy = () => new WritableStream({
					async start() { handle = await open(path, 'a'); }, async write(data) { await handle.write(data); },
					async close() { await handle.close(); }, async abort() { await handle.close(); },
				});
				const writer = (variant === 'file-handle' ? legacy() : new FileWriterStream(path)).getWriter();
				try { for (let i = 0; i < 64; i++) await writer.write(chunk); await writer.close(); } catch (error) { await writer.abort(); throw error; }
			}, async verify() { assert.deepEqual(await readFile(path), Buffer.alloc(1048576, 37)); }, async close() { await rm(root, { recursive: true, force: true }); } };
		} });
	}
	for (const variant of ['node-stream', 'bun-stream']) {
		cases.push({ name: `runtime/file-hash/md5/1024KiB/${variant}`, async setup() {
			const root = await mkdtemp(join(directory, 'misskey-bench-'));
			const path = join(root, 'input');
			const data = Buffer.alloc(1048576, 37);
			await writeFile(path, data);
			let digest;
			return { metadata: { sizeBytes: data.length, algorithm: 'md5', cache: 'OS cache enabled' }, async run() {
				if (variant === 'node-stream') {
					const hash = createHash('md5').setEncoding('hex');
					await pipeline(createReadStream(path), hash);
					digest = hash.read();
				} else {
					const hash = new Bun.CryptoHasher('md5');
					for await (const chunk of Bun.file(path).stream()) hash.update(chunk);
					digest = hash.digest('hex');
				}
			}, async verify() { assert.equal(digest, createHash('md5').update(data).digest('hex')); }, async close() { await rm(root, { recursive: true, force: true }); } };
		} });
	}
	for (const variant of ['node-crypto', 'web-crypto']) {
		cases.push({ name: `runtime/uuid/1000/${variant}`, async setup() {
			let value;
			return { metadata: { operationsPerSample: 1000 }, async run() { for (let i = 0; i < 1000; i++) value = variant === 'node-crypto' ? randomUUID() : globalThis.crypto.randomUUID(); },
				async verify() { assert.match(value, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/); }, async close() {} };
		} });
	}
	return cases;
}
