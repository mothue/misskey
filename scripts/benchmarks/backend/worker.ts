/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { scenarios } from './registry.js';
import { phase } from './runner.js';
import type { LoadOptions, WorkerResult } from './types.js';
import type { Manifest } from '../../../fixtures/benchmark/definition.js';
const [name, manifestPath, optionsText, baseUrl = 'http://127.0.0.1:61912'] = process.argv.slice(2);
const scenario = scenarios.find(item => item.name === name);
if (!scenario || !manifestPath || !optionsText || !process.send) throw new Error('worker requires controller IPC');
const manifest: Manifest = await Bun.file(manifestPath).json();
const options = JSON.parse(optionsText) as LoadOptions;
const stop = new AbortController();
process.on('SIGINT', () => stop.abort());
process.on('SIGTERM', () => stop.abort());
const send = (message: unknown) => process.send!(message);
const proceed = new Promise<void>(resolve => {
	stop.signal.addEventListener('abort', () => resolve(), { once: true });
	process.on('message', message => {
		if ((message as { type?: string }).type === 'measure') resolve();
		if ((message as { type?: string }).type === 'stop') { stop.abort(); resolve(); }
	});
});
// Bun IPC listeners alone do not keep an idle subprocess alive between phases.
const keepAlive = setInterval(() => {}, 1000);
const cpu = process.cpuUsage();
try {
	await scenario.setup?.(manifest);
	const warmup = await phase(scenario, manifest, baseUrl, options, options.warmup, 'warmup', stop.signal);
	send({ type: 'warmup', result: warmup });
	await proceed;
	const measure = stop.signal.aborted ? undefined : await phase(scenario, manifest, baseUrl, options,
		options.requests, 'measure', stop.signal, result => send({ type: 'progress', result }));
	const result: WorkerResult = { warmup, measure, cpu: process.cpuUsage(cpu) };
	const acknowledged = new Promise<void>(resolve => {
		process.on('message', message => { if ((message as { type?: string }).type === 'ack') resolve(); });
		// The controller acknowledges even partial results before releasing the worker.
	});
	send({ type: 'result', result });
	await acknowledged;
} catch {
	send({ type: 'fatal' });
	process.exitCode = 1;
} finally {
	clearInterval(keepAlive);
	await scenario.teardown?.();
	process.disconnect?.();
}
