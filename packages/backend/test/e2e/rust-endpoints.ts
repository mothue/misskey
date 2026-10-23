/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { describe, test } from 'vitest';
import { loadConfig } from '@/config.js';
import endpoints from '@/server/api/endpoints.js';
import { api } from '../utils.js';

describe('Rust endpoint catalog', () => {
	test('anonymous HTTP returns every registered endpoint exactly once', async () => {
		const response = await api('endpoints', {});
		assert.equal(response.status, 200);
		// Vite's source namespace and Rolldown's server namespace enumerate exports
		// differently. Check membership here; the production HTTP differential and
		// core/transport tests check the actual registry order without sorting it.
		assert.deepEqual(response.body.toSorted(), endpoints.map(endpoint => endpoint.name).toSorted());
	});

	test('additional input properties do not filter or reorder the catalog', async () => {
		const original = await api('endpoints', {});
		assert.equal(original.status, 200);
		const response = await fetch(`http://127.0.0.1:${loadConfig().port}/api/endpoints`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ endpoint: '', extra: null }),
		});
		assert.equal(response.status, 200);
		assert.deepEqual(await response.json(), original.body);
	});

	test('Rust kernel rejects malformed JSON and non-object input before dispatch', async () => {
		for (const body of ['{', '[]', 'null', '42']) {
			const response = await fetch(`http://127.0.0.1:${loadConfig().port}/api/endpoints`, {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body,
			});
			assert.equal(response.status, 400);
		}
	});
});
