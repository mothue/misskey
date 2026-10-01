/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { benchmark as global } from './timeline-global.js';
import { benchmark as home } from './timeline-home.js';
import { benchmark as user } from './timeline-user.js';
import { benchmark as reactions } from './reaction-list.js';
import { benchmark as create } from './note-create.js';
export const scenarios = [global, home, user, reactions, create];
export function select(filters: string[]) {
	return scenarios.filter(scenario => !filters.length || filters.some(filter => scenario.name.includes(filter)));
}
