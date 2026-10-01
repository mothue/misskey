/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import globals from 'globals';
import shared from '../../packages/shared/eslint.config.js';
export default [
	...shared,
	{
		languageOptions: {
			globals: { ...globals.node, ...globals.browser, Bun: 'readonly' },
			parserOptions: { project: null },
		},
	},
];
