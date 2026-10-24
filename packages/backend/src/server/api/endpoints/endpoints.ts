/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { Injectable } from '@nestjs/common';
import { Endpoint } from '@/server/api/endpoint-base.js';
import { RustBackendService } from '@/server/rust/RustBackendService.js';

export const meta = {
	requireCredential: false,

	tags: ['meta'],

	res: {
		type: 'array',
		optional: false, nullable: false,
		items: {
			type: 'string',
			optional: false, nullable: false,
		},
		example: [
			'admin/abuse-user-reports',
			'admin/accounts/create',
			'admin/announcements/create',
			'...',
		],
	},
} as const;

export const paramDef = {
	type: 'object',
	properties: {},
	required: [],
} as const;

@Injectable()
export default class extends Endpoint<typeof meta, typeof paramDef> { // eslint-disable-line import/no-default-export
	constructor(
		private rustBackendService: RustBackendService,
	) {
		super(meta, paramDef, async () => this.rustBackendService.endpoints());
	}
}
