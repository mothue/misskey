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
	fs.rmSync(rootDir + '/packages/backend/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/frontend-shared/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/frontend-builder/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/frontend/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/frontend/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/frontend-embed/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/frontend-embed/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/sw/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/sw/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/i18n/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/i18n/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/misskey-js/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-js/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/misskey-reversi/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-reversi/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/packages/misskey-bubble-game/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/packages/misskey-bubble-game/node_modules', { recursive: true, force: true });

	fs.rmSync(rootDir + '/built', { recursive: true, force: true });
	fs.rmSync(rootDir + '/node_modules', { recursive: true, force: true });
})();
