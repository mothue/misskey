/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import type { BenchmarkScenario } from './types.js';
export const ports: BenchmarkScenario[] = ['ping', 'test', 'endpoint'].map(name => ({
 name: `ports/${name}`,
 request: context => fetch(`${context.url}/api/${name}`, { method: 'POST', headers: { 'content-type': 'application/json' }, signal: context.signal,
  body: JSON.stringify(name === 'test' ? { required: true, string: '日本語😀', extra: { nested: [1, null, false] } } : name === 'endpoint' ? { endpoint: 'test' } : {}) }),
 validate(body) {
  assert(body && typeof body === 'object');
  if (name === 'ping') assert(Number.isSafeInteger(body.pong) && Math.abs(Date.now() - body.pong) < 10000);
  if (name === 'test') assert.deepEqual(body, { required: true, string: '日本語😀', extra: { nested: [1, null, false] }, default: 'hello', nullableDefault: 'hello' });
  if (name === 'endpoint') assert.deepEqual(body, { params: [{ name: 'required', type: 'Boolean' }, { name: 'string', type: 'String' }, { name: 'default', type: 'String' }, { name: 'nullableDefault', type: 'String' }, { name: 'id', type: 'String' }] });
 },
}));
