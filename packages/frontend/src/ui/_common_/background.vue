<!--
SPDX-FileCopyrightText: syuilo and misskey-project
SPDX-License-Identifier: AGPL-3.0-only
-->
<template>
	<div
		v-if="active"
		class="_backgroundImageLayer"
		:class="$style.root"
		data-testid="background"
	>
		<template v-if="isSlideshow && current">
			<div
				v-if="previous"
				:class="$style.image"
				:style="imageStyle(previous.url)"
				data-testid="background-previous"
			></div>
			<div
				:key="current.key"
				:class="[$style.image, { [$style.fadeIn]: previous && animation }]"
				:style="imageStyle(current.url)"
				data-testid="background-current"
			></div>
		</template>
		<div
			v-else
			:class="$style.image"
			:style="imageStyle(singleImage!)"
			data-testid="background-current"
		></div>
		<div :class="$style.darkness"></div>
	</div>
</template>
<script lang="ts" setup>
import { computed, onScopeDispose, ref, watch } from 'vue';
import { prefer } from '@/preferences.js';
import { useBackgroundImage } from '@/composables/use-background-image.js';
import { useBackgroundSlideshow } from '@/composables/use-background-slideshow.js';

const emit = defineEmits<{ (event: 'active', value: boolean): void }>();
const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
const reducedMotion = ref(motion.matches);
const updateMotion = () => {
	reducedMotion.value = motion.matches;
};
motion.addEventListener('change', updateMotion);
onScopeDispose(() => motion.removeEventListener('change', updateMotion));
const animation = computed(
	() => prefer.r.animation.value && !reducedMotion.value,
);
const isSlideshow = computed(
	() => prefer.r['backgroundImage.mode'].value === 'slideshow',
);
const singleImage = useBackgroundImage(prefer.r['backgroundImage.url']);
const { current, previous } = useBackgroundSlideshow({
	folderId: computed(() =>
		isSlideshow.value
			? prefer.r['backgroundImage.slideshowFolderId'].value
			: null,
	),
	intervalSec: prefer.r['backgroundImage.slideshowInterval'],
	animation,
});
const active = computed(() =>
	Boolean((isSlideshow.value && current.value) || singleImage.value),
);
watch(active, (value) => emit('active', value), { immediate: true });

function imageStyle(url: string) {
	return {
		backgroundImage: `url(${JSON.stringify(url)})`,
		filter: `blur(${prefer.r['backgroundImage.blur'].value}px)`,
	};
}
</script>
<style lang="scss" module>
.root {
	position: fixed;
	z-index: -1000;
	inset: -50px;
	pointer-events: none;
}
.image {
	position: absolute;
	inset: 0;
	background-size: cover;
	background-position: center;
}
.fadeIn {
	animation: fadeIn 1000ms linear both;
}
@keyframes fadeIn {
	from {
		opacity: 0;
	}
	to {
		opacity: 1;
	}
}
.darkness {
	position: absolute;
	inset: 0;
	background: light-dark(var(--MI_THEME-fg), var(--MI_THEME-bg));
	opacity: var(--MI-backgroundImageDarkness);
}
</style>
