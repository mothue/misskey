/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import type { Ref } from 'vue';
import { url as host } from '@@/js/config.js';
import { $i } from '@/i.js';
import { fetchBackgroundSlideshowFiles } from '@/utility/background-slideshow-files.js';
import {
	clearBackgroundImageCache,
	getBackgroundImageBlob,
} from '@/utility/background-image-cache.js';

export type BackgroundSlide = { key: number; url: string };
type LoadedSlide = BackgroundSlide & { fileId: string };
const revision = ref(0);

export function refreshBackgroundSlideshow(folderId: string | null): void {
	if (folderId && $i) void clearBackgroundImageCache({ host, accountId: $i.id, folderId }).catch(
			() => {},
		);
	revision.value++;
}
export function useBackgroundSlideshow(options: {
	folderId: Readonly<Ref<string | null>>;
	intervalSec: Readonly<Ref<number>>;
	animation: Readonly<Ref<boolean>>;
}) {
	const current = shallowRef<BackgroundSlide | null>(null);
	const previous = shallowRef<BackgroundSlide | null>(null);
	const state = ref<'loading' | 'ready' | 'empty' | 'error'>('empty');
	let key = 0;
	let displayedScope: string | null = null;
	const retainedUrls = new Set<string>();
	onScopeDispose(() => {
		for (const url of retainedUrls) URL.revokeObjectURL(url);
		retainedUrls.clear();
	});
	watch(
		[options.folderId, () => $i?.id, revision],
		async ([folderId, accountId], _, onCleanup) => {
			const controller = new AbortController();
			const { signal } = controller;
			const ownedUrls = new Set<string>();
			let timer: number | undefined;
			let fade: number | undefined;
			let next: Promise<LoadedSlide | null> | null = null;
			let busy = false;
			let currentId: string | null = null;
			let cursor = 0;
			let images: Awaited<ReturnType<typeof fetchBackgroundSlideshowFiles>> =
				[];

			function release(slide: BackgroundSlide | null) {
				if (slide && ownedUrls.delete(slide.url)) URL.revokeObjectURL(slide.url);
			}

			function finishFade() {
				window.clearTimeout(fade);
				fade = undefined;
				release(previous.value);
				previous.value = null;
				if (!signal.aborted && images.length > 1 && !next) next = loadNext();
			}

			function schedule() {
				window.clearTimeout(timer);
				timer = undefined;
				if (
					signal.aborted ||
					images.length < 2 ||
					window.document.visibilityState !== 'visible'
				) return;
				const value = options.intervalSec.value;
				const seconds = Number.isFinite(value)
					? Math.max(10, Math.min(3600, Math.floor(value)))
					: 60;
				timer = window.setTimeout(() => {
					void advance();
				}, seconds * 1000);
			}

			const visibility = () => {
				if (window.document.visibilityState === 'visible') {
					if (!busy) schedule();
				} else {
					window.clearTimeout(timer);
					timer = undefined;
				}
			};
			const stopIntervalWatch = watch(options.intervalSec, schedule);
			const stopAnimationWatch = watch(options.animation, (enabled) => {
				if (!enabled && previous.value) finishFade();
			});
			window.document.addEventListener('visibilitychange', visibility);
			onCleanup(() => {
				controller.abort();
				stopIntervalWatch();
				stopAnimationWatch();
				window.clearTimeout(timer);
				window.clearTimeout(fade);
				window.document.removeEventListener('visibilitychange', visibility);
				const keepCurrent =
					options.folderId.value === folderId && $i?.id === accountId;
				for (const url of ownedUrls) {
					if (keepCurrent && current.value?.url === url) retainedUrls.add(url);
					else URL.revokeObjectURL(url);
				}
				ownedUrls.clear();
			});
			const sessionScope = JSON.stringify([accountId, folderId]);
			if (current.value && displayedScope === sessionScope) {
				if (retainedUrls.delete(current.value.url)) ownedUrls.add(current.value.url);
			} else {
				current.value = null;
				displayedScope = null;
			}
			previous.value = null;
			if (!folderId || !accountId) {
				state.value = 'empty';
				return;
			}
			const cacheScope = { host, accountId, folderId };

			async function loadNext(): Promise<LoadedSlide | null> {
				for (
					let attempts = 0;
					attempts < images.length && !signal.aborted;
					attempts++
				) {
					const file = images[cursor++ % images.length];
					if (file.id === currentId) continue;
					let imageUrl = file.url;
					try {
						try {
							const blob = await getBackgroundImageBlob(file.url, {
								scope: cacheScope,
								signal,
							});
							signal.throwIfAborted();
							imageUrl = URL.createObjectURL(blob);
							ownedUrls.add(imageUrl);
						} catch {
							signal.throwIfAborted();
							// Images without CORS support can still be decoded and displayed by URL.
						}
						const image = new Image();
						image.src = imageUrl;
						await new Promise<void>((resolve, reject) => {
							const abort = () => {
								image.src = '';
								reject(signal.reason);
							};
							signal.addEventListener('abort', abort, { once: true });
							image
								.decode()
								.then(resolve, reject)
								.finally(() => signal.removeEventListener('abort', abort));
						});
						signal.throwIfAborted();
						return { key: ++key, url: imageUrl, fileId: file.id };
					} catch {
						if (ownedUrls.delete(imageUrl)) URL.revokeObjectURL(imageUrl);
					}
				}
				return null;
			}

			function show(slide: LoadedSlide) {
				previous.value = current.value;
				current.value = slide;
				displayedScope = sessionScope;
				currentId = slide.fileId;
				state.value = 'ready';
				if (previous.value && options.animation.value) fade = window.setTimeout(finishFade, 1000);
				else finishFade();
			}

			async function advance() {
				if (busy || previous.value || signal.aborted) {
					schedule();
					return;
				}
				busy = true;
				const candidate = await (next ?? loadNext());
				next = null;
				if (!signal.aborted && candidate) {
					if (window.document.visibilityState === 'visible') show(candidate);
					else next = Promise.resolve(candidate);
				}
				busy = false;
				schedule();
			}

			try {
				state.value = 'loading';
				images = await fetchBackgroundSlideshowFiles(folderId, signal);
				if (signal.aborted) return;
				if (images.length === 0) {
					release(current.value);
					current.value = null;
					displayedScope = null;
					state.value = 'empty';
					return;
				}
				const first = await loadNext();
				if (signal.aborted) return;
				if (!first) {
					state.value = 'error';
					return;
				}
				show(first);
				schedule();
			} catch {
				if (!signal.aborted) state.value = 'error';
			}
		},
		{ immediate: true },
	);
	return {
		current,
		previous,
		state,
		reload: () => refreshBackgroundSlideshow(options.folderId.value),
	};
}
