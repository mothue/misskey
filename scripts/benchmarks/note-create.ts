/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { post, viewer } from './scenarios.js';
import type { BenchmarkScenario, RequestContext } from './types.js';
const text = (context: RequestContext) => `benchmark ${context.phase} ${context.requestIndex} `.padEnd(256, 'x');
export const benchmark: BenchmarkScenario = {
	name: 'note/create',
	request: context => post(context, 'notes/create', { text: text(context), visibility: 'public', localOnly: true }),
	validate(body, context) {
		const note = (body as { createdNote?: { id: string; userId: string; text: string } })?.createdNote;
		assert(note && typeof note.id === 'string');
		assert.equal(note.userId, viewer(context).id);
		assert.equal(note.text, text(context));
	},
};
