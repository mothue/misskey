/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { Injectable } from '@nestjs/common';
import { bindThis } from '@/decorators.js';
import { TelemetryService } from '@/core/telemetry/TelemetryService.js';
import type { MiLocalUser } from '@/models/User.js';
import type { MiAccessToken } from '@/models/AccessToken.js';
import { ApiError } from './error.js';
import { ApiLoggerService } from './ApiLoggerService.js';
import { AuthenticationError } from './AuthenticateService.js';

export type ValidatedLegacyEndpoint = {
	invokeValidated: (params: any, user: MiLocalUser | null, token: MiAccessToken | null, file?: { name?: string | null; path: string }, ip?: string | null, headers?: Record<string, string> | null) => Promise<unknown>;
};

/** Temporary legacy business executor. The Rust API kernel owns all public checks. */
@Injectable()
export class ApiCallService {
	constructor(
		private apiLoggerService: ApiLoggerService,
		private telemetryService: TelemetryService,
	) {}

	@bindThis
	public async executeLegacy(
		name: string,
		endpoint: ValidatedLegacyEndpoint,
		params: unknown,
		user: MiLocalUser | null,
		token: MiAccessToken | null,
		file?: { name?: string | null; path: string },
		ip?: string,
		headers?: Record<string, string>,
		serialize: (value: unknown) => unknown = value => value,
	): Promise<unknown> {
		try {
			return serialize(await endpoint.invokeValidated(params, user, token, file, ip, headers));
		} catch (error) {
			if (error instanceof ApiError || error instanceof AuthenticationError) throw error;
			const err = error instanceof Error ? error : new Error(String(error));
			const errId = globalThis.crypto.randomUUID();
			this.apiLoggerService.logger.write({
				level: 'error',
				eventName: 'api.endpoint.failed',
				message: `Internal error occurred in ${name}: ${err.message}`,
				attributes: { 'api.endpoint': name, 'error.id': errId, 'api.params': params },
				error: err,
			});
			this.telemetryService.captureMessage(`Internal error occurred in ${name}: ${err.message}`, {
				level: 'error',
				userId: user?.id,
				extra: { ep: name, e: { message: err.message, code: err.name, stack: err.stack, id: errId } },
			});
			throw new ApiError(null, { e: { message: err.message, code: err.name, id: errId } });
		}
	}

	public withApiSpan<T>(name: string, callback: () => Promise<T>): Promise<T> {
		return this.telemetryService.startSpan('API: ' + name, callback);
	}
}
