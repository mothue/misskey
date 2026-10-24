/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { Injectable } from '@nestjs/common';
import { RustBackendService } from '@/server/rust/RustBackendService.js';
import { Endpoint } from '@/server/api/endpoint-base.js';

export const meta = {
	requireCredential: false,

	tags: ['meta'],

	res: {
		type: 'object',
		nullable: true,
		properties: {
			params: {
				type: 'array',
				items: {
					type: 'object',
					properties: {
						name: { type: 'string' },
						type: { type: 'string' },
					},
				},
			},
		},
	},
} as const;

export const paramDef = {
	type: 'object',
	properties: {
		endpoint: { type: 'string' },
	},
	required: ['endpoint'],
} as const;

@Injectable()
export default class extends Endpoint<typeof meta, typeof paramDef> { // eslint-disable-line import/no-default-export
	constructor(
		private rustBackendService: RustBackendService,
	) {
		super(meta, paramDef, async (ps) => this.rustBackendService.endpoint(ps.endpoint));
	}
}
