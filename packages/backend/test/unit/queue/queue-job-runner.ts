/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { describe, expect, test, vi } from 'vitest';
import { runQueueJob } from '@/queue/queue-job-runner.js';
import { TelemetryService } from '@/core/telemetry/TelemetryService.js';
import jsonld from 'jsonld';
import { Job } from 'bullmq';
import type { MinimalQueue } from 'bullmq';

describe('runQueueJob', () => {
	const telemetry = { startSpan: <T>(_name: string, fn: () => T): T => fn() };

	test.each([undefined, 0, 5, 100])('bounds retained log count for existing jobs with keepLogs=%s', async keepLogs => {
		const opts = { keepLogs };
		const log = vi.fn().mockResolvedValue(1);
		const error = new Error('failure');
		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw error; }, vi.fn(), { log, opts })).rejects.toBe(error);
		expect(opts.keepLogs).toBe(keepLogs === 5 ? 5 : 20);
	});

	test('passes the retention cap through BullMQ on every repeated manual retry', async () => {
		const addLog = vi.fn().mockResolvedValue(20);
		const queue = {
			toKey: (key: string) => `inbox:${key}`,
			qualifiedName: 'inbox',
			backend: { addLog },
		} as unknown as MinimalQueue;
		const job = new Job(queue, 'inbox', {}, {}, '42');
		const error = new Error('failure');
		for (let attempt = 0; attempt < 25; attempt++) {
			await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw error; }, vi.fn(), job)).rejects.toBe(error);
		}
		expect(addLog).toHaveBeenCalledTimes(25);
		for (const [jobId, line, keepLogs] of addLog.mock.calls) {
			expect(jobId).toBe('42');
			expect(JSON.parse(line).eventName).toBe('queue.job.failed');
			expect(keepLogs).toBe(20);
		}
	});

	test('persists the real JSON-LD safe-mode cause in the job log before rethrowing', async () => {
		const error = await jsonld.normalize({
			'@context': { name: 'https://schema.org/name' },
			'@id': 'https://example.com/note',
			name: 'test',
			unmapped: 'value',
		}).catch((error: unknown) => error);
		expect(error).toBeInstanceOf(Error);
		const log = vi.fn().mockResolvedValue(1);

		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw error; }, vi.fn(), { log, opts: {} })).rejects.toBe(error);

		expect(log).toHaveBeenCalledOnce();
		expect(JSON.parse(log.mock.calls[0][0])).toMatchObject({
			eventName: 'queue.job.failed',
			error: {
				type: 'jsonld.ValidationError',
				message: 'Safe mode validation error.',
				details: { event: { code: 'invalid property', details: { property: 'unmapped' } } },
			},
		});
	});

	test('retains a real JSON-LD document-loader error nested inside details.cause', async () => {
		const cause = Object.assign(new Error('TLS certificate invalid'), { code: 'CERT_HAS_EXPIRED' });
		const error = await jsonld.normalize({ '@context': 'https://example.invalid/context' }, {
			documentLoader: async () => { throw cause; },
		}).catch((error: unknown) => error);
		const log = vi.fn().mockResolvedValue(1);
		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw error; }, vi.fn(), { log, opts: {} })).rejects.toBe(error);
		expect(JSON.parse(log.mock.calls[0][0])).toMatchObject({
			error: { details: { cause: { message: cause.message, code: cause.code } } },
		});
	});

	test('awaits job-log persistence even when the regular error handler fails', async () => {
		let finishLog!: (count: number) => void;
		const log = vi.fn(() => new Promise<number>(resolve => { finishLog = resolve; }));
		const originalError = new Error('original');
		let settled = false;
		const result = runQueueJob(telemetry, 'Queue: Inbox', () => { throw originalError; }, () => { throw new Error('logger failed'); }, { log, opts: {} });
		const assertion = expect(result.finally(() => { settled = true; })).rejects.toBe(originalError);
		await Promise.resolve();
		expect(log).toHaveBeenCalledOnce();
		expect(settled).toBe(false);
		finishLog(1);
		await assertion;
	});

	test('does not replace the processor error when saving the job log fails', async () => {
		const originalError = new Error('original');
		const log = vi.fn().mockRejectedValue(new Error('Redis unavailable'));

		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw originalError; }, vi.fn(), { log, opts: {} })).rejects.toBe(originalError);
		expect(log).toHaveBeenCalledOnce();
	});

	test('redacts and bounds persisted job errors and does not log successful jobs', async () => {
		const log = vi.fn().mockResolvedValue(1);
		const error = Object.assign(new Error('failed'), { details: { token: 'sensitive', value: 'x'.repeat(100000) } });
		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => { throw error; }, vi.fn(), { log, opts: {} })).rejects.toBe(error);
		const line = log.mock.calls[0][0];
		expect(line).not.toContain('sensitive');
		expect(JSON.parse(line).error.details.token).toBe('[REDACTED]');
		expect(Buffer.byteLength(line, 'utf8')).toBeLessThanOrEqual(17 * 1024);
		log.mockClear();
		await expect(runQueueJob(telemetry, 'Queue: Inbox', () => 'ok', vi.fn(), { log, opts: {} })).resolves.toBe('ok');
		expect(log).not.toHaveBeenCalled();
	});

	test('returns the processor result without invoking the error handler', async () => {
		let spanActive = false;
		const startSpan = vi.fn(<T>(_name: string, fn: () => T): T => {
			spanActive = true;
			const result = fn();
			if (result instanceof Promise) return result.finally(() => { spanActive = false; }) as T;
			spanActive = false;
			return result;
		});
		const telemetryService = {
			startSpan,
		} as unknown as TelemetryService;
		const onError = vi.fn();

		await expect(runQueueJob(telemetryService, 'Queue: test', () => 'ok', onError)).resolves.toBe('ok');

		expect(onError).not.toHaveBeenCalled();
		expect(spanActive).toBe(false);
	});

	test('handles failures while the processor span is active and rethrows the original error', async () => {
		let spanActive = false;
		const startSpan = vi.fn(<T>(_name: string, fn: () => T): T => {
			spanActive = true;
			const result = fn();
			if (result instanceof Promise) return result.finally(() => { spanActive = false; }) as T;
			spanActive = false;
			return result;
		});
		const telemetryService = {
			startSpan,
		} as unknown as TelemetryService;
		const onError = vi.fn((error: Error) => {
			expect(spanActive).toBe(true);
			expect(error).toBeInstanceOf(Error);
		});
		const originalError = new Error('failed');

		await expect(runQueueJob(telemetryService, 'Queue: test', async () => {
			throw originalError;
		}, onError)).rejects.toBe(originalError);

		expect(onError).toHaveBeenCalledOnce();
		expect(spanActive).toBe(false);
	});
});
