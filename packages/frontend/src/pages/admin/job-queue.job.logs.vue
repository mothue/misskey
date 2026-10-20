<!--
SPDX-FileCopyrightText: syuilo and misskey-project
SPDX-License-Identifier: AGPL-3.0-only
-->

<template>
<div class="_gaps_s">
	<div v-if="entries.length === 0">{{ i18n.ts._jobQueue.noLogs }}</div>
	<template v-for="(entry, index) in entries" :key="`${index}:${entry.raw}`">
		<div v-if="entry.record" class="_gaps_s">
			<MkTime :time="entry.record.timestamp" mode="detail"/>
			<div><i class="ti ti-alert-triangle"></i> {{ entry.record.error.type }}: {{ entry.record.error.message }}</div>
			<MkKeyValue v-if="entry.record.code != null">
				<template #key>{{ i18n.ts.code }}</template>
				<template #value>{{ entry.record.code }}</template>
			</MkKeyValue>
			<MkKeyValue v-if="entry.record.reason">
				<template #key>{{ i18n.ts._jobQueue.reason }}</template>
				<template #value>{{ entry.record.reason }}</template>
			</MkKeyValue>
			<MkKeyValue v-if="entry.record.property">
				<template #key>{{ i18n.ts._jobQueue.property }}</template>
				<template #value>{{ entry.record.property }}</template>
			</MkKeyValue>
			<MkFolder>
				<template #label>{{ i18n.ts.details }}</template>
				<MkCode :code="JSON.stringify(entry.record.error, null, '\t')" lang="json" :forceShow="true"/>
			</MkFolder>
		</div>
		<pre v-else :class="$style.legacy">{{ entry.raw }}</pre>
	</template>
</div>
</template>

<script lang="ts" setup>
import { computed } from 'vue';
import { i18n } from '@/i18n.js';
import MkCode from '@/components/MkCode.vue';
import MkFolder from '@/components/MkFolder.vue';
import MkKeyValue from '@/components/MkKeyValue.vue';

const props = defineProps<{ logs: string[] }>();

function isObject(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseLog(raw: string) {
	try {
		const record: unknown = JSON.parse(raw);
		if (!isObject(record) || record.eventName !== 'queue.job.failed' || typeof record.timestamp !== 'string'
			|| !Number.isFinite(Date.parse(record.timestamp)) || !isObject(record.error)
			|| typeof record.error.type !== 'string' || typeof record.error.message !== 'string') return null;
		// causeでラップされたJSON-LDエラーも、具体的な診断を要約に表示します。
		let diagnostic = record.error;
		for (let depth = 0; depth < 10; depth++) {
			if (isObject(diagnostic.details) && isObject(diagnostic.details.event)) break;
			if (!isObject(diagnostic.cause)) break;
			diagnostic = diagnostic.cause;
		}
		const event = isObject(diagnostic.details) && isObject(diagnostic.details.event) ? diagnostic.details.event : null;
		const code = event?.code ?? diagnostic.code;
		return {
			timestamp: record.timestamp,
			error: record.error,
			code: typeof code === 'string' || typeof code === 'number' ? code : null,
			reason: typeof event?.message === 'string' ? event.message : null,
			property: isObject(event?.details) && typeof event.details.property === 'string' ? event.details.property : null,
		};
	} catch {
		return null;
	}
}

const entries = computed(() => props.logs.map(raw => ({ raw, record: parseLog(raw) })));
</script>

<style lang="scss" module>
.legacy {
	margin: 0;
	white-space: pre-wrap;
	overflow-wrap: anywhere;
}
</style>
