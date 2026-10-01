/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { expect, test } from 'bun:test';
import { definition, timelineEligible } from '../../fixtures/benchmark/definition.js';
import { scenarios, select } from './registry.js';
import { phase, summary, appendProgress } from './runner.js';
import type { BenchmarkScenario, PhaseResult } from './types.js';
const fixture = definition('small').manifest;
const options = { requests: 12, concurrency: 3, warmup: 2, timeout: 30 };

test('default registry contains 22 scenarios and filters names', () => {
	expect(select([])).toHaveLength(22);
	expect(new Set(select([]).map(item => item.name)).size).toBe(22);
	expect(select(['timeline'])).toHaveLength(8);
	expect(select(['timeline/user', 'reaction'])).toHaveLength(5);
	expect(select(['unknown'])).toHaveLength(0);
});

test('fixture identities, data and relations are deterministic and do not overlap', () => {
	const a = definition('small');
	const b = definition('small');
	expect(a).toEqual(b);
	expect(a.manifest.users).toHaveLength(128);
	expect(a.notes).toHaveLength(8192);
	expect(new Set(a.notes.map(note => note.id)).size).toBe(8192);
	for (const user of a.manifest.users) {
		expect(user.follows).not.toContain(user.id);
		for (const excluded of [...user.mutes, ...user.blocks]) expect(user.follows).not.toContain(excluded);
	}
});

test('closed workers consume the complete body and respect global count and concurrency', async () => {
	let active = 0;
	let maximum = 0;
	const indices = new Set<number>();
	const scenario: BenchmarkScenario = {
		name: 'test/body',
		async request(context) {
			indices.add(context.requestIndex);
			active++;
			maximum = Math.max(maximum, active);
			return new Response(new ReadableStream({
				async start(controller) {
					await Bun.sleep(15);
					controller.enqueue(new TextEncoder().encode('{"ok":true}'));
					controller.close();
					active--;
				},
			}));
		},
		validate(body) { expect(body).toEqual({ ok: true }); },
	};
	const warmup = await phase(scenario, fixture, '', options, 2, 'warmup', new AbortController().signal);
	expect(warmup.completed).toBe(2);
	indices.clear();
	const result = await phase(scenario, fixture, '', options, 12, 'measure', new AbortController().signal);
	expect(indices.size).toBe(12);
	expect(result.completed).toBe(12);
	expect(maximum).toBeLessThanOrEqual(3);
	expect(result.samples.success.every(value => value >= 10)).toBe(true);
});

test('classifies HTTP errors, malformed responses, transport errors and timeouts without retry', async () => {
	let calls = 0;
	const scenario: BenchmarkScenario = {
		name: 'test/errors',
		async request(context) {
			calls++;
			if (context.requestIndex === 0) return Response.json({}, { status: 500 });
			if (context.requestIndex === 1) return new Response('invalid JSON');
			if (context.requestIndex === 2) throw new Error('connection lost');
			if (context.requestIndex === 3) {
				await new Promise((_, reject) => context.signal.addEventListener('abort', () => reject(context.signal.reason), { once: true }));
			}
			return Response.json({});
		},
		validate() {},
	};
	const result = await phase(scenario, fixture, '', options, 5, 'measure', new AbortController().signal);
	expect(calls).toBe(5);
	for (const samples of Object.values(result.samples)) expect(samples).toHaveLength(1);
	expect(result.httpStatuses).toEqual({ '500': 1 });
});

test('nearest-rank percentiles and throughput use phase wall time', () => {
	const result: PhaseResult = { requested: 4, completed: 4, elapsedMs: 2000, partial: false, httpStatuses: {},
		samples: { success: [40, 10, 30], httpError: [20], timeout: [], transportError: [], validationFailure: [] } };
	const output = summary(result);
	expect(output.throughput).toBe(1.5);
	expect(output.completedThroughput).toBe(2);
	expect(output.latencyMs).toEqual({ min: 10, p50: 30, p90: 40, p95: 40, p99: 40, max: 40 });
});

