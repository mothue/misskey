/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { beforeEach, afterEach, expect, test, vi } from 'vitest';
import { get, set, del, update } from 'idb-keyval';
import {
	getBackgroundImageBlob,
	clearBackgroundImageCache,
} from '@/utility/background-image-cache.js';
vi.mock('idb-keyval', () => ({
	get: vi.fn(),
	set: vi.fn(),
	del: vi.fn(),
	update: vi.fn(),
}));
const data = new Map<IDBValidKey, unknown>();
const scope = {
	host: 'https://example.test',
	accountId: 'user1',
	folderId: 'folder1',
};
const signal = () => new AbortController().signal;
let network = vi.fn<typeof window.fetch>();
function load(url: string, cacheScope = scope) {
	return getBackgroundImageBlob(url, { scope: cacheScope, signal: signal() });
}
function blob(size = 5) {
	const value = new Blob(['image'], { type: 'image/png' });
	// Exercise byte-budget accounting without allocating hundreds of MiB in unit tests.
	Object.defineProperty(value, 'size', { value: size });
	return value;
}
beforeEach(() => {
	data.clear();
	vi.mocked(get)
		.mockReset()
		.mockImplementation(async (key) => data.get(key));
	vi.mocked(set)
		.mockReset()
		.mockImplementation(async (key, value) => {
			data.set(key, value);
		});
	vi.mocked(del)
		.mockReset()
		.mockImplementation(async (key) => {
			data.delete(key);
		});
	vi.mocked(update)
		.mockReset()
		.mockImplementation(async (key, updater) => {
			await set(key, updater(data.get(key)));
		});

	network = vi
		.fn<typeof window.fetch>()
		.mockImplementation(
			async () => new Response(new Blob(['image'], { type: 'image/png' })),
		);
	vi.spyOn(window, 'fetch').mockImplementation(network);
});
afterEach(() => vi.restoreAllMocks());

test('reads the existing single-image record without a request', async () => {
	data.set('background-image', { url: '/old.png', blob: blob() });
	expect(
		await (
			await getBackgroundImageBlob('/old.png', {
				scope: 'single',
				signal: signal(),
			})
		).text(),
	).toBe('image');
	expect(network).not.toHaveBeenCalled();
});
test('isolates account, host and folder while reusing the matching cached image', async () => {
	await load('/image.png');
	await load('/image.png');
	expect(network).toHaveBeenCalledTimes(1);
	await load('/image.png', { ...scope, accountId: 'user2' });
	await load('/image.png', { ...scope, host: 'https://other.test' });
	await load('/image.png', { ...scope, folderId: 'other' });
	expect(network).toHaveBeenCalledTimes(4);
});
test('keeps at most 20 images and evicts the least recently used one', async () => {
	for (let i = 0; i < 20; i++) await load(`/image${i}.png`);
	await load('/image0.png');
	await load('/image20.png');
	network.mockClear();
	await load('/image0.png');
	expect(network).not.toHaveBeenCalled();
	await load('/image1.png');
	expect(network).toHaveBeenCalledOnce();
});
test('serializes concurrent writes so both downloaded images can be reused', async () => {
	await Promise.all([load('/a.png'), load('/b.png')]);
	network.mockClear();
	await load('/a.png');
	await load('/b.png');
	expect(network).not.toHaveBeenCalled();
});
test('enforces the 100 MiB budget and does not persist an oversized image', async () => {
	network.mockResolvedValueOnce({
		ok: true,
		blob: async () => blob(100 * 1024 * 1024),
	} as Response);
	await load('/large.png');
	network.mockClear();
	await load('/large.png');
	expect(network).not.toHaveBeenCalled();
	await load('/small.png');
	network.mockClear();
	await load('/large.png');
	expect(network).toHaveBeenCalledOnce();
	network.mockResolvedValueOnce({
		ok: true,
		blob: async () => blob(100 * 1024 * 1024 + 1),
	} as Response);
	await load('/too-large.png');
	network.mockClear();
	await load('/too-large.png');
	expect(network).toHaveBeenCalledOnce();
});
test('clears only the requested scope and preserves the other folder', async () => {
	const other = { ...scope, folderId: 'other' };
	await load('/a.png');
	await load('/b.png', other);
	await clearBackgroundImageCache(scope);
	network.mockClear();
	await load('/b.png', other);
	expect(network).not.toHaveBeenCalled();
	await load('/a.png');
	expect(network).toHaveBeenCalledOnce();
});
test('returns downloaded bytes even if writing IndexedDB fails', async () => {
	vi.mocked(set).mockRejectedValueOnce(new Error('quota'));
	expect(await (await load('/a.png')).text()).toBe('image');
});
test('does not persist an image when its request was superseded', async () => {
	const controller = new AbortController();
	network.mockImplementationOnce(async () => {
		controller.abort();
		return new Response(blob());
	});
	await expect(
		getBackgroundImageBlob('/a.png', { scope, signal: controller.signal }),
	).rejects.toThrow();
	network.mockClear();
	await load('/a.png');
	expect(network).toHaveBeenCalledOnce();
});

test('can reuse saved image bytes when only the LRU metadata write fails', async () => {
	await load('/cached.png');
	network.mockClear();
	vi.mocked(set).mockRejectedValueOnce(new Error('storage is read-only'));
	expect(await (await load('/cached.png')).text()).toBe('image');
	expect(network).not.toHaveBeenCalled();
});

test('does not lose records when independent tabs update the same IndexedDB cache', async () => {
	vi.resetModules();
	const firstTab = await import('@/utility/background-image-cache.js');
	vi.resetModules();
	const secondTab = await import('@/utility/background-image-cache.js');
	await Promise.all([
		firstTab.getBackgroundImageBlob('/tab-a.png', { scope, signal: signal() }),
		secondTab.getBackgroundImageBlob('/tab-b.png', { scope, signal: signal() }),
	]);
	network.mockClear();
	await firstTab.getBackgroundImageBlob('/tab-a.png', {
		scope,
		signal: signal(),
	});
	await secondTab.getBackgroundImageBlob('/tab-b.png', {
		scope,
		signal: signal(),
	});
	expect(network).not.toHaveBeenCalled();
});
