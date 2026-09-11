/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { RE2JS } from 're2js';

const supportedFlags = new Set(['d', 'g', 'i', 'm', 's', 'u', 'y']);

function convertFlags(flags: string): { flags: number; sticky: boolean } {
	const seen = new Set<string>();
	let converted = 0;

	for (const flag of flags) {
		if (!supportedFlags.has(flag) || seen.has(flag)) {
			throw new SyntaxError(`Invalid flags supplied to RegExp constructor '${flags}'`);
		}

		seen.add(flag);
		if (flag === 'i') converted |= RE2JS.CASE_INSENSITIVE;
		if (flag === 'm') converted |= RE2JS.MULTILINE;
		if (flag === 's') converted |= RE2JS.DOTALL;
	}

	return {
		flags: converted,
		sticky: seen.has('y'),
	};
}

/**
 * Compatibility layer for the subset of node-re2 used by Misskey.
 *
 * RE2JS always operates in Unicode mode. The d, g, and u flags do not alter
 * the one-shot boolean checks used by Misskey, so only i, m, s, and y need
 * explicit handling here.
 */
export default class RE2 {
	private readonly compiled: RE2JS;
	private readonly sticky: boolean;

	constructor(pattern: string, flags = '') {
		if (typeof pattern !== 'string') {
			throw new TypeError('RE2 pattern must be a string');
		}
		if (typeof flags !== 'string') {
			throw new TypeError('RE2 flags must be a string');
		}

		const converted = convertFlags(flags);
		this.compiled = RE2JS.compile(pattern, converted.flags);
		this.sticky = converted.sticky;
	}

	public test(input: string | Uint8Array): boolean {
		return this.sticky
			? this.compiled.matcher(input).lookingAt()
			: this.compiled.test(input);
	}
}