test('cancelled phases stop issuing requests and preserve completed samples', async () => {
	const abort = new AbortController();
	const scenario: BenchmarkScenario = {
		name: 'test/cancel',
		async request() { abort.abort(); return Response.json({}); }, validate() {},
	};
	const result = await phase(scenario, fixture, '', { ...options, concurrency: 1 }, 12, 'measure', abort.signal);
	expect(result.completed).toBe(1);
	expect(result.partial).toBe(true);
	expect(result.samples.success).toHaveLength(1);
});

test('controller CLI lists without Docker and rejects bad arguments', async () => {
	const run = async (...args: string[]) => {
		const child = Bun.spawn([process.execPath, 'scripts/benchmark.ts', ...args], { stdout: 'pipe', stderr: 'pipe' });
		const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
		return { stdout, stderr, code };
	};
	expect((await run('--list')).stdout.trim().split('\n')).toHaveLength(22);
	expect((await run('timeline', '--list')).stdout.trim().split('\n')).toHaveLength(8);
	for (const args of [['missing', '--list'], ['--requests', '0'], ['--warmup', '-1'], ['--concurrency', '1.5'], ['--fixture', 'production']]) {
		expect((await run(...args)).code).toBe(1);
	}
});

test('real HTTP body delays, connection termination and timeout are classified', async () => {
	const server = Bun.serve({ port: 0, fetch(request, server) {
		const path = new URL(request.url).pathname;
		if (path === '/disconnect') { server.stop(true); return new Response('closed'); }
		if (path === '/http') return new Response('error', { status: 503 });
		if (path === '/invalid') return Response.json({ ok: false });
		return new Response(new ReadableStream({ async start(controller) {
			controller.enqueue(new TextEncoder().encode('{"ok":'));
			await Bun.sleep(path === '/timeout' ? 100 : 20);
			try { controller.enqueue(new TextEncoder().encode('true}')); controller.close(); } catch { /* Client timed out. */ }
		} }));
	} });
	try {
		const paths = ['/body', '/http', '/invalid', '/timeout', '/disconnect'];
		const scenario: BenchmarkScenario = {
			name: 'test/http', request: context => fetch(new URL(paths[context.requestIndex], server.url), { signal: context.signal }),
			validate(body) { if (!(body as { ok: boolean }).ok) throw new Error('invalid response'); },
		};
		const result = await phase(scenario, fixture, '', { ...options, concurrency: 1, timeout: 50 }, 5, 'measure', new AbortController().signal);
		expect(result.completed).toBe(5);
		for (const samples of Object.values(result.samples)) expect(samples).toHaveLength(1);
		expect(result.samples.success[0]).toBeGreaterThanOrEqual(15);
	} finally { server.stop(true); }
});

test('separate generator transfers all samples and waits for controller acknowledgement', async () => {
	const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
	const { tmpdir } = await import('node:os');
	const { join } = await import('node:path');
	const directory = await mkdtemp(join(tmpdir(), 'misskey-benchmark-worker-'));
	const manifestPath = join(directory, 'manifest.json');
	await writeFile(manifestPath, JSON.stringify(fixture), { mode: 0o600 });
	const server = Bun.serve({ port: 0, fetch() { return Response.json([{ id: fixture.reactionNotes[0], visibility: 'public' }]); } });
	let child: ReturnType<typeof Bun.spawn> | undefined;
	let result: any;
	let warmup: any;
	try {
		child = Bun.spawn([process.execPath, 'scripts/benchmark-worker.ts', 'timeline/global', manifestPath,
			JSON.stringify({ requests: 1000, concurrency: 8, warmup: 100, timeout: 1000 }), server.url.toString()], {
			stdout: 'ignore', stderr: 'pipe', ipc(message: any) {
				if (message.type === 'warmup') { warmup = message.result; child!.send({ type: 'measure' }); }
				if (message.type === 'result') { result = message.result; child!.send({ type: 'ack' }); }
			},
		});
		expect(await child.exited).toBe(0);
		expect(warmup.samples.success).toHaveLength(100);
		expect(result.measure.samples.success).toHaveLength(1000);
		expect(result.measure.completed).toBe(1000);
		expect(result.cpu.user).toBeGreaterThan(0);
	} finally {
		child?.kill();
		server.stop(true);
		await rm(directory, { recursive: true });
	}
}, 10000);

