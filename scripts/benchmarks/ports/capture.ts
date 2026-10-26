/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { readFile, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { comparable } from './http.js';
const [url, output, baseline] = process.argv.slice(2);
if (!url || !output) throw new Error('usage: capture.ts URL OUTPUT [BASELINE]');
const fixtures = JSON.parse(await readFile(new URL('../../../fixtures/ports/http.json', import.meta.url), 'utf8'));
const namesResponse = await fetch(`${url}/api/endpoints`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
if (!namesResponse.ok) throw new Error(`endpoints ${namesResponse.status}`);
const names: string[] = await namesResponse.json();
fixtures.push(...names.map(endpoint => ({ id: `registry:${endpoint}`, endpoint: 'endpoint', method: 'POST', body: JSON.stringify({ endpoint }), expectedStatus: 200 })));
const records = [];
for (const fixture of fixtures) {
 let body = fixture.body;
 if (fixture.depth) body = '{"required":true,"extra":' + '['.repeat(fixture.depth) + '0' + ']'.repeat(fixture.depth) + '}';
 if (fixture.bodyBytes) {
  const prefix = fixture.pattern === 'exponent' ? '{"required":true,"extra":[' : '{"required":true,"extra":"';
  const suffix = fixture.pattern === 'exponent' ? '0]}' : '"}';
  const unit = fixture.pattern === 'exponent' ? '1e-100,' : fixture.pattern === 'surrogate' ? '\\ud800' : 'x';
  const available = fixture.bodyBytes - prefix.length - suffix.length;
  const count = Math.floor(available / unit.length);
  body = prefix + unit.repeat(count) + ' '.repeat(available - count * unit.length) + suffix;
 }
 if (fixture.bodyBytes && Buffer.byteLength(body) !== fixture.bodyBytes) throw new Error('body size mismatch');
 const before = Date.now();
 const response = await fetch(`${url}/api/${fixture.endpoint}`, { method: fixture.method, headers: { 'content-type': 'application/json' }, body: fixture.method === 'GET' ? undefined : body, signal: AbortSignal.timeout(10000) });
 const text = await response.text(); const after = Date.now();
 assert.equal(response.status, fixture.expectedStatus, `${fixture.id}: unexpected HTTP status`);
 const result = { id: fixture.id, requestBytes: body === undefined || body === null ? 0 : Buffer.byteLength(body), ...comparable({ status: response.status, headers: Object.fromEntries(response.headers), body: text }) };
 if (fixture.endpoint === 'ping' && response.status === 200) { const pong = JSON.parse(text).pong; if (!Number.isSafeInteger(pong) || pong < before || pong > after) throw new Error('ping clock range'); result.body = '{"pong":"<clock-range-checked>"}'; }
 records.push(result);
}
await writeFile(output, JSON.stringify({ schemaVersion: 1, normalization: ['omit Date header', 'ping pong checked against request clock range'], endpointNames: names, records }, null, 2) + '\n', { mode: 0o600 });
if (baseline) { const original = JSON.parse(await readFile(baseline, 'utf8')); if (JSON.stringify(original.records) !== JSON.stringify(records)) throw new Error('HTTP differential mismatch'); }
console.log(`Captured ${records.length} cases, ${names.length} endpoint descriptions`);
