/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { afterEach, describe, expect, test, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/vue';
import type { entities } from 'misskey-js';
import QueueJobLogs from '@/pages/admin/job-queue.job.logs.vue';
import QueueJob from '@/pages/admin/job-queue.job.vue';
import MkTime from '@/components/global/MkTime.vue';
import { dateTimeFormat } from '@@/js/intl-const.js';
import * as os from '@/os.js';

vi.mock('@/os.js', async importOriginal => ({
	...await importOriginal<typeof import('@/os.js')>(),
	apiWithDialog: vi.fn(),
}));

const diagnostic = JSON.stringify({
	eventName: 'queue.job.failed',
	timestamp: '2026-10-20T00:00:00.000Z',
	error: {
		type: 'jsonld.ValidationError',
		message: 'Safe mode validation error.',
		details: { event: { code: 'invalid property', message: 'Dropping property.', details: { property: 'unmapped' } } },
	},
});

const global = { stubs: {
	MkTime: { template: '<time />' },
	MkKeyValue: { template: '<div><span><slot name="key"/></span><span><slot name="value"/></span></div>' },
	MkFolder: { template: '<details><summary><slot name="label"/></summary><slot/></details>' },
	MkCode: { props: ['code'], template: '<pre>{{ code }}</pre>' },
} };

afterEach(cleanup);

describe('queue job logs', () => {
	test('shows the JSON-LD reason and offending property with expandable diagnostic JSON', () => {
		const view = render(QueueJobLogs, { props: { logs: [diagnostic] }, global });
		expect(view.getByText('jsonld.ValidationError: Safe mode validation error.')).toBeTruthy();
		expect(view.getByText('invalid property')).toBeTruthy();
		expect(view.getByText('Dropping property.')).toBeTruthy();
		expect(view.getByText('unmapped')).toBeTruthy();
		expect(view.container.querySelector('details pre')?.textContent).toContain('"event"');
	});

	test('keeps legacy text, malformed JSON and unrelated JSON visible', () => {
		const lines = ['legacy log', '{invalid', '{"message":"other log"}', 'null', '[]'];
		const view = render(QueueJobLogs, { props: { logs: lines }, global });
		for (const line of lines) expect(view.getByText(line)).toBeTruthy();
	});

	test('displays a generic error without JSON-LD details and escapes remote text', () => {
		const view = render(QueueJobLogs, { props: { logs: [JSON.stringify({
			eventName: 'queue.job.failed', timestamp: '2026-10-20T00:00:00.000Z',
			error: { type: 'Error', message: '<img src=x onerror=alert(1)>', code: 'ECONNRESET' },
		})] }, global });
		expect(view.getByText('ECONNRESET')).toBeTruthy();
		expect(view.getByText('Error: <img src=x onerror=alert(1)>')).toBeTruthy();
		expect(view.container.querySelector('img')).toBeNull();
	});

	test('handles a nested cause and shows the empty-log state', async () => {
		const record = JSON.parse(diagnostic);
		record.error = { type: 'Error', message: 'outer', cause: record.error };
		const view = render(QueueJobLogs, { props: { logs: [JSON.stringify(record)] }, global });
		expect(view.getByText('invalid property')).toBeTruthy();
		await view.rerender({ logs: [] });
		expect(view.getByText('No saved logs.')).toBeTruthy();
		expect(view.container.querySelector('pre')).toBeNull();
	});

	test('refreshes timestamps when capped logs discard the oldest entry', async () => {
		const dates = ['2026-10-20T00:00:00.000Z', '2026-10-21T00:00:00.000Z', '2026-10-22T00:00:00.000Z'];
		const lines = dates.map(timestamp => JSON.stringify({ ...JSON.parse(diagnostic), timestamp }));
		const view = render(QueueJobLogs, {
			props: { logs: lines.slice(0, 2) },
			global: { components: { MkTime }, stubs: { ...global.stubs, MkTime: false } },
		});
		await view.rerender({ logs: lines.slice(1) });
		expect(Array.from(view.container.querySelectorAll('time'), el => el.title)).toEqual(
			dates.slice(1).map(date => dateTimeFormat.format(new Date(date))),
		);
	});

	test('loads Inbox diagnostics from the existing job-log API through the job UI', async () => {
		vi.mocked(os.apiWithDialog).mockResolvedValueOnce([diagnostic]);
		const view = render(QueueJob, {
			props: {
				queueType: 'inbox',
				job: {
					id: '42', name: 'inbox', data: {}, opts: {}, timestamp: 0,
					stacktrace: [], attempts: 1, isFailed: true,
					progress: {}, delay: 0, failedReason: 'Safe mode validation error.', returnValue: {},
				} satisfies entities.QueueJob,
			},
			global: { stubs: {
				...global.stubs,
				MkFolder: { template: '<div><slot name="header"/><slot/></div>' },
				MkTabs: { template: '<button @click="$emit(\'update:tab\', \'logs\')">Logs</button>' },
				MkButton: { template: '<button><slot/></button>' },
			} },
		});
		await fireEvent.click(view.getByRole('button', { name: 'Logs' }));
		await fireEvent.click(view.getByRole('button', { name: 'Load logs' }));
		expect(os.apiWithDialog).toHaveBeenCalledWith('admin/queue/show-job-logs', { queue: 'inbox', jobId: '42' });
		expect(await view.findByText('invalid property')).toBeTruthy();
		expect(view.getByText('unmapped')).toBeTruthy();
	});
});
