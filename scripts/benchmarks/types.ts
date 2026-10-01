/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import type { Manifest } from '../../fixtures/benchmark/definition.js';
export interface LoadOptions { requests: number; concurrency: number; warmup: number; timeout: number; }
export interface RequestContext { manifest: Manifest; url: string; workerId: number; requestIndex: number; phase: 'warmup' | 'measure'; signal: AbortSignal; requestOrdinal?: number; }
export interface BenchmarkScenario {
	name: string;
	shape?: Record<string, string | number>;
	write?: boolean;
	writeDelta?: { field: 'notes' | 'reactions' | 'follows' | 'readMarkers'; direction: 1 | -1 };
	emptyResponse?: boolean;
	capacity?(manifest: Manifest): number;
	setup?(manifest: Manifest): Promise<void>;
	request(context: RequestContext): Promise<Response>;
	validate(body: unknown, context: RequestContext): void;
	teardown?(): Promise<void>;
}
export type Outcome = 'success' | 'httpError' | 'timeout' | 'transportError' | 'validationFailure';
export interface PhaseResult {
	requested: number; completed: number; elapsedMs: number; samples: Record<Outcome, number[]>;
	httpStatuses: Record<string, number>; partial: boolean;
}
export interface WorkerResult { warmup: PhaseResult; measure?: PhaseResult; cpu: { user: number; system: number }; }
