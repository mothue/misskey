/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import type { entities } from 'misskey-js';
import { misskeyApi } from '@/utility/misskey-api.js';

export async function fetchBackgroundSlideshowFiles(
	folderId: string,
	signal?: AbortSignal,
): Promise<entities.DriveFile[]> {
	const images = new Map<string, entities.DriveFile>();
	let untilId: string | undefined;
	while (true) {
		signal?.throwIfAborted();
		const page = await misskeyApi(
			'drive/files',
			{
				folderId,
				type: 'image/*',
				limit: 100,
				...(untilId ? { untilId } : {}),
			},
			undefined,
			signal,
		);
		signal?.throwIfAborted();
		for (const file of page) if (file.type.startsWith('image/')) images.set(file.id, file);
		if (page.length < 100) break;
		const last = page.at(-1)!.id;
		if (untilId && last >= untilId) throw new Error('Background folder pagination did not advance');
		untilId = last;
	}
	return [...images.values()].sort((a, b) =>
		a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
	);
}
