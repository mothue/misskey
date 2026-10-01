/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import * as fs from 'node:fs/promises';
import * as Bun from 'bun';
import type { PathLike } from 'node:fs';

/**
 * `fs.createWriteStream()`相当のことを行う`WritableStream` (Web標準)
 */
export class FileWriterStream extends WritableStream<Uint8Array> {
	constructor(path: PathLike) {
		let file: fs.FileHandle | null = null;
		let sink: Bun.FileSink | null = null;
		const close = async () => {
			try { await sink?.end(); } finally { sink = null; await file?.close(); file = null; }
		};

		super({
			start: async () => {
				// Keep O_APPEND and PathLike support; writer(path) starts at offset zero.
				file = await fs.open(path, 'a');
				try { sink = Bun.file(file.fd).writer(); } catch (error) { await close(); throw error; }
			},
			write: async (chunk) => {
				if (!sink) throw new Error('file writer is closed');
				try {
					await sink.write(chunk);
					// Await flushing to preserve backpressure and surface filesystem failures.
					await sink.flush();
				} catch (error) { await close(); throw error; }
			},
			close,
			abort: close,
		});
	}
}
