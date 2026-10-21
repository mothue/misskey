/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { ref, shallowRef, watch } from 'vue';
import type { Ref } from 'vue';
import { clearBackgroundImageCache, getBackgroundImageBlob } from '@/utility/background-image-cache.js';

const revision = ref(0);

// Reselecting the same file must also invalidate its saved contents.
export function refreshBackgroundImage(): void {
	// Settings can also be opened from another UI where this composable is
	// not mounted. Invalidate the persistent copy in that case as well.
	void clearBackgroundImageCache('single').catch(() => {});
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
				await clearBackgroundImageCache('single');
				return;
			}

			const blob = await getBackgroundImageBlob(source, {
				scope: 'single',
				signal: controller.signal,
				refresh: previous.length > 0 && version !== previous[1],
			});

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
