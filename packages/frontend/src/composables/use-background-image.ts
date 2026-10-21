/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { ref, shallowRef, watch } from 'vue';
import type { Ref } from 'vue';
import { get, set, del } from 'idb-keyval';

const cacheKey = 'background-image';
const revision = ref(0);

type CachedBackgroundImage = {
	url: string;
	blob: Blob;
};

// Reselecting the same file must also invalidate its saved contents.
export function refreshBackgroundImage(): void {
	// Settings can also be opened from another UI where this composable is
	// not mounted. Invalidate the persistent copy in that case as well.
	void del(cacheKey).catch(() => {});
	revision.value++;
}

export function useBackgroundImage(url: Ref<string | null>): Ref<string | null> {
	const image = shallowRef<string | null>(null);

	watch([() => url.value?.trim() || null, revision], async ([source, version], previous, onCleanup) => {
		const controller = new AbortController();
		let objectUrl: string | null = null;
		onCleanup(() => {
			controller.abort();
			if (objectUrl) URL.revokeObjectURL(objectUrl);
		});

		// Do not send the source URL to CSS before checking IndexedDB: doing so
		// would start a network request even when we already have the image.
		image.value = null;
		try {
			if (!source) {
				await del(cacheKey);
				return;
			}

			// Use IndexedDB directly: idb-proxy's JSON/localStorage fallback cannot
			// preserve Blobs. The default store is also cleared during signout.
			const cached = await get<CachedBackgroundImage>(cacheKey);
			if (controller.signal.aborted) return;
			const refresh = previous.length > 0 && version !== previous[1];
			let blob: Blob;
			if (!refresh && cached?.url === source && cached.blob instanceof Blob && cached.blob.size > 0) {
				blob = cached.blob;
			} else {
				await del(cacheKey);
				if (controller.signal.aborted) return;
				const response = await window.fetch(source, {
					signal: controller.signal,
					credentials: 'omit',
					// When the saved copy is missing or invalidated, get fresh bytes
					// even if an HTTP cache still contains the previous file.
					cache: 'reload',
				});
				if (!response.ok) throw new Error('Background image download failed');
				blob = await response.blob();
				if (blob.size === 0 || !blob.type.startsWith('image/')) throw new Error('Invalid background image');
				if (controller.signal.aborted) return;
				try {
					await set(cacheKey, { url: source, blob } satisfies CachedBackgroundImage);
				} catch {
					// Storage quota must not prevent displaying the downloaded image.
				}
			}

			if (controller.signal.aborted) return;
			objectUrl = URL.createObjectURL(blob);
			image.value = objectUrl;
		} catch {
			// Keep existing rendering for unavailable storage, CORS restrictions,
			// or download errors. Never restore a superseded background.
			if (!controller.signal.aborted) image.value = source;
		}
	}, { immediate: true });

	return image;
}
