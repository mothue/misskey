/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import * as fs from 'node:fs';

const rootDir = import.meta.dirname + '/../..';

(async () => {
	fs.rmSync(rootDir + '/packages/backend-rust/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/backend/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/backend/src-js', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/frontend/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/frontend-embed/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/icons-subsetter/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/i18n/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/sw/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-js/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-reversi/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-bubble-game/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/built', { recursive: true, force: true });
})();
