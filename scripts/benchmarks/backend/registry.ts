/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { benchmark as global } from './timeline-global.js';
import { benchmark as home } from './timeline-home.js';
import { benchmark as user } from './timeline-user.js';
import { benchmark as reactions } from './reaction-list.js';
import { benchmark as create } from './note-create.js';
import { local, heavy, list, channel, userPage, show, rich, thread, notifications, userShow, followers, following, remove, reactionCreate, reactionDelete, followCreate, markRead } from './additional.js';
import { ports } from './ports.js';
export const scenarios = [...ports, global, local, home, heavy, user, userPage, list, channel, show, rich, thread, reactions, notifications, userShow, followers, following, create, remove, reactionCreate, reactionDelete, followCreate, markRead];
export function select(filters: string[]) {
	return scenarios.filter(scenario => !filters.length || filters.some(filter => scenario.name.includes(filter)));
}
