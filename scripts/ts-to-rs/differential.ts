/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { readFile } from 'node:fs/promises';
import { isDeepStrictEqual } from 'node:util';
import { fileURLToPath } from 'node:url';
import { generate } from './native-status.js';
import { createResponseValidator, type ResponseSample, type ResponseValidator } from './response-validation.js';

export type Capture = { id: string; sample: ResponseSample; clockWindow?: [number, number] };
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);

/** Capture files contain complete response bytes and must be kept private. */
export function decodeCaptures(value: unknown): Capture[] {
	if (!Array.isArray(value)) throw new Error('Expected an array of captured responses');
	const seen = new Set<string>();
	return value.map((row: unknown): Capture => {
		if (!object(row) || typeof row.id !== 'string' || seen.has(row.id)
			|| typeof row.endpoint !== 'string' || typeof row.status !== 'number' || !Number.isInteger(row.status)
			|| row.status < 100 || row.status > 599 || typeof row.rawBody !== 'string'
			|| (row.method !== undefined && row.method !== 'post' && row.method !== 'get')) {
			throw new Error('Invalid or duplicate captured response');
		}
		seen.add(row.id);
		if (row.rawBody.length) {
			let decoded: unknown;
			try { decoded = JSON.parse(row.rawBody); } catch { throw new Error('Response bytes are not JSON'); }
			if (!isDeepStrictEqual(decoded, row.body)) throw new Error('Captured body disagrees with response bytes');
		}
		let clockWindow: [number, number] | undefined;
		if (row.clockWindowMs !== undefined && row.clockWindowMs !== null) {
			const range = row.clockWindowMs;
			if (!Array.isArray(range) || range.length !== 2 || typeof range[0] !== 'number' || typeof range[1] !== 'number'
				|| !Number.isSafeInteger(range[0]) || !Number.isSafeInteger(range[1]) || range[0] > range[1]) {
				throw new Error('Invalid captured clock interval');
			}
			clockWindow = [range[0], range[1]];
		}
		return { id: row.id, sample: { endpoint: row.endpoint, status: row.status, body: row.body, rawBody: row.rawBody,
			...(row.method ? { method: row.method } : {}) }, ...(clockWindow ? { clockWindow } : {}) };
	});
}

function semanticBody(capture: Capture): unknown {
	const { sample, clockWindow } = capture;
	if (sample.endpoint !== 'ping') return sample.body;
	// Check each actual clock value before comparing a clock-independent view.
	// SDK validation still receives the original, unmodified response body.
	if (!clockWindow || !object(sample.body) || typeof sample.body.pong !== 'number'
		|| !Number.isSafeInteger(sample.body.pong) || sample.body.pong < clockWindow[0] - 1 || sample.body.pong > clockWindow[1] + 1) {
		throw new Error('Ping timestamp is outside its observed request interval');
	}
	return { ...sample.body, pong: 0 };
}

/** Identical inherited SDK violations stay BASELINE; semantic differences fail. */
export function compareCaptures(validator: ResponseValidator, baseline: Capture[], candidate: Capture[], requiredEndpoints: readonly string[]) {
	const left = new Map(baseline.map(row => [row.id, row]));
	const right = new Map(candidate.map(row => [row.id, row]));
	if (left.size !== baseline.length || right.size !== candidate.length) throw new Error('Duplicate capture IDs');
	const rows = [...new Set([...left.keys(), ...right.keys()])].map(id => {
		const a = left.get(id), b = right.get(id);
		if (!a || !b) return { id, endpoint: a?.sample.endpoint ?? b?.sample.endpoint ?? '', status: 'FAIL', parity: 'FAIL', success: false, reason: 'Missing paired response' };
		const baselineResult = validator.validate(a.sample), candidateResult = validator.validate(b.sample);
		const parity = a.sample.endpoint === b.sample.endpoint && a.sample.status === b.sample.status
			&& (a.sample.method ?? 'post') === (b.sample.method ?? 'post')
			&& ((a.sample.rawBody?.length === 0 && b.sample.rawBody?.length === 0)
				|| isDeepStrictEqual(semanticBody(a), semanticBody(b)));
		const status = !parity ? 'FAIL'
			: baselineResult.status === 'PASS' && candidateResult.status === 'PASS' ? 'PASS'
				: baselineResult.status === 'FAIL' && candidateResult.status === 'FAIL'
					&& isDeepStrictEqual(baselineResult.diagnostics, candidateResult.diagnostics) ? 'BASELINE' : 'FAIL';
		return { id, endpoint: a.sample.endpoint, status, parity: parity ? 'PASS' : 'FAIL', success: a.sample.status >= 200 && a.sample.status < 300, baseline: baselineResult, candidate: candidateResult };
	});
	const missing = requiredEndpoints.filter(endpoint => !rows.some(row => row.endpoint === endpoint && row.parity === 'PASS' && row.success));
	const status = missing.length || rows.some(row => row.status === 'FAIL') ? 'FAIL'
		: rows.some(row => row.status === 'BASELINE') ? 'BASELINE' : 'PASS';
	return { status, coverage: { status: missing.length ? 'FAIL' : 'PASS', required: requiredEndpoints.length, missing }, rows };
}

if (import.meta.main) {
	try {
		const [baselinePath, candidatePath] = process.argv.slice(2);
		if (!baselinePath || !candidatePath || process.argv.length !== 4) throw new Error('Usage: bun scripts/ts-to-rs/differential.ts BASELINE.json CANDIDATE.json');
		const baseline: unknown = JSON.parse(await readFile(baselinePath, 'utf8'));
		const candidate: unknown = JSON.parse(await readFile(candidatePath, 'utf8'));
		const root = fileURLToPath(new URL('../../', import.meta.url));
		const { inventory } = await generate(root);
		const required = inventory.rows.filter(row => row.status === 'Native').map(row => row.endpoint);
		const result = compareCaptures(createResponseValidator(), decodeCaptures(baseline), decodeCaptures(candidate), required);
		console.log(JSON.stringify(result));
		if (result.status === 'FAIL') process.exitCode = 1;
	} catch (error) {
		console.error(error instanceof Error ? error.message : 'Differential validation failed');
		process.exitCode = 1;
	}
}
