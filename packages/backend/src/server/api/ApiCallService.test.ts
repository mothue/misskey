/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test, vi } from 'vitest';
import { ApiCallService } from './ApiCallService.js';
import { ApiError } from './error.js';
import { encodeApiResult } from '../rust/api-callback.js';
import type { ApiLoggerService } from './ApiLoggerService.js';
import type { TelemetryService } from '@/core/telemetry/TelemetryService.js';

vi.mock('./AuthenticateService.js', () => ({ AuthenticationError: class AuthenticationError extends Error {} }));
vi.mock('./ApiLoggerService.js', () => ({ ApiLoggerService: class {} }));
vi.mock('@/core/telemetry/TelemetryService.js', () => ({ TelemetryService: class {} }));

function fixture() {
	const write = vi.fn();
	const captureMessage = vi.fn();
	const startSpan = vi.fn((_name: string, callback: () => unknown) => callback());
	return { service: new ApiCallService({ logger: { write } } as unknown as ApiLoggerService, { captureMessage, startSpan } as unknown as TelemetryService), write, captureMessage, startSpan };
}

test('legacy executor invokes only the validated callback, once, and preserves known API errors', async () => {
	const { service, write } = fixture();
	const invokeValidated = vi.fn(async () => ({ value: 1 }));
	expect(await service.executeLegacy('emoji', { invokeValidated }, { name: 'a' }, null, null)).toEqual({ value: 1 });
	expect(invokeValidated).toHaveBeenCalledExactlyOnceWith({ name: 'a' }, null, null, undefined, undefined, undefined);
	const error = new ApiError({ code: 'KNOWN', id: 'known', message: 'Known error' });
	invokeValidated.mockRejectedValueOnce(error);
	await expect(service.executeLegacy('emoji', { invokeValidated }, {}, null, null)).rejects.toBe(error);
	expect(write).not.toHaveBeenCalled();
});

test('legacy business exceptions retain correlated log, telemetry and public error information', async () => {
	const { service, write, captureMessage } = fixture();
	const invokeValidated = vi.fn(async () => { throw new TypeError('business failed'); });
	await expect(service.executeLegacy('emoji', { invokeValidated }, {}, null, null)).rejects.toMatchObject({ code: 'INTERNAL_ERROR', info: { e: { message: 'business failed', code: 'TypeError' } } });
	expect(write).toHaveBeenCalledOnce();
	expect(captureMessage).toHaveBeenCalledOnce();
	const logId = write.mock.calls[0][0].attributes['error.id'];
	expect(captureMessage.mock.calls[0][1].extra.e.id).toBe(logId);
});

test('business serializer failures are captured instead of leaking through the callback transport', async () => {
	const { service, write } = fixture();
	const invokeValidated = vi.fn(async () => ({ unsupported: 1n }));
	await expect(service.executeLegacy('fixture', { invokeValidated }, {}, null, null, undefined, undefined, undefined, encodeApiResult)).rejects.toMatchObject({ code: 'INTERNAL_ERROR' });
	expect(write).toHaveBeenCalledOnce();
});
