/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import type { BenchmarkScenario, LoadOptions, Outcome, PhaseResult } from './types.js';
import type { Manifest } from '../../fixtures/benchmark/definition.js';
export const outcomes: Outcome[] = ['success', 'httpError', 'timeout', 'transportError', 'validationFailure'];
export async function phase(scenario: BenchmarkScenario, manifest: Manifest, url: string, options: LoadOptions,
	count: number, name: 'warmup' | 'measure', stop: AbortSignal, progress?: (result: PhaseResult) => void): Promise<PhaseResult> {
	const samples: Record<Outcome, number[]> = { success: [], httpError: [], timeout: [], transportError: [], validationFailure: [] };
	const result: PhaseResult = { requested: count, completed: 0, elapsedMs: 0, samples, httpStatuses: {}, partial: false };
	const success = new Float64Array(count);
	let successful = 0;
	let next = 0;
	const start = performance.now();
	await Promise.all(Array.from({ length: Math.min(options.concurrency, count) }, async (_, workerId) => {
		while (!stop.aborted && next < count) {
			const requestIndex = next++;
			const signal = AbortSignal.any([stop, AbortSignal.timeout(options.timeout)]);
			const context = { manifest, url, workerId, requestIndex, phase: name, signal };
			let outcome: Outcome = 'success';
			const requestStart = performance.now();
			let elapsed = 0;
			let bytes: ArrayBuffer | undefined;
			try {
				const response = await scenario.request(context);
				bytes = await response.arrayBuffer();
				elapsed = performance.now() - requestStart;
				if (!response.ok) {
					outcome = 'httpError';
					result.httpStatuses[response.status] = (result.httpStatuses[response.status] ?? 0) + 1;
				}
			} catch {
				elapsed = performance.now() - requestStart;
				outcome = signal.aborted && !stop.aborted ? 'timeout' : 'transportError';
			}
			if (outcome === 'success') {
				try { scenario.validate(JSON.parse(new TextDecoder().decode(bytes)), context); } catch { outcome = 'validationFailure'; }
			}
			if (outcome === 'success') success[successful++] = elapsed;
			else samples[outcome].push(elapsed);
			result.completed++;
			if (result.completed % 100 === 0 && progress) {
				progress({ ...result, elapsedMs: performance.now() - start, samples: { ...samples, success: Array.from(success.subarray(0, successful)) }, partial: true });
			}
		}
	}));
	result.elapsedMs = performance.now() - start;
	result.samples.success = Array.from(success.subarray(0, successful));
	result.partial = stop.aborted || result.completed !== count;
	return result;
}
export function summary(result: PhaseResult) {
	const sorted = result.samples.success.toSorted((a, b) => a - b);
	const percentile = (p: number) => sorted.length ? sorted[Math.ceil(sorted.length * p) - 1] : null;
	return {
		elapsedMs: result.elapsedMs, completed: result.completed,
		counts: Object.fromEntries(outcomes.map(outcome => [outcome, result.samples[outcome].length])),
		throughput: result.elapsedMs ? sorted.length / (result.elapsedMs / 1000) : 0,
		completedThroughput: result.elapsedMs ? result.completed / (result.elapsedMs / 1000) : 0,
		latencyMs: { min: sorted[0] ?? null, p50: percentile(0.5), p90: percentile(0.9), p95: percentile(0.95), p99: percentile(0.99), max: sorted.at(-1) ?? null },
		partial: result.partial,
	};
}
