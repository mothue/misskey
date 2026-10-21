<!--
SPDX-FileCopyrightText: syuilo and misskey-project
SPDX-License-Identifier: AGPL-3.0-only
-->
<template>
	<SearchMarker
		path="/settings/background"
		:label="i18n.ts._backgroundImage.title"
		:keywords="['background', 'wallpaper', 'image']"
		icon="ti ti-photo"
	>
		<div class="_gaps_m">
			<MkPreferenceContainer k="backgroundImage.mode">
				<MkSelect
					v-model="mode"
					data-testid="background-mode"
					:items="[
						{ value: 'single', label: i18n.ts._backgroundImage.single },
						{ value: 'slideshow', label: i18n.ts._backgroundImage.slideshow },
					]"
				>
					<template #label
						><SearchLabel>{{
							i18n.ts._backgroundImage.mode
						}}</SearchLabel></template
					>
				</MkSelect>
			</MkPreferenceContainer>
			<div v-if="mode === 'slideshow'" class="_gaps_m">
				<MkPreferenceContainer k="backgroundImage.slideshowFolderId"
					><div class="_gaps_s">
						<MkButton data-testid="background-folder" @click="chooseFolder"
							><i class="ti ti-folder"></i>
							{{ i18n.ts._backgroundImage.selectFolder }}</MkButton
						>
						<div v-if="folderId">
							{{ selectedFolderName ?? i18n.ts.folder }}
						</div>
						<div>{{ i18n.ts._backgroundImage.folderDescription }}</div>
					</div></MkPreferenceContainer
				>
				<MkPreferenceContainer k="backgroundImage.slideshowInterval"
					><MkRange
						v-model="interval"
						data-testid="background-interval"
						:min="10"
						:max="3600"
						:step="1"
						><template #label
							><SearchLabel>{{
								i18n.ts._backgroundImage.interval
							}}</SearchLabel></template
						><template #suffix>{{ i18n.ts._time.second }}</template></MkRange
					></MkPreferenceContainer
				>
				<MkButton
					v-if="folderId"
					data-testid="background-reload"
					@click="reloadFolder"
					><i class="ti ti-refresh"></i> {{ i18n.ts.reload }}</MkButton
				>
				<p v-if="folderState === 'loading'">{{ i18n.ts.loading }}</p>
				<p v-else-if="folderState === 'empty'" data-testid="background-empty">
					{{ i18n.ts._backgroundImage.emptyFolder }}
				</p>
				<p v-else-if="folderState === 'error'" data-testid="background-error">
					{{ i18n.ts._backgroundImage.folderError }}
				</p>
			</div>
			<div v-if="mode === 'slideshow'">
				{{ i18n.ts._backgroundImage.fallbackDescription }}
			</div>
			<div class="_buttons">
				<MkButton data-testid="background-select" @click="selectBackgroundImage"
					><i class="ti ti-photo"></i>
					<SearchLabel>{{
						backgroundImageUrl ? i18n.ts.edit : i18n.ts.selectFile
					}}</SearchLabel></MkButton
				>
				<MkButton
					v-if="backgroundImageUrl"
					data-testid="background-remove"
					danger
					@click="removeBackgroundImage"
					><i class="ti ti-trash"></i> {{ i18n.ts.remove }}</MkButton
				>
			</div>
			<MkPreferenceContainer k="backgroundImage.opacity"
				><MkRange
					v-model="backgroundImageOpacity"
					:min="0"
					:max="1"
					:step="0.01"
					:textConverter="toPercentage"
					><template #label
						><SearchLabel>{{
							i18n.ts._backgroundImage.opacity
						}}</SearchLabel></template
					></MkRange
				></MkPreferenceContainer
			>
			<MkPreferenceContainer k="backgroundImage.darkness"
				><MkRange
					v-model="backgroundImageDarkness"
					:min="0"
					:max="1"
					:step="0.01"
					:textConverter="toPercentage"
					><template #label
						><SearchLabel>{{
							i18n.ts._backgroundImage.darkness
						}}</SearchLabel></template
					></MkRange
				></MkPreferenceContainer
			>
			<MkPreferenceContainer k="backgroundImage.blur"
				><MkRange v-model="backgroundImageBlur" :min="0" :max="30" :step="1"
					><template #label
						><SearchLabel>{{
							i18n.ts._backgroundImage.blur
						}}</SearchLabel></template
					><template #suffix>px</template></MkRange
				></MkPreferenceContainer
			>
		</div>
	</SearchMarker>
