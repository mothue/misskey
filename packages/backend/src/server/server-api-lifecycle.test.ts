/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test, vi } from 'vitest';
import { ServerService } from './ServerService.js';

test('startup prepares the private callback before starting Rust or public HTTP', async () => {
	const events: string[] = [];
	const service = Object.create(ServerService.prototype) as ServerService;
	Object.assign(service, {
		apiServerService: { prepareRustApi: async () => { events.push('prepare'); } },
		rustBackendService: { start: async () => { events.push('rust'); } },
		launchHttp: async () => { events.push('http'); },
	});
	await service.launch();
	expect(events).toEqual(['prepare', 'rust', 'http']);
});

test('failed Rust startup also disposes already-prepared callbacks', async () => {
	const disposed = vi.fn(async () => {});
	const launchHttp = vi.fn(async () => {});
	const service = Object.create(ServerService.prototype) as ServerService;
	Object.assign(service, {
		apiServerService: { prepareRustApi: async () => {} },
		rustBackendService: { start: async () => { throw new Error('startup failed'); } },
		launchHttp,
	});
	Object.defineProperty(service, 'dispose', { value: disposed });
	await expect(service.launch()).rejects.toThrow('startup failed');
	expect(disposed).toHaveBeenCalledOnce();
	expect(launchHttp).not.toHaveBeenCalled();
});

test('shutdown still closes private callbacks when stopping Rust rejects', async () => {
	const stop = vi.fn(async () => {});
	const service = Object.create(ServerService.prototype) as ServerService;
	Object.assign(service, {
		rustBackendService: { stop: async () => { throw new Error('stop failed'); } },
		apiServerService: { stop },
	});
	await expect(service.dispose()).rejects.toThrow('stop failed');
	expect(stop).toHaveBeenCalledOnce();
});

test('shutdown closes public HTTP, then Rust, then the private callback server', async () => {
	const events: string[] = [];
	const service = Object.create(ServerService.prototype) as ServerService;
	Object.assign(service, {
		httpShutdown: { close: async (detach: () => void) => { events.push('http'); detach(); } },
		streamingApiServerService: { detach: () => { events.push('streaming'); } },
		rustBackendService: { stop: async () => { events.push('rust'); } },
		apiServerService: { stop: async () => { events.push('callback'); } },
	});
	await service.dispose();
	expect(events).toEqual(['http', 'streaming', 'rust', 'callback']);
});
