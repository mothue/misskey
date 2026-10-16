// SPDX-FileCopyrightText: 2026 Misskey contributors
// SPDX-License-Identifier: MIT

import { copyFileSync, mkdtempSync, renameSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';

export function publishFile(source, destination) {
	// Readers of the old inode (including dlopen mappings in other containers)
	// must keep the original bytes until they unload the library.
	const temporary = mkdtempSync(join(dirname(destination), '.slacc-publish-'));
	try {
		const next = join(temporary, 'addon');
		copyFileSync(source, next);
		renameSync(next, destination);
	} finally {
		rmSync(temporary, { recursive: true, force: true });
	}
}
