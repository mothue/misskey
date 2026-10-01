/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { post, reactionNote, rows } from './scenarios.js';
import { id } from '../../fixtures/benchmark/definition.js';
import type { BenchmarkScenario } from './types.js';
export const benchmark: BenchmarkScenario = {
	name: 'reaction/list',
	request: context => post(context, 'notes/reactions', { noteId: reactionNote(context), limit: 20 }),
	validate(body, context) {
		const start = context.manifest.reactionNotes.indexOf(reactionNote(context)) * context.manifest.counts.reactionsPerNote;
		const ids = Array.from({ length: context.manifest.counts.reactionsPerNote }, (_, i) => id(6, start + i));
		for (const reaction of rows(body)) {
			assert(ids.includes(reaction.id));
			assert.equal(reaction.type, '👍');
			const index = ids.indexOf(reaction.id);
			assert.equal(reaction.user.id, context.manifest.users[index].id);
		}
	},
};
