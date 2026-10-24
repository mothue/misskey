/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Resolve from the executing source/bundle, independent of storage and cwd. */
export function findCodeRoot(moduleUrl = import.meta.url): string {
	let directory = dirname(fileURLToPath(moduleUrl));
	for (;;) {
		if (existsSync(join(directory, 'packages/backend/package.json'))) return directory;
		const parent = dirname(directory);
		if (parent === directory) throw new Error('Cannot locate backend code root');
		directory = parent;
	}
}
