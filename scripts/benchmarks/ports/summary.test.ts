/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { test, expect } from 'bun:test';
import { distribution } from './summary.js';
test('reports median, range and population variance without discarding spread', () => {
 expect(distribution([3, 1, 2])).toEqual({ median: 2, min: 1, max: 3, variance: 2 / 3 });
});
import { completedOrder } from './summary.js';
test('order metadata describes completed smoke and partial paired runs', () => {
 expect(completedOrder([{ pair: 0, label: 'baseline' }, { pair: 0, label: 'candidate' }])).toEqual(['AB']);
 expect(completedOrder([{ pair: 1, label: 'baseline' }, { pair: 1, label: 'candidate' }, { pair: 2, label: 'candidate' }])).toEqual(['AB', 'B']);
});