test('port conflict preserves the unrelated server and saves an incomplete report', async () => {
	const { mkdtemp, readFile, rm } = await import('node:fs/promises');
	const { tmpdir } = await import('node:os');
	const { join } = await import('node:path');
	const directory = await mkdtemp(join(tmpdir(), 'misskey-benchmark-conflict-'));
	const server = Bun.serve({ port: 61912, hostname: '0.0.0.0', fetch() { return new Response('unrelated'); } });
	try {
		const output = join(directory, 'result.json');
		const child = Bun.spawn([process.execPath, 'scripts/benchmark.ts', '--json', output], { stdout: 'ignore', stderr: 'ignore' });
		expect(await child.exited).toBe(1);
		const result = JSON.parse(await readFile(output, 'utf8'));
		expect(result.status).toBe('fail');
		expect(result.failure).toContain('EADDRINUSE');
		expect(await (await fetch('http://127.0.0.1:61912')).text()).toBe('unrelated');
	} finally { server.stop(true); await rm(directory, { recursive: true }); }
});

test('SIGINT during the generator phase barrier returns a partial result and leaves server alive', async () => {
	const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
	const { tmpdir } = await import('node:os');
	const { join } = await import('node:path');
	const directory = await mkdtemp(join(tmpdir(), 'misskey-benchmark-interrupt-'));
	const manifestPath = join(directory, 'manifest.json');
	await writeFile(manifestPath, JSON.stringify(fixture), { mode: 0o600 });
	const server = Bun.serve({ port: 0, fetch() { return Response.json([{ id: fixture.reactionNotes[0], visibility: 'public' }]); } });
	let child: ReturnType<typeof Bun.spawn> | undefined;
	let result: any;
	try {
		child = Bun.spawn([process.execPath, 'scripts/benchmark-worker.ts', 'timeline/global', manifestPath,
			JSON.stringify({ requests: 1000, concurrency: 1, warmup: 1, timeout: 1000 }), server.url.toString()], {
			stdout: 'ignore', stderr: 'pipe', ipc(message: any) {
				if (message.type === 'warmup') child!.kill('SIGINT');
				if (message.type === 'result') { result = message.result; child!.send({ type: 'ack' }); }
			},
		});
		expect(await child.exited).toBe(0);
		expect(result.warmup.completed).toBe(1);
		expect(result.measure).toBeUndefined();
		expect((await fetch(server.url)).ok).toBe(true);
	} finally { child?.kill(); server.stop(true); await rm(directory, { recursive: true }); }
}, 10000);

test('health readiness requires both HTTP success and runtime readiness', async () => {
	const { waitForHealth } = await import('./lifecycle.js');
	const server = Bun.serve({ port: 0, fetch() { return new Response('healthy'); } });
	try {
		await expect(waitForHealth({ exitCode: null }, server.url.toString(), () => false, new AbortController().signal, 30)).rejects.toThrow('health timeout');
		await expect(waitForHealth({ exitCode: 1 }, server.url.toString(), () => true, new AbortController().signal, 30)).rejects.toThrow('server stopped');
		await waitForHealth({ exitCode: null }, server.url.toString(), () => true, new AbortController().signal, 30);
		expect((await fetch(server.url)).ok).toBe(true);
	} finally { server.stop(true); }
});

