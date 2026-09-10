<!--
SPDX-FileCopyrightText: syuilo and misskey-project
SPDX-License-Identifier: AGPL-3.0-only
-->

<template>
<div class="gbhvwtnk" :style="`--globalHeaderHeight:${globalHeaderHeight}px`">
	<XHeaderMenu v-if="showMenuOnTop" v-get-size="(_width, height) => globalHeaderHeight = height" @widgetButtonClick="widgetsShowing = true"/>

	<div class="columns" :class="{ fullView, withGlobalHeader: showMenuOnTop }">
		<div v-if="!showMenuOnTop" class="sidebar">
			<XSidebar @widgetButtonClick="widgetsShowing = true"/>
		</div>
		<div v-else-if="!pageMetadata?.needWideArea" class="widgets left">
			<XWidgets place="left"/>
		</div>

		<main class="main" @contextmenu.stop="onContextmenu">
			<div class="content" style="container-type: inline-size;">
				<div>
					<XReloadSuggestion v-if="shouldSuggestReload"/>
					<XPreferenceRestore v-if="shouldSuggestRestoreBackup"/>
					<XThemePreviewing v-if="isThemePreviewMode"/>
					<XAnnouncements v-if="$i"/>
					<XStatusBars class="statusbars"/>
				</div>
				<StackingRouterView v-if="prefer.s['experimental.stackingRouterView']"/>
				<RouterView v-else/>
			</div>
		</main>

		<div v-if="isDesktop && !pageMetadata?.needWideArea" class="widgets right">
			<XWidgets :place="showMenuOnTop ? 'right' : null"/>
		</div>
	</div>

	<iframe v-if="prefer.r.aiChanMode.value" ref="live2d" class="ivnzpscs" src="https://misskey-dev.github.io/mascot-web/?scale=2&y=1.4" :title="i18n.ts.aiChanMode"></iframe>

	<XCommon v-model:widgetsShowing="widgetsShowing"/>
</div>
</template>

<script lang="ts" setup>
import { computed, defineAsyncComponent, onMounted, provide, ref, useTemplateRef } from 'vue';
import XSidebar from './classic.sidebar.vue';
import XCommon from './_common_/common.vue';
import { instanceName } from '@@/js/config.js';
import * as os from '@/os.js';
import { provideMetadataReceiver, provideReactiveMetadata } from '@/page.js';
import type { PageMetadata } from '@/page.js';
import { prefer } from '@/preferences.js';
import { store } from '@/store.js';
import { i18n } from '@/i18n.js';
import { miLocalStorage } from '@/local-storage.js';
import { mainRouter } from '@/router.js';
import { isLink } from '@@/js/is-link.js';
import { DI } from '@/di.js';
import { $i } from '@/i.js';
import { isPreviewMode as isThemePreviewMode } from '@/theme.js';
import { shouldSuggestRestoreBackup } from '@/preferences/utility.js';
import { shouldSuggestReload } from '@/utility/reload-suggest.js';
import XPreferenceRestore from '@/ui/_common_/PreferenceRestore.vue';
import XReloadSuggestion from '@/ui/_common_/ReloadSuggestion.vue';
import XThemePreviewing from '@/ui/_common_/ThemePreviewing.vue';

const XHeaderMenu = defineAsyncComponent(() => import('./classic.header.vue'));
const XWidgets = defineAsyncComponent(() => import('./_common_/widgets.vue'));
const XStatusBars = defineAsyncComponent(() => import('./_common_/statusbars.vue'));
const XAnnouncements = defineAsyncComponent(() => import('./_common_/announcements.vue'));

const isRoot = computed(() => mainRouter.currentRoute.value.name === 'index');

const DESKTOP_THRESHOLD = 1100;

const isDesktop = ref(window.innerWidth >= DESKTOP_THRESHOLD);

const pageMetadata = ref<null | PageMetadata>(null);
const widgetsShowing = ref(false);
const fullView = ref(false);
const globalHeaderHeight = ref(0);
const showMenuOnTop = computed(() => store.r.menuDisplay.value === 'top');
const live2d = useTemplateRef('live2d');

provide(DI.router, mainRouter);
provideMetadataReceiver((metadataGetter) => {
	const info = metadataGetter();
	pageMetadata.value = info;
	if (pageMetadata.value) {
		if (isRoot.value && pageMetadata.value.title === instanceName) {
			window.document.title = pageMetadata.value.title;
		} else {
			window.document.title = `${pageMetadata.value.title} | ${instanceName}`;
		}
	}
});
provideReactiveMetadata(pageMetadata);
provide('shouldHeaderThin', showMenuOnTop.value);
provide('forceSpacerMin', true);