</template>
<script lang="ts" setup>
import { onScopeDispose, ref, watch } from 'vue';
import MkSelect from '@/components/MkSelect.vue';
import MkButton from '@/components/MkButton.vue';
import MkRange from '@/components/MkRange.vue';
import MkPreferenceContainer from '@/components/MkPreferenceContainer.vue';
import { prefer } from '@/preferences.js';
import { i18n } from '@/i18n.js';
import * as os from '@/os.js';
import { definePage } from '@/page.js';
import { ensureSignin } from '@/i.js';
import { selectFile, selectDriveFolder } from '@/utility/drive.js';
import { refreshBackgroundImage } from '@/composables/use-background-image.js';
import { refreshBackgroundSlideshow } from '@/composables/use-background-slideshow.js';
import { fetchBackgroundSlideshowFiles } from '@/utility/background-slideshow-files.js';

ensureSignin();
const mode = prefer.model('backgroundImage.mode');
const folderId = prefer.model('backgroundImage.slideshowFolderId');
const interval = prefer.model('backgroundImage.slideshowInterval');
const folderState = ref<'loading' | 'ready' | 'empty' | 'error' | null>(null);
const selectedFolderName = ref<string | null>(null);
const backgroundImageUrl = prefer.model('backgroundImage.url');
const backgroundImageOpacity = prefer.model('backgroundImage.opacity');
const backgroundImageDarkness = prefer.model('backgroundImage.darkness');
const backgroundImageBlur = prefer.model('backgroundImage.blur');
const toPercentage = (value: number) => `${Math.round(value * 100)}%`;

let folderRequest = 0;
let folderController: AbortController | null = null;
onScopeDispose(() => {
	folderRequest++;
	folderController?.abort();
});
watch(
	[folderId, mode],
	() => {
		void checkFolder();
	},
	{ immediate: true },
);

async function checkFolder() {
	const request = ++folderRequest;
	folderController?.abort();
	folderController = new AbortController();
	if (!folderId.value || mode.value !== 'slideshow') {
		folderState.value = null;
		return;
	}
	folderState.value = 'loading';
	try {
		const images = await fetchBackgroundSlideshowFiles(
			folderId.value,
			folderController.signal,
		);
		if (request === folderRequest) folderState.value = images.length ? 'ready' : 'empty';
	} catch {
		if (request === folderRequest) folderState.value = 'error';
	}
}

async function chooseFolder() {
	const selection = await selectDriveFolder(folderId.value);
	if (selection.canceled) return;
	const folder = selection.folders[0];
	if (!folder) {
		os.alert({
			type: 'warning',
			text: i18n.ts._backgroundImage.selectSpecificFolder,
		});
		return;
	}
	folderId.value = folder.id;
	selectedFolderName.value = folder.name;
	reloadFolder();
}

function reloadFolder() {
	refreshBackgroundSlideshow(folderId.value);
	void checkFolder();
}

async function selectBackgroundImage(ev: PointerEvent) {
	const file = await selectFile({
		anchorElement: ev.currentTarget ?? ev.target,
		multiple: false,
		label: i18n.ts.selectFile,
	});
	if (!file.type.startsWith('image/')) {
		os.alert({
			type: 'warning',
			title: i18n.ts._watermarkEditor.driveFileTypeWarn,
			text: i18n.ts._watermarkEditor.driveFileTypeWarnDescription,
		});
		return;
	}
	backgroundImageUrl.value = file.url;
	refreshBackgroundImage();
}

async function removeBackgroundImage() {
	const { canceled } = await os.confirm({
		type: 'warning',
		text: i18n.tsx.deleteAreYouSure({ x: i18n.ts._backgroundImage.title }),
	});
	if (canceled) return;
	backgroundImageUrl.value = null;
	refreshBackgroundImage();
}

definePage(() => ({
	title: i18n.ts._backgroundImage.title,
	icon: 'ti ti-photo',
}));
</script>