test('abnormal generator exit leaves a usable partial checkpoint', async () => {
	const { mkdtemp, writeFile, rm } = await import('node:fs/promises');
	const { tmpdir } = await import('node:os');
	const { join } = await import('node:path');
	const directory = await mkdtemp(join(tmpdir(), 'misskey-benchmark-crash-'));
	const manifestPath = join(directory, 'manifest.json');
	await writeFile(manifestPath, JSON.stringify(fixture), { mode: 0o600 });
	const server = Bun.serve({ port: 0, async fetch() {
		await Bun.sleep(2);
		return Response.json([{ id: fixture.reactionNotes[0], visibility: 'public' }]);
	} });
	let child: ReturnType<typeof Bun.spawn> | undefined;
	let checkpoint: PhaseResult | undefined;
	try {
		child = Bun.spawn([process.execPath, 'scripts/benchmark-worker.ts', 'timeline/global', manifestPath,
			JSON.stringify({ requests: 10000, concurrency: 4, warmup: 1, timeout: 1000 }), server.url.toString()], {
			stdout: 'ignore', stderr: 'pipe', ipc(message: any) {
				if (message.type === 'warmup') child!.send({ type: 'measure' });
				if (message.type === 'progress') { checkpoint = appendProgress(checkpoint, message.result); if (checkpoint.completed === 5) child!.kill('SIGKILL'); }
			},
		});
		expect(await child.exited).not.toBe(0);
		expect(checkpoint).toBeDefined();
		expect(checkpoint!.partial).toBe(true);
		expect(summary(checkpoint!).counts.success).toBe(checkpoint!.completed);
		expect(checkpoint!.completed).toBeGreaterThanOrEqual(5);
		expect(checkpoint!.completed).toBeLessThan(100);
		expect((await fetch(server.url)).ok).toBe(true);
	} finally { child?.kill(); server.stop(true); await rm(directory, { recursive: true }); }
}, 10000);

test('fixture v2 adds reproducible shapes, relations and mutation capacities', () => {
	const extra = fixture.extension;
	expect(fixture.version).toBe(2);
	expect(extra.richNotes.some(note => note.replyId)).toBe(true);
	expect(extra.richNotes.some(note => note.renoteId)).toBe(true);
	expect(extra.richNotes.some(note => note.fileIds?.length)).toBe(true);
	expect(extra.richNotes.some(note => note.hasPoll)).toBe(true);
	expect(extra.lists).toHaveLength(128);
	expect(extra.notificationReaders).toHaveLength(2048);
	expect(extra.heavyUsers).toHaveLength(32);
	for (const user of extra.heavyUsers) {
		expect(user.mutes.length / user.follows.length).toBe(0.875);
		for (const blocked of user.blocks) expect(user.follows).not.toContain(blocked);
	}
	for (const scenario of scenarios.filter(item => item.capacity)) expect(scenario.capacity!(fixture)).toBeGreaterThanOrEqual(1100);
	const pairs = extra.followTargets.map(pair => `${pair.userId}/${pair.targetId}`);
	expect(new Set(pairs).size).toBe(pairs.length);
	for (const pair of extra.followTargets) {
		const user = fixture.users.find(user => user.id === pair.userId)!;
		const target = fixture.users.find(user => user.id === pair.targetId)!;
		expect([...user.follows, ...user.mutes, ...user.blocks, user.id]).not.toContain(pair.targetId);
		expect(target.blocks).not.toContain(pair.userId);
	}
});

test('write scenarios allocate unique targets across warmup and concurrent measurement and accept empty HTTP bodies', async () => {
	const seen: string[] = [];
	const server = Bun.serve({ port: 0, async fetch(request) {
		const body = await request.json() as { i: string; noteId?: string; userId?: string };
		seen.push(`${body.i}/${body.noteId ?? body.userId ?? ''}`);
		return new URL(request.url).pathname.endsWith('following/create') ? Response.json({ id: body.userId, isFollowing: true }) : new Response(null, { status: 204 });
	} });
	try {
		for (const scenario of scenarios.filter(item => item.write && item.name !== 'note/create')) {
			seen.length = 0;
			const options = { requests: 10, concurrency: 3, warmup: 3, timeout: 1000 };
			const warmup = await phase(scenario, fixture, server.url.origin, options, 3, 'warmup', new AbortController().signal);
			const measure = await phase(scenario, fixture, server.url.origin, options, 10, 'measure', new AbortController().signal);
			expect(warmup.samples.success).toHaveLength(3);
			expect(measure.samples.success).toHaveLength(10);
			expect(new Set(seen).size).toBe(13);
		}
	} finally { server.stop(true); }
});