function onContextmenu(ev: PointerEvent) {
	if (isLink(ev.target as HTMLElement)) return;
	if (['INPUT', 'TEXTAREA', 'IMG', 'VIDEO', 'CANVAS'].includes((ev.target as HTMLElement).tagName) || (ev.target as HTMLElement).attributes.getNamedItem('contenteditable') != null) return;
	if (window.getSelection()?.toString() !== '') return;
	const path = mainRouter.getCurrentFullPath();
	os.contextMenu([{
		type: 'label',
		text: path,
	}, {
		icon: fullView.value ? 'ti ti-minimize' : 'ti ti-maximize',
		text: fullView.value ? i18n.ts.quitFullView : i18n.ts.fullView,
		action: () => {
			fullView.value = !fullView.value;
		},
	}, {
		icon: 'ti ti-window-maximize',
		text: i18n.ts.openInWindow,
		action: () => {
			os.pageWindow(path);
		},
	}], ev);
}

// 1024px未満のモバイル幅では自動的にdefault UIへフォールバック
if (window.innerWidth < 1024) {
	const currentUI = miLocalStorage.getItem('ui');
	miLocalStorage.setItem('ui_temp', currentUI ?? 'default');
	miLocalStorage.setItem('ui', 'default');
	const url = new URL(window.location.href);
	url.searchParams.delete('ui');
	window.location.replace(url);
}

window.document.documentElement.style.overflowY = 'scroll';

onMounted(() => {
	window.addEventListener('resize', () => {
		isDesktop.value = (window.innerWidth >= DESKTOP_THRESHOLD);
	}, { passive: true });

	if (prefer.s.aiChanMode && live2d.value) {
		const iframeRect = live2d.value.getBoundingClientRect();
		window.addEventListener('mousemove', ev => {
			live2d.value?.contentWindow?.postMessage({
				type: 'moveCursor',
				body: {
					x: ev.clientX - iframeRect.left,
					y: ev.clientY - iframeRect.top,
				},
			}, '*');
		}, { passive: true });
		window.addEventListener('touchmove', ev => {
			const touch = ev.touches[0];
			if (!touch) return;
			live2d.value?.contentWindow?.postMessage({
				type: 'moveCursor',
				body: {
					x: touch.clientX - iframeRect.left,
					y: touch.clientY - iframeRect.top,
				},
			}, '*');
		}, { passive: true });
	}
});
</script>

<style lang="scss" scoped>
.gbhvwtnk {
	$ui-font-size: 1em;
	$widgets-hide-threshold: 1200px;

	min-height: 100dvh;
	box-sizing: border-box;

	> .columns {
		display: flex;
		justify-content: center;
		max-width: 100%;

		&.fullView {
			margin: 0;

			> .sidebar {
				display: none;
			}

			> .widgets {
				display: none;
			}

			> .main {
				margin: 0;
				border-radius: 0;
				box-shadow: none;
				width: 100%;
			}
		}

		> .main {
			min-width: 0;
			width: 750px;
			margin: 0 16px 0 0;
			border-left: solid 1px var(--MI_THEME-divider);
			border-right: solid 1px var(--MI_THEME-divider);
			border-radius: 0;
			overflow: clip;
			--MI-margin: 12px;
		}

		> .widgets {
			width: 300px;
			position: sticky;
			top: var(--MI-stickyTop, 0);
			align-self: flex-start;
			padding-top: var(--MI-margin);
			padding-bottom: calc(var(--MI-margin) + env(safe-area-inset-bottom, 0px));

			@media (max-width: $widgets-hide-threshold) {
				display: none;
			}

			&.left {
				margin-right: 16px;
			}
		}

		> .sidebar {
			margin-top: 16px;
		}

		&.withGlobalHeader {
			> .main {
				margin-top: 0;
				border: solid 1px var(--MI_THEME-divider);
				border-radius: var(--MI-radius);
				--MI-stickyTop: var(--globalHeaderHeight);
			}

			> .widgets {
				--MI-stickyTop: var(--globalHeaderHeight);
				margin-top: 0;

				&.right {
					padding-top: 0;
				}
			}
		}

		@media (max-width: 850px) {
			margin: 0;

			> .sidebar {
				border-right: solid 0.5px var(--MI_THEME-divider);
			}

			> .main {
				margin: 0;
				border-radius: 0;
				box-shadow: none;
				width: 100%;
			}
		}
	}

	> .ivnzpscs {
		position: fixed;
		bottom: 0;
		right: 0;
		width: 300px;
		height: 600px;
		border: none;
		pointer-events: none;
	}
}

.content {
	height: 100%;
}

.statusbars {
	position: sticky;
	top: var(--MI-stickyTop, 0);
	left: 0;
}
</style>
