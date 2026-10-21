/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { get, set, del, update } from 'idb-keyval';

export type BackgroundImageCacheScope =
	'single' | { host: string; accountId: string; folderId: string };
type Record = { scope: string; url: string; blob: Blob };
const slideshowKey = 'background-slideshow';
const singleKey = 'background-image';
const maxBytes = 100 * 1024 * 1024;
let queue: Promise<unknown> = Promise.resolve();

function serialize<T>(fn: () => Promise<T>): Promise<T> {
	const next = queue.then(fn, fn);
	queue = next.catch(() => {});
	return next;
}

function scopeKey(scope: BackgroundImageCacheScope): string {
	return scope === 'single'
		? scope
		: JSON.stringify([scope.host, scope.accountId, scope.folderId]);
}

function records(saved: unknown): Record[] {
	return Array.isArray(saved)
		? saved.filter(
				(r): r is Record =>
					r != null &&
					typeof r.scope === 'string' &&
					typeof r.url === 'string' &&
					r.blob instanceof Blob &&
					r.blob.size > 0,
			)
		: [];
}

export async function clearBackgroundImageCache(
	scope: BackgroundImageCacheScope,
): Promise<void> {
	if (scope === 'single') return del(singleKey);
	await serialize(() =>
		update(slideshowKey, (saved) =>
			records(saved).filter((r) => r.scope !== scopeKey(scope)),
		),
	);
}
export async function getBackgroundImageBlob(
	url: string,
	options: {
		scope: BackgroundImageCacheScope;
		signal: AbortSignal;
		refresh?: boolean;
	},
): Promise<Blob> {
	const { scope, signal, refresh } = options;
	signal.throwIfAborted();
	if (scope === 'single') {
		const saved = await get<{ url: string; blob: Blob }>(singleKey);
		signal.throwIfAborted();
		if (
			!refresh &&
			saved?.url === url &&
			saved.blob instanceof Blob &&
			saved.blob.size > 0
		) return saved.blob;
		await del(singleKey);
	} else {
		const cached = await serialize(async () => {
			const entry = records(await get(slideshowKey)).find(
				(r) => r.scope === scopeKey(scope) && r.url === url,
			);
			signal.throwIfAborted();
			if (!entry) return null;
			try {
				// Mutations use a single IDB transaction, including between tabs.
				await update(slideshowKey, (saved) => {
					const entries = records(saved);
					const index = entries.findIndex(
						(r) => r.scope === scopeKey(scope) && r.url === url,
					);
					if (index >= 0) {
						const [latest] = entries.splice(index, 1);
						if (!refresh) entries.push(latest);
					}
					return entries;
				});
			} catch {
				// LRU metadata is best-effort: use bytes already read even if writes fail.
			}
			return refresh ? null : entry.blob;
		});
		signal.throwIfAborted();
		if (cached) return cached;
	}
	signal.throwIfAborted();
	const response = await window.fetch(url, {
		credentials: 'omit',
		cache: 'reload',
		signal,
	});
	if (!response.ok) throw new Error('Background image download failed');
	const blob = await response.blob();
	signal.throwIfAborted();
	if (!blob.size || !blob.type.startsWith('image/')) throw new Error('Invalid background image');
	try {
		if (scope === 'single') {
			await set(singleKey, { url, blob });
		} else if (blob.size <= maxBytes) {
			await serialize(() =>
				update(slideshowKey, (saved) => {
					const entries = records(saved).filter(
						(r) => !(r.scope === scopeKey(scope) && r.url === url),
					);
					signal.throwIfAborted();
					entries.push({ scope: scopeKey(scope), url, blob });
					let bytes = entries.reduce((total, r) => total + r.blob.size, 0);
					while (entries.length > 20 || bytes > maxBytes) bytes -= entries.shift()!.blob.size;
					return entries;
				}),
			);
		}
	} catch {
		// A quota or write failure must not hide already downloaded bytes.
	}
	signal.throwIfAborted();
	return blob;
}