test('thread viewers rotate only through accounts that can see the parent', async () => {
	const parentAuthor = fixture.extension.plainNote.userId;
	const parentBlocks = fixture.users.find(user => user.id === parentAuthor)!.blocks;
	const viewers = new Set<string>();
	const server = Bun.serve({ port: 0, async fetch(request) {
		const { i } = await request.json() as { i: string };
		const user = fixture.users.find(user => user.token === i)!;
		expect([...user.mutes, ...user.blocks]).not.toContain(parentAuthor);
		expect(parentBlocks).not.toContain(user.id);
		viewers.add(user.id);
		return Response.json([{ id: fixture.extension.richNotes[0].id, replyId: fixture.extension.threadRoot }]);
	} });
	try {
		const scenario = scenarios.find(item => item.name === 'note/thread')!;
		const result = await phase(scenario, fixture, server.url.origin, { ...options, concurrency: 1, timeout: 1000 }, 128, 'measure', new AbortController().signal);
		expect(result.samples.success).toHaveLength(128);
		expect(viewers.size).toBeGreaterThan(100);
	} finally { server.stop(true); }
});

test('progress streams samples once and retains failures with linear transfer volume', async () => {
	let checkpoint: PhaseResult | undefined;
	let transferred = 0;
	const scenario: BenchmarkScenario = { name: 'mock', request: async () => Response.json({}), validate() {} };
	const result = await phase(scenario, fixture, '', { ...options, timeout: 1000 }, 10000, 'measure', new AbortController().signal, delta => {
		transferred += Object.values(delta.samples).reduce((sum, values) => sum + values.length, 0);
		checkpoint = appendProgress(checkpoint, delta);
	});
	expect(transferred).toBe(10000);
	expect(checkpoint!.samples).toEqual(result.samples);
	expect(checkpoint!.completed).toBe(10000);
});

test('timeline fixture and validation reject unsubscribed channels, remote authors and noneligible replies', () => {
	const user = fixture.users[0];
	const context = { manifest: fixture, url: '', workerId: 0, requestIndex: 0, phase: 'measure' as const, signal: new AbortController().signal };
	const local = scenarios.find(item => item.name === 'timeline/local')!;
	const channel = { id: fixture.extension.channelNotes[0], userId: user.id, visibility: 'public', channelId: fixture.extension.channelId };
	expect(timelineEligible(channel as any, 'local')).toBe(false);
	expect(() => local.validate([channel], context)).toThrow();
	expect(() => local.validate([{ id: fixture.reactionNotes[0], visibility: 'public', user: { host: 'remote.example' } }], context)).toThrow();
	const reply = fixture.extension.richNotes.find(note => note.replyId && note.replyUserId !== note.userId)!;
	expect(timelineEligible(reply, 'user')).toBe(false);
	expect(timelineEligible(reply, 'home', reply.replyUserId)).toBe(true);
	expect(timelineEligible(reply, 'home', 'unrelated')).toBe(false);
	expect(() => local.validate([reply], context)).toThrow();
});

test('write expectations and reaction capacities belong to scenarios; notifications reject reordered or missing pages', () => {
	for (const scenario of scenarios.filter(item => item.write)) expect(scenario.writeDelta).toBeDefined();
	const changed = { ...fixture, extension: { ...fixture.extension, reactionCreateTargets: fixture.extension.reactionCreateTargets.slice(0, 3) } };
	expect(scenarios.find(item => item.name === 'reaction/create')!.capacity!(changed)).toBe(3);
	expect(scenarios.find(item => item.name === 'reaction/delete')!.capacity!(changed)).toBe(fixture.extension.reactionDeleteTargets.length);
	const context = { manifest: fixture, url: '', workerId: 0, requestIndex: 0, phase: 'measure' as const, signal: new AbortController().signal };
	const notifications = scenarios.find(item => item.name === 'notification/list')!;
	const expected = fixture.extension.notifications.filter(item => item.userId === fixture.users[0].id).toReversed().slice(0, 20).map(item => ({ ...item, user: { id: item.notifierId }, note: { id: item.noteId } }));
	expect(() => notifications.validate(expected, context)).not.toThrow();
	expect(() => notifications.validate(expected.toReversed(), context)).toThrow();
	expect(() => notifications.validate(expected.slice(1), context)).toThrow();
});
