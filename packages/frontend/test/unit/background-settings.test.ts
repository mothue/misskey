/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/vue';
import { ref } from 'vue';
import { prefer } from '@/preferences.js';
import * as os from '@/os.js';
import { selectFile, selectDriveFolder } from '@/utility/drive.js';
import { refreshBackgroundImage } from '@/composables/use-background-image.js';
import BackgroundSettings from '@/pages/settings/background.vue';

vi.mock('@/preferences.js', () => ({
	prefer: { model: vi.fn(), s: {}, r: {} },
}));
vi.mock('@/i.js', () => ({ ensureSignin: () => ({ id: 'user1' }), $i: null }));
vi.mock('@/os.js', () => ({ alert: vi.fn(), confirm: vi.fn() }));
vi.mock('@/utility/drive.js', () => ({
	selectFile: vi.fn(),
	selectDriveFolder: vi.fn(),
}));
vi.mock('@/composables/use-background-slideshow.js', () => ({
	refreshBackgroundSlideshow: vi.fn(),
}));
vi.mock('@/utility/background-slideshow-files.js', () => ({
	fetchBackgroundSlideshowFiles: vi.fn(async () => []),
}));
vi.mock('@/components/MkSelect.vue', () => ({
	default: {
		props: ['modelValue', 'items'],
		emits: ['update:modelValue'],
		template:
			'<select :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :value="item.value">{{ item.label }}</option></select>',
	},
}));
vi.mock('@/composables/use-background-image.js', () => ({
	refreshBackgroundImage: vi.fn(),
}));
vi.mock('@/page.js', () => ({ definePage: vi.fn() }));
vi.mock('@/components/MkButton.vue', () => ({
	default: {
		emits: ['click'],
		template: '<button @click="$emit(\'click\', $event)"><slot/></button>',
	},
}));
vi.mock('@/components/MkRange.vue', () => ({
	default: {
		props: ['modelValue'],
		emits: ['update:modelValue'],
		template:
			'<label><slot name="label"/><input type="range" :value="modelValue" @input="$emit(\'update:modelValue\', Number($event.target.value))"/></label>',
	},
}));
vi.mock('@/components/MkPreferenceContainer.vue', () => ({
	default: { template: '<div><slot/></div>' },
}));

const settings = new Map<string, ReturnType<typeof ref>>();
const slot = { template: '<div><slot/><slot name="label"/></div>' };
function show() {
	return render(BackgroundSettings, {
		global: {
			stubs: {
				SearchMarker: slot,
				SearchLabel: slot,
				SearchText: slot,
				MkPreferenceContainer: slot,
			},
		},
	});
}
beforeEach(() => {
	vi.clearAllMocks();
	settings.clear();
	for (const [key, value] of Object.entries({
		'backgroundImage.mode': 'single',
		'backgroundImage.slideshowFolderId': null,
		'backgroundImage.slideshowInterval': 60,
		'backgroundImage.url': 'https://files.example/old.png',
		'backgroundImage.opacity': 0.72,
		'backgroundImage.darkness': 0.2,
		'backgroundImage.blur': 0,
	}))
		settings.set(key, ref(value));
	vi.mocked(prefer.model).mockImplementation(
		(key: string) => settings.get(key) as never,
	);
});
afterEach(cleanup);

test('keeps existing settings and selects a new image through the page', async () => {
	const view = show();
	expect(settings.get('backgroundImage.opacity')!.value).toBe(0.72);
	vi.mocked(selectFile).mockResolvedValueOnce({
		url: 'https://files.example/new.png',
		type: 'image/png',
	} as never);
	await fireEvent.click(view.getByRole('button', { name: /Edit|編集/ }));
	expect(settings.get('backgroundImage.url')!.value).toBe(
		'https://files.example/new.png',
	);
	expect(refreshBackgroundImage).toHaveBeenCalledOnce();
});

test('only removes the image after confirmation', async () => {
	const view = show();
	vi.mocked(os.confirm).mockResolvedValueOnce({ canceled: true });
	await fireEvent.click(view.getByTestId('background-remove'));
	expect(settings.get('backgroundImage.url')!.value).toBe(
		'https://files.example/old.png',
	);
	vi.mocked(os.confirm).mockResolvedValueOnce({ canceled: false });
	await fireEvent.click(view.getByTestId('background-remove'));
	expect(settings.get('backgroundImage.url')!.value).toBeNull();
	expect(refreshBackgroundImage).toHaveBeenCalledOnce();
});

test('rejects a non-image without changing the saved background', async () => {
	const view = show();
	vi.mocked(selectFile).mockResolvedValueOnce({
		url: 'https://files.example/file.txt',
		type: 'text/plain',
	} as never);
	await fireEvent.click(view.getByRole('button', { name: /Edit|編集/ }));
	expect(settings.get('backgroundImage.url')!.value).toBe(
		'https://files.example/old.png',
	);
	expect(os.alert).toHaveBeenCalledOnce();
	expect(refreshBackgroundImage).not.toHaveBeenCalled();
});

test('the moved opacity control updates the existing preference key', async () => {
	const view = show();
	const range = view.getByLabelText(/Panel opacity|パネルの不透明度/);
	expect((range as HTMLInputElement).value).toBe('0.72');
	await fireEvent.input(range, { target: { value: '0.4' } });
	expect(settings.get('backgroundImage.opacity')!.value).toBe(0.4);
});
test('selecting a folder sets the account-scoped folder and cancel preserves it', async () => {
	settings.get('backgroundImage.mode')!.value = 'slideshow';
	const view = show();
	vi.mocked(selectDriveFolder).mockResolvedValueOnce({
		canceled: false,
		folders: [{ id: 'folder1', name: 'Slides' }],
	} as never);
	await fireEvent.click(view.getByTestId('background-folder'));
	expect(settings.get('backgroundImage.slideshowFolderId')!.value).toBe(
		'folder1',
	);
	vi.mocked(selectDriveFolder).mockResolvedValueOnce({
		canceled: true,
		folders: undefined,
	});
	await fireEvent.click(view.getByTestId('background-folder'));
	expect(settings.get('backgroundImage.slideshowFolderId')!.value).toBe(
		'folder1',
	);
});
test('does not accept the drive root as a slideshow folder', async () => {
	settings.get('backgroundImage.mode')!.value = 'slideshow';
	const view = show();
	vi.mocked(selectDriveFolder).mockResolvedValueOnce({
		canceled: false,
		folders: [null],
	});
	await fireEvent.click(view.getByTestId('background-folder'));
	expect(settings.get('backgroundImage.slideshowFolderId')!.value).toBeNull();
	expect(os.alert).toHaveBeenCalledOnce();
});
