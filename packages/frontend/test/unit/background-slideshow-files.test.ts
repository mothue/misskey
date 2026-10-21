/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { beforeEach, expect, test, vi } from 'vitest';
import { misskeyApi } from '@/utility/misskey-api.js';
import { fetchBackgroundSlideshowFiles } from '@/utility/background-slideshow-files.js';
vi.mock('@/utility/misskey-api.js', () => ({ misskeyApi: vi.fn() }));
function file(id: number, type = 'image/png') {
	return {
		id: String(id).padStart(6, '0'),
		url: `/image${id}.png`,
		type,
	} as never;
}
beforeEach(() => vi.mocked(misskeyApi).mockReset());
test.each([0, 1, 100, 101])(
	'lists all %i images in creation order across page boundaries',
	async (count) => {
		const images = Array.from({ length: count }, (_, i) => file(count - i));
		vi.mocked(misskeyApi)
			.mockResolvedValueOnce(images.slice(0, 100))
			.mockResolvedValueOnce(images.slice(100));
		const result = await fetchBackgroundSlideshowFiles('folder');
		expect(result.map((f) => f.id)).toEqual(
			Array.from({ length: count }, (_, i) => String(i + 1).padStart(6, '0')),
		);
		expect(misskeyApi).toHaveBeenNthCalledWith(
			1,
			'drive/files',
			{ folderId: 'folder', type: 'image/*', limit: 100 },
			undefined,
			undefined,
		);
		if (count >= 100)
			expect(vi.mocked(misskeyApi).mock.calls[1][1]).toMatchObject({
				untilId: '000001'.replace('1', count === 101 ? '2' : '1'),
			});
	},
);
test('filters non-images and duplicate IDs', async () => {
	vi.mocked(misskeyApi).mockResolvedValueOnce([
		file(2),
		file(1),
		file(1),
		file(3, 'text/plain'),
	]);
	expect(
		(await fetchBackgroundSlideshowFiles('folder')).map((f) => f.id),
	).toEqual(['000001', '000002']);
});
test('propagates a page error instead of silently returning a truncated list', async () => {
	vi.mocked(misskeyApi)
		.mockResolvedValueOnce(Array.from({ length: 100 }, (_, i) => file(100 - i)))
		.mockRejectedValueOnce(new Error('denied'));
	await expect(fetchBackgroundSlideshowFiles('folder')).rejects.toThrow(
		'denied',
	);
});
