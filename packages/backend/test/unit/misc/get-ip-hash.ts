/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { describe, expect, test } from 'vitest';
import { getIpHash } from '@/misc/get-ip-hash.js';

describe('getIpHash', () => {
	test.each([
		['127.0.0.1', 'ip-z8kflt'],
		['192.0.2.1', 'ip-1h9u1vl'],
		['192.0.2.1:12345', 'ip-1h9u1vl'],
		['::ffff:192.0.2.1', 'ip-1h9u1vl'],
		['2001:db8:1234:5678:1::1', 'ip-hir6906uoepk'],
		['2001:db8:1234:5678:ffff::abcd', 'ip-hir6906uoepk'],
		['2001:db8:1234:5679::1', 'ip-hir6906uoepl'],
		['::ffff:c000:201', 'ip-0'],
	])('preserves the rate limit key for %s', (ip, expected) => {
		expect(getIpHash(ip)).toBe(expected);
	});

	test.each(['012.0.0.1', 'not-an-ip', '256.0.0.1'])('rejects invalid address %s', (ip) => {
		expect(() => getIpHash(ip)).toThrow();
	});
});
