/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { createHash } from 'node:crypto';

export const fixtureVersion = 1;
export const seed = 'misskey-benchmark-v1';
export const epoch = Date.parse('2026-01-01T00:00:00Z');
export const sizes = {
	small: { users: 128, notes: 8192, follows: 8, heavyFollows: 32, reactionNotes: 16, reactionsPerNote: 32 },
	standard: { users: 1024, notes: 131072, follows: 32, heavyFollows: 128, reactionNotes: 64, reactionsPerNote: 256 },
};
export type FixtureSize = keyof typeof sizes;
export function id(kind: number, index: number): string {
	return (epoch + index - 946684800000).toString(36).padStart(8, '0') + kind.toString(36).padStart(4, '0') + index.toString(36).padStart(4, '0');
}
export function random(seedValue: string) {
	let state = createHash('sha256').update(seedValue).digest().readUInt32LE(0);
	return () => {
		state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
		return state / 4294967296;
	};
}
export interface FixtureUser {
	key: string; id: string; token: string; follows: string[]; mutes: string[]; blocks: string[];
}
export interface FixtureNote { id: string; userId: string; text: string; visibility: 'public' | 'home' | 'followers'; }
export interface Manifest {
	id: string; version: number; seed: string; fingerprint: string; size: FixtureSize;
	counts: typeof sizes.small; users: FixtureUser[]; reactionNotes: string[];
	noteIds: string[]; distribution: { visibility: Record<string, number>; textLength: Record<string, number> };
}
export function definition(size: FixtureSize) {
	const counts = sizes[size];
	const rng = random(seed);
	const users: FixtureUser[] = Array.from({ length: counts.users }, (_, index) => {
		const following = index % 4 === 3 ? counts.heavyFollows : counts.follows;
		return {
			key: `user/${index}`, id: id(1, index),
			token: createHash('sha256').update(`${seed}:${size}:token:${index}`).digest('hex').slice(0, 16),
			follows: Array.from({ length: following }, (_, offset) => id(1, (index + offset + 1) % counts.users)),
			mutes: [id(1, (index + following + 1) % counts.users), id(1, (index + following + 2) % counts.users)],
			blocks: [id(1, (index + following + 3) % counts.users)],
		};
	});
	const notes: FixtureNote[] = Array.from({ length: counts.notes }, (_, index) => {
		// Stratified visibility/text lengths; skewed authors, with a guaranteed note for every user.
		const author = index < counts.users ? index : Math.floor(rng() ** 2 * counts.users);
		const length = index % 10 < 6 ? 32 : index % 10 < 9 ? 256 : 1024;
		return { id: id(2, index), userId: users[author].id, text: `benchmark ${index} `.padEnd(length, 'x'),
			visibility: index % 10 < 8 ? 'public' : index % 10 === 8 ? 'home' : 'followers' };
	});
	const reactionNotes = notes.filter(note => note.visibility === 'public').slice(-counts.reactionNotes).map(note => note.id);
	const fingerprint = createHash('sha256').update(JSON.stringify({ fixtureVersion, seed, counts, users, notes })).digest('hex');
	const distribution = { visibility: { public: 0, home: 0, followers: 0 }, textLength: {} as Record<string, number> };
	for (const note of notes) {
		distribution.visibility[note.visibility]++;
		distribution.textLength[note.text.length] = (distribution.textLength[note.text.length] ?? 0) + 1;
	}
	// noteIds[index] resolves the logical key note/<index> without storing bodies twice.
	const manifest: Manifest = { id: `misskey-benchmark-v1/${size}`, version: fixtureVersion, seed, fingerprint, size, counts, users, reactionNotes, noteIds: notes.map(note => note.id), distribution };
	return { manifest, notes };
}
