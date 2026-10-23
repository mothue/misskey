/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import assert from 'node:assert/strict';
import { request } from 'node:http';
import { describe, test } from 'vitest';
import capture from './fixtures/rust-api-routing.json' with { type: 'json' };
import methodCapture from './fixtures/rust-api-method-classes.json' with { type: 'json' };
import { port, relativeFetch } from '../utils.js';

type RoutingCase = {
	id: string;
	path: string;
	method: string;
	headers: Record<string, string>;
	body: string | null;
	expected: { status: number; headers: Record<string, string | undefined>; body: string };
};

const cases: RoutingCase[] = capture.cases;
const methodCases: RoutingCase[] = methodCapture.cases;
const meaningfulHeaders = [
	'content-type', 'cache-control', 'www-authenticate', 'retry-after',
	'access-control-allow-origin', 'access-control-allow-methods',
	'access-control-allow-headers', 'vary', 'clear-site-data',
];

/** Fetch rejects TRACE and can add framing to an otherwise bodyless DELETE. */
function exactMethodRequest({ path, method, headers, body }: RoutingCase): Promise<{
	status: number;
	headers: Record<string, string | string[] | undefined>;
	body: string;
}> {
	return new Promise((resolve, reject) => {
		const outgoing = request({
			host: '127.0.0.1', port, path, method,
			setDefaultHeaders: false,
			headers: {
				host: `127.0.0.1:${port}`, connection: 'close', ...headers,
				...(body === null ? {} : { 'content-length': Buffer.byteLength(body) }),
			},
			signal: AbortSignal.timeout(5000),
		}, response => {
			const chunks: Buffer[] = [];
			response.on('data', chunk => chunks.push(Buffer.from(chunk)));
			response.once('error', reject);
			response.once('end', () => resolve({
				status: response.statusCode!, headers: response.headers,
				body: Buffer.concat(chunks).toString('utf8'),
			}));
		});
		// In the no-length fixture there must be no implicit CL: 0 or chunked
		// framing: Fastify distinguishes methods that are allowed to have bodies.
		outgoing.useChunkedEncodingByDefault = false;
		outgoing.once('error', reject);
		if (body === null) outgoing.end();
		else outgoing.end(body);
	});
}

describe('Rust API routing and parser compatibility with the TS HTTP baseline', () => {
	// These fixtures come from real HTTP responses, including the distinction
	// between global not-found parsing and parsing inside the API plugin. In
	// particular, multipart input on an unmatched route remains a 404, while
	// registered routes retain their API CORS/cache headers on input errors.
	test.each(cases)('$id', async ({ path, method, headers, body, expected }) => {
		const response = await relativeFetch(path, {
			method,
			headers,
			...(body === null ? {} : { body }),
			signal: AbortSignal.timeout(5000),
		});
		assert.equal(response.status, expected.status);
		assert.equal(await response.text(), expected.body);
		for (const name of meaningfulHeaders) {
			assert.equal(response.headers.get(name), expected.headers[name] ?? null, name);
		}
	});

	test.each(methodCases)('$id preserves explicit HTTP request framing', async testCase => {
		const response = await exactMethodRequest(testCase);
		assert.equal(response.status, testCase.expected.status);
		assert.equal(response.body, testCase.expected.body);
		for (const name of ['content-type', 'cache-control', 'www-authenticate', 'access-control-allow-origin']) {
			assert.equal(response.headers[name], testCase.expected.headers[name], name);
		}
	});
});
