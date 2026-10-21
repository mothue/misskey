/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { beforeEach, afterEach, expect, test, vi } from 'vitest';
import { effectScope, ref, nextTick } from 'vue';
import type { EffectScope } from 'vue';
import { useBackgroundSlideshow } from '@/composables/use-background-slideshow.js';
import { fetchBackgroundSlideshowFiles } from '@/utility/background-slideshow-files.js';
import { getBackgroundImageBlob } from '@/utility/background-image-cache.js';
vi.mock('@/i.js', () => ({ $i: { id: 'user1' } }));
vi.mock('@/utility/background-slideshow-files.js', () => ({
	fetchBackgroundSlideshowFiles: vi.fn(),
}));
vi.mock('@/utility/background-image-cache.js', () => ({
	getBackgroundImageBlob: vi.fn(),
	clearBackgroundImageCache: vi.fn(async () => {}),
}));
const scopes: EffectScope[] = [];
const blobs = new Map<string, Blob>();
function files(count: number) {
	return Array.from({ length: count }, (_, i) => ({
		id: String(i),
		url: `/image${i}.png`,
		type: 'image/png',
	})) as never;
}
function start(interval = 60, animate = true) {
	const folderId = ref<string | null>('folder');
	const intervalSec = ref(interval);
	const animation = ref(animate);
	const scope = effectScope();
	scopes.push(scope);
	return {
		...scope.run(() =>
			useBackgroundSlideshow({ folderId, intervalSec, animation }),
		)!,
		folderId,
		intervalSec,
		animation,
		scope,
	};
}
async function settle() {
	await vi.advanceTimersByTimeAsync(0);
}
async function contents(url: string | undefined) {
	return url ? blobs.get(url)?.text() : null;
}
beforeEach(() => {
	vi.useFakeTimers();
	blobs.clear();
	vi.mocked(fetchBackgroundSlideshowFiles)
		.mockReset()
		.mockResolvedValue(files(3));
	vi.mocked(getBackgroundImageBlob)
		.mockReset()
		.mockImplementation(async (url) => new Blob([url], { type: 'image/png' }));
	let id = 0;
	vi.spyOn(URL, 'createObjectURL').mockImplementation((blob) => {
		const url = `blob:slide${++id}`;
		blobs.set(url, blob as Blob);
		return url;
	});
	vi.spyOn(URL, 'revokeObjectURL').mockImplementation((url) => {
		blobs.delete(url);
	});
	vi.spyOn(Image.prototype, 'decode').mockResolvedValue(undefined);
	vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
});
afterEach(() => {
	for (const scope of scopes.splice(0)) scope.stop();
	vi.useRealTimers();
	vi.restoreAllMocks();
});

test('preloads and changes after 60 seconds, retaining the previous image for a one-second fade', async () => {
	const slides = start();
	await settle();
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	await vi.advanceTimersByTimeAsync(59999);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	await vi.advanceTimersByTimeAsync(1);
	expect(await contents(slides.current.value?.url)).toBe('/image1.png');
	expect(await contents(slides.previous.value?.url)).toBe('/image0.png');
	await vi.advanceTimersByTimeAsync(1000);
	expect(slides.previous.value).toBeNull();
	expect(blobs.size).toBeLessThanOrEqual(2);
});
test.each([0, 1])(
	'does not rotate a folder containing %i images',
	async (count) => {
		vi.mocked(fetchBackgroundSlideshowFiles).mockResolvedValueOnce(
			files(count),
		);
		const slides = start();
		await settle();
		expect(slides.state.value).toBe(count ? 'ready' : 'empty');
		const calls = vi.mocked(getBackgroundImageBlob).mock.calls.length;
		await vi.advanceTimersByTimeAsync(120000);
		expect(getBackgroundImageBlob).toHaveBeenCalledTimes(calls);
	},
);
test('keeps the current image until a delayed next image decodes', async () => {
	let finish!: () => void;
	vi.spyOn(Image.prototype, 'decode')
		.mockResolvedValueOnce(undefined)
		.mockImplementationOnce(
			() =>
				new Promise<void>((resolve) => {
					finish = resolve;
				}),
		);
	const slides = start();
	await settle();
	await vi.advanceTimersByTimeAsync(60000);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	finish();
	await settle();
	expect(await contents(slides.current.value?.url)).toBe('/image1.png');
});
test('skips a broken image and bounds retries when all remaining images fail', async () => {
	vi.spyOn(Image.prototype, 'decode')
		.mockResolvedValueOnce(undefined)
		.mockRejectedValue(new Error('broken'));
	const slides = start();
	await settle();
	await vi.advanceTimersByTimeAsync(60000);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	expect(getBackgroundImageBlob).toHaveBeenCalledTimes(3);
});
test('ignores an old folder response after selecting another folder', async () => {
	let finish!: (value: never) => void;
	vi.mocked(fetchBackgroundSlideshowFiles).mockImplementationOnce(
		() =>
			new Promise<never>((resolve) => {
				finish = resolve;
			}),
	);
	const slides = start();
	slides.folderId.value = 'other';
	await nextTick();
	await settle();
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	finish([] as never);
	await settle();
	expect(slides.state.value).toBe('ready');
});
test('pauses while hidden and starts a full interval on returning', async () => {
	const visibility = vi.spyOn(document, 'visibilityState', 'get');
	const slides = start();
	await settle();
	visibility.mockReturnValue('hidden');
	document.dispatchEvent(new Event('visibilitychange'));
	await vi.advanceTimersByTimeAsync(120000);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	visibility.mockReturnValue('visible');
	document.dispatchEvent(new Event('visibilitychange'));
	await vi.advanceTimersByTimeAsync(59999);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	await vi.advanceTimersByTimeAsync(1);
	expect(await contents(slides.current.value?.url)).toBe('/image1.png');
});
test.each([
	[Number.NaN, 60],
	[1, 10],
	[5000, 3600],
])('normalizes interval %s to %i seconds', async (value, expected) => {
	const slides = start(value, false);
	await settle();
	await vi.advanceTimersByTimeAsync(expected * 1000 - 1);
	expect(await contents(slides.current.value?.url)).toBe('/image0.png');
	await vi.advanceTimersByTimeAsync(1);
	expect(await contents(slides.current.value?.url)).toBe('/image1.png');
	expect(slides.previous.value).toBeNull();
});
test('releases URLs and prevents late decoded images after unmount', async () => {
	let finish!: () => void;
	vi.spyOn(Image.prototype, 'decode')
		.mockResolvedValueOnce(undefined)
		.mockImplementationOnce(
			() =>
				new Promise<void>((resolve) => {
					finish = resolve;
				}),
		);
	const slides = start();
	await settle();
	slides.scope.stop();
	finish();
	await settle();
	expect(blobs.size).toBe(0);
	expect(vi.getTimerCount()).toBe(0);
});

test('keeps the displayed image when reloading the same folder fails', async () => {
	const slides = start();
	await settle();
	const oldUrl = slides.current.value!.url;
	vi.mocked(fetchBackgroundSlideshowFiles).mockRejectedValueOnce(
		new Error('offline'),
	);
	slides.reload();
	await settle();
	expect(slides.state.value).toBe('error');
	expect(slides.current.value?.url).toBe(oldUrl);
	expect(await contents(oldUrl)).toBe('/image0.png');
});
