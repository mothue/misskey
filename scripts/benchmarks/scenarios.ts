/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import type { Manifest } from '../../fixtures/benchmark/definition.js';
import type { BenchmarkScenario, RequestContext } from './types.js';
export function viewer(context: RequestContext) {
	return context.manifest.users[(context.requestIndex + context.workerId) % context.manifest.users.length];
}
export function target(context: RequestContext) {
	return context.manifest.users[(context.requestIndex * 17 + context.workerId) % context.manifest.users.length];
}
export function reactionNote(context: RequestContext) {
	return context.manifest.reactionNotes[context.requestIndex % context.manifest.reactionNotes.length];
}
export function post(context: RequestContext, endpoint: string, body: Record<string, unknown>) {
	return fetch(`${context.url}/api/${endpoint}`, { method: 'POST', signal: context.signal,
		headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...body, i: viewer(context).token }) });
}
export function rows(body: unknown): Record<string, any>[] {
	assert(Array.isArray(body) && body.length > 0, 'expected nonempty result');
	for (const row of body) assert(row && typeof row.id === 'string', 'missing item id');
	return body;
}
const noteSets = new WeakMap<Manifest, Set<string>>();
export function timeline(name: string, endpoint: string, mode: 'global' | 'home' | 'user'): BenchmarkScenario {
	return {
		name,
		request: context => post(context, endpoint, { limit: 20, ...(mode === 'user' ? { userId: target(context).id } : {}) }),
		validate(body, context) {
			let ids = noteSets.get(context.manifest);
			if (!ids) { ids = new Set(context.manifest.noteIds); noteSets.set(context.manifest, ids); }
			for (const note of rows(body)) {
				assert(ids.has(note.id), 'unknown fixture note');
				if (mode === 'global') assert.equal(note.visibility, 'public');
				if (mode === 'user') {
					assert.equal(note.userId, target(context).id);
					if (note.visibility === 'followers') assert(note.userId === viewer(context).id || viewer(context).follows.includes(note.userId));
				}
				if (mode === 'home') {
					const user = viewer(context);
					assert(note.userId === user.id || user.follows.includes(note.userId), 'home contains unrelated author');
					assert(!user.mutes.includes(note.userId) && !user.blocks.includes(note.userId), 'home contains muted/blocked author');
					assert(!context.manifest.users.find(author => author.id === note.userId)?.blocks.includes(user.id), 'home contains an author blocking the viewer');
					assert(['public', 'home', 'followers'].includes(note.visibility), 'unexpected visibility');
				}
			}
		},
	};
}
