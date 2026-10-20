/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { Address4, Address6 } from 'ip-address';

function parseIp(ip: string): Address4 | Address6 {
	const address = ip.includes(':') ? new Address6(ip) : new Address4(ip);
	return address instanceof Address6 && address.address4 ? address.address4 : address;
}

export function getIpHash(ip: string): string {
	try {
		// because a single person may control many IPv6 addresses,
		// only a /64 subnet prefix of any IP will be taken into account.
		// (this means for IPv4 the entire address is used)
		const prefix = parseIp(ip).mask(64);
		return 'ip-' + BigInt('0b' + prefix).toString(36);
	} catch (_) {
		const prefix = parseIp(ip.replace(/:[0-9]+$/, '')).mask(64);
		return 'ip-' + BigInt('0b' + prefix).toString(36);
	}
}
