/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';

export async function waitForHealth(child: { exitCode: number | null }, url: string, ready: () => boolean,
	signal: AbortSignal, timeout = 60000) {
	const start = performance.now();
	while (performance.now() - start < timeout) {
		if (signal.aborted || child.exitCode !== null) throw new Error('server stopped before healthy');
		try {
			const response = await fetch(url, { signal: AbortSignal.any([signal, AbortSignal.timeout(Math.min(1000, timeout))]) });
			await response.arrayBuffer();
			if (response.ok && ready()) return;
		} catch { /* Retry health checks, never benchmark requests. */ }
		await Bun.sleep(Math.min(100, timeout));
	}
	assert.fail('server health timeout');
}
