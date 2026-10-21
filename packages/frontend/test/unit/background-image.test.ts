/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { MockInstance } from 'vitest';
import { effectScope, nextTick, ref } from 'vue';
import type { EffectScope } from 'vue';
import { get, set, del } from 'idb-keyval';
import { refreshBackgroundImage, useBackgroundImage } from '@/composables/use-background-image.js';

// IndexedDB and HTTP are the external boundaries; the Vue watcher and Blob
// lifecycle run normally. Browser persistence is checked separately in Chromium.
vi.mock('idb-keyval', () => ({ get: vi.fn(), set: vi.fn(), del: vi.fn() }));

const firstUrl = 'https://files.example/background.png';
const secondUrl = 'https://files.example/other.png';
const stored = new Map<IDBValidKey, unknown>();
const scopes: EffectScope[] = [];
const objectUrls = new Map<string, Blob>();
let fetchImage: MockInstance<typeof window.fetch>;

function start(url = ref<string | null>(firstUrl)) {
	const scope = effectScope();
	scopes.push(scope);
	return { scope, url, image: scope.run(() => useBackgroundImage(url))! };
}

function imageResponse(body = 'first image') {
	return new Response(new Blob([body], { type: 'image/png' }), {
		headers: { 'Content-Type': 'image/png' },
	});
}

async function displayedImage(image: { value: string | null }) {
	await vi.waitFor(() => expect(image.value).toMatch(/^blob:/));
	return objectUrls.get(image.value!)!;
}

beforeEach(() => {
	stored.clear();
	objectUrls.clear();
	vi.mocked(get).mockReset().mockImplementation(async key => stored.get(key));
	vi.mocked(set).mockReset().mockImplementation(async (key, value) => { stored.set(key, value); });
	vi.mocked(del).mockReset().mockImplementation(async key => { stored.delete(key); });
	fetchImage = vi.spyOn(window, 'fetch').mockReset().mockImplementation(async () => imageResponse());
	let nextId = 0;
	vi.spyOn(URL, 'createObjectURL').mockImplementation(blob => {
		const url = `blob:test-${++nextId}`;
		objectUrls.set(url, blob as Blob);
		return url;
	});
	vi.spyOn(URL, 'revokeObjectURL').mockImplementation(url => { objectUrls.delete(url); });
});

afterEach(() => {
	for (const scope of scopes.splice(0)) scope.stop();
	vi.useRealTimers();
	vi.restoreAllMocks();
});

