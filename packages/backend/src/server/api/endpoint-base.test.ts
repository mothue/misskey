/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test, vi } from 'vitest';
import { Endpoint } from './endpoint-base.js';
const meta = { requireCredential: false } as const;
const params = { type: 'object', properties: { required: { type: 'boolean' }, value: { type: 'string', default: 'baseline' } }, required: ['required'] } as const;
class FixtureEndpoint extends Endpoint<typeof meta, typeof params> {}

test('legacy exec keeps validation/defaults and private invokeValidated runs exactly the supplied callback', async () => {
	const callback = vi.fn(async (value: unknown) => value);
	const endpoint = new FixtureEndpoint(meta, params, callback);
	await expect(endpoint.exec({}, null, null)).rejects.toMatchObject({ code: 'INVALID_PARAM', info: { param: '#/required' } });
	expect(callback).not.toHaveBeenCalled();
	expect(await endpoint.exec({ required: true }, null, null)).toEqual({ required: true, value: 'baseline' });
	expect(callback).toHaveBeenCalledTimes(1);
	callback.mockClear();
	const validated = { required: true, value: 'Rust' };
	expect(await endpoint.invokeValidated(validated, null, null)).toBe(validated);
	expect(callback).toHaveBeenCalledExactlyOnceWith(validated, null, null, undefined, undefined, undefined, undefined);
});