describe('background image cache', () => {
	test('reuses the saved image after remounting 48 hours later without HTTP', async () => {
		const first = start();
		expect(await (await displayedImage(first.image)).text()).toBe('first image');
		first.scope.stop();
		expect(objectUrls.size).toBe(0);
		fetchImage.mockClear().mockRejectedValue(new Error('offline'));
		vi.setSystemTime(new Date(Date.now() + 48 * 60 * 60 * 1000));
		const reopened = start();
		expect(await (await displayedImage(reopened.image)).text()).toBe('first image');
		expect(fetchImage).not.toHaveBeenCalled();
	});

	test('does not expose the remote URL while the persistent cache is being read', async () => {
		let finishRead!: (value: undefined) => void;
		vi.mocked(get).mockImplementationOnce(() => new Promise(resolve => { finishRead = resolve; }));
		const { image } = start();
		expect(image.value).toBeNull();
		expect(fetchImage).not.toHaveBeenCalled();
		finishRead(undefined);
		await displayedImage(image);
	});

	test('replaces the cached image when the URL changes and releases the old Blob URL', async () => {
		const { image, url } = start();
		await displayedImage(image);
		const previous = image.value;
		fetchImage.mockImplementationOnce(async () => imageResponse('second image'));
		url.value = secondUrl;
		expect(await (await displayedImageAfterChange(image)).text()).toBe('second image');
		expect(objectUrls.has(previous!)).toBe(false);
		expect(stored.size).toBe(1);
		expect(fetchImage.mock.calls.at(-1)?.[0]).toBe(secondUrl);
	});

	test('removing the background deletes the cached image and releases its Blob URL', async () => {
		const { image, url } = start();
		await displayedImage(image);
		url.value = null;
		await vi.waitFor(() => {
			expect(image.value).toBeNull();
			expect(stored.size).toBe(0);
			expect(objectUrls.size).toBe(0);
		});
	});

	test('reselecting the same URL refreshes both the saved image and the HTTP cache', async () => {
		const { image } = start();
		await displayedImage(image);
		fetchImage.mockImplementationOnce(async () => imageResponse('updated image'));
		refreshBackgroundImage();
		expect(await (await displayedImageAfterChange(image)).text()).toBe('updated image');
		expect(fetchImage.mock.calls.at(-1)?.[1]).toMatchObject({ cache: 'reload' });
	});

	test('invalidates the saved image when reselected while the background UI is not mounted', async () => {
		const first = start();
		await displayedImage(first.image);
		first.scope.stop();
		fetchImage.mockImplementationOnce(async () => imageResponse('updated image'));
		refreshBackgroundImage();
		const reopened = start();
		expect(await (await displayedImage(reopened.image)).text()).toBe('updated image');
		expect(fetchImage.mock.calls.at(-1)?.[1]).toMatchObject({ cache: 'reload' });
	});

	test('falls back to the original URL when IndexedDB is unavailable', async () => {
		vi.mocked(get).mockRejectedValueOnce(new Error('storage denied'));
		const { image } = start();
		await vi.waitFor(() => expect(image.value).toBe(firstUrl));
		expect(stored.size).toBe(0);
	});

	test('still displays the downloaded image when writing the cache fails', async () => {
		vi.mocked(set).mockRejectedValueOnce(new Error('quota exceeded'));
		const { image } = start();
		expect(await (await displayedImage(image)).text()).toBe('first image');
	});

	test.each([
		['a failed request', () => Promise.reject(new Error('CORS or network error'))],
		['an HTTP error', async () => new Response('missing', { status: 404 })],
		['a non-image response', async () => new Response('<html>error</html>', { headers: { 'Content-Type': 'text/html' } })],
		['an empty image', async () => imageResponse('')],
	])('does not persist %s and preserves URL rendering', async (_, response) => {
		fetchImage.mockImplementationOnce(response);
		const { image } = start();
		await vi.waitFor(() => expect(image.value).toBe(firstUrl));
		expect(stored.size).toBe(0);
	});

	test('ignores an old download that finishes after changing the background', async () => {
		let finishFirst!: (response: Response) => void;
		fetchImage.mockImplementationOnce(() => new Promise(resolve => { finishFirst = resolve; }));
		const { image, url } = start();
		await vi.waitFor(() => expect(fetchImage).toHaveBeenCalledTimes(1));
		fetchImage.mockImplementationOnce(async () => imageResponse('second image'));
		url.value = secondUrl;
		expect(await (await displayedImageAfterChange(image)).text()).toBe('second image');
		finishFirst(imageResponse('stale image'));
		await nextTick();
		await new Promise(resolve => setTimeout(resolve, 0));
		expect(await objectUrls.get(image.value!)!.text()).toBe('second image');
		// A fresh consumer must also see the second image, not the late result.
		fetchImage.mockClear();
		const reopened = start(ref<string | null>(secondUrl));
		expect(await (await displayedImage(reopened.image)).text()).toBe('second image');
		expect(fetchImage).not.toHaveBeenCalled();
	});

	test('does not restore an image removed while downloading', async () => {
		let finish!: (response: Response) => void;
		fetchImage.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
		const { image, url } = start();
		await vi.waitFor(() => expect(fetchImage).toHaveBeenCalledTimes(1));
		url.value = null;
		await nextTick();
		finish(imageResponse());
		await new Promise(resolve => setTimeout(resolve, 0));
		expect(image.value).toBeNull();
		expect(stored.size).toBe(0);
		expect(objectUrls.size).toBe(0);
	});
});

async function displayedImageAfterChange(image: { value: string | null }) {
	await nextTick();
	return displayedImage(image);
}
