/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import assert from 'node:assert/strict';
import { cp, mkdir, open, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { createServer } from 'node:net';
import { cpus, release } from 'node:os';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { waitForHealth } from './lifecycle.js';
import { select } from './registry.js';
import { summary, outcomes, appendProgress } from './runner.js';
import type { LoadOptions, PhaseResult, WorkerResult } from './types.js';
import { notificationStreamId } from '../../../fixtures/benchmark/definition.js';
import type { FixtureSize, Manifest } from '../../../fixtures/benchmark/definition.js';

const root = resolve(import.meta.dirname, '../../..');
const work = resolve(root, '.benchmark');
const backend = resolve(root, 'packages/backend');
const compose = ['docker', 'compose', '-p', 'misskey-benchmark-v1', '-f', resolve(root, 'fixtures/benchmark/compose.yml')];
const { values, positionals } = parseArgs({ args: process.argv.slice(2), allowPositionals: true, options: {
	list: { type: 'boolean' }, help: { type: 'boolean' }, requests: { type: 'string', default: '1000' },
	concurrency: { type: 'string', default: '8' }, warmup: { type: 'string', default: '100' },
	timeout: { type: 'string', default: '10000' }, fixture: { type: 'string', default: 'small' }, json: { type: 'string' },
} });
if (values.help) {
	console.log('bun run bench:backend [NAME FILTER ...] [--list] [--requests 1000] [--concurrency 8]\n  [--warmup 100] [--timeout 10000] [--fixture small|standard] [--json results.json]');
	process.exit(0);
}
const options: LoadOptions = { requests: Number(values.requests), concurrency: Number(values.concurrency), warmup: Number(values.warmup), timeout: Number(values.timeout) };
for (const [key, value] of Object.entries(options)) assert(Number.isSafeInteger(value) && value >= 1, `${key} must be a positive integer`);
assert(values.fixture === 'small' || values.fixture === 'standard', 'fixture must be small or standard');
const scenarios = select(positionals);
assert(scenarios.length, 'no benchmark names match');
if (values.list) { scenarios.forEach(scenario => console.log(scenario.name)); process.exit(0); }
const children = new Set<ReturnType<typeof Bun.spawn>>();
const abort = new AbortController();
let interruptTimer: ReturnType<typeof setTimeout> | undefined;
const stopped = () => {
	if (abort.signal.aborted) return;
	abort.abort();
	for (const child of children) child.kill('SIGTERM');
	interruptTimer = setTimeout(() => { for (const child of children) child.kill('SIGKILL'); }, 10000);
};
process.on('SIGINT', stopped);
process.on('SIGTERM', stopped);
const sleep = (ms: number) => new Promise<void>(resolvePromise => setTimeout(resolvePromise, ms));

async function stop(child: ReturnType<typeof Bun.spawn>) {
	if (child.exitCode === null) {
		child.kill('SIGTERM');
		let timer: ReturnType<typeof setTimeout>;
		await Promise.race([child.exited, new Promise<void>(resolvePromise => {
			timer = setTimeout(() => { child.kill('SIGKILL'); resolvePromise(); }, 10000);
		})]);
		clearTimeout(timer!);
		await child.exited;
	}
	children.delete(child);
}

async function command(args: string[], settings: { cwd?: string; env?: Record<string, string>; input?: Uint8Array } = {}) {
	if (abort.signal.aborted) throw new Error('interrupted');
	const child = Bun.spawn(args, { cwd: settings.cwd ?? root, env: { ...process.env, ...settings.env },
		stdin: settings.input ?? 'ignore', stdout: 'pipe', stderr: 'pipe' });
	children.add(child);
	const [stdout, _stderr, code] = await Promise.all([new Response(child.stdout).arrayBuffer(), new Response(child.stderr).text(), child.exited]);
	children.delete(child);
	if (code !== 0) throw new Error(`command failed: ${args[0]} ${args[1]} (${code}); subprocess diagnostics are withheld to avoid leaking fixture credentials`);
	return Buffer.from(stdout);
}

const dockerExec = (service: string, args: string[], input?: Uint8Array) => command([...compose, 'exec', '-T', service, ...args], { input });

async function checkPort() {
	const server = createServer();
	await new Promise<void>((resolvePromise, reject) => { server.once('error', reject); server.listen(61912, '0.0.0.0', resolvePromise); });
	await new Promise<void>((resolvePromise, reject) => server.close(error => error ? reject(error) : resolvePromise()));
}

const requireBackend = createRequire(resolve(backend, 'package.json'));

async function redisClient() {
	const { default: Redis } = await import(requireBackend.resolve('ioredis'));
	return new Redis({ host: '127.0.0.1', port: 56412, maxRetriesPerRequest: 1 });
}

async function dbQuery(sql: string) {
	const { default: pg } = await import(requireBackend.resolve('pg'));
	const client = new pg.Client({ host: '127.0.0.1', port: 54412, database: 'misskey-benchmark', user: 'benchmark', password: 'benchmark-local-only', connectionTimeoutMillis: 10000 });
	try { await client.connect(); return (await client.query(sql)).rows; } finally { await client.end(); }
}

async function resetDb() {
	await dockerExec('postgres', ['psql', '-U', 'benchmark', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1', '-c', 'DROP DATABASE IF EXISTS "misskey-benchmark" WITH (FORCE)', '-c', 'CREATE DATABASE "misskey-benchmark"']);
}

const configPath = resolve(work, 'config.json');
const runtimeEnv = { NODE_ENV: 'benchmark', MISSKEY_CONFIG_JSON: configPath, MISSKEY_STORAGE_ROOT: resolve(work, 'storage'), MK_NO_DAEMONS: '1', MISSKEY_BENCH_MIGRATIONS: resolve(work, 'migration-repo/packages/backend/migration') };
const runtime = resolve(backend, 'built-benchmark/entry.js');

async function restore(manifest: Manifest, dump: string, redisSnapshot: { key: string; value: string; ttl: number }[]) {
	await rm(resolve(work, 'storage'), { recursive: true, force: true });
	await cp(resolve(work, 'storage-baseline'), resolve(work, 'storage'), { recursive: true });
	await resetDb();
	await dockerExec('postgres', ['pg_restore', '-U', 'benchmark', '-d', 'misskey-benchmark', '--exit-on-error'], new Uint8Array(await readFile(dump)));
	const redis = await redisClient();
	try {
		await redis.flushall();
		for (const item of redisSnapshot) await redis.restore(item.key, item.ttl > 0 ? item.ttl : 0, Buffer.from(item.value, 'base64'));
		const markerCheck = redis.pipeline();
		const expectedMarkers = manifest.users.map(user => {
			const notifications = manifest.extension.notifications.filter(item => item.userId === user.id);
			assert.equal(notifications.length, 32, 'fixture notification count');
			markerCheck.get(`benchmark:latestReadNotification:${user.id}`);
			return notificationStreamId(notifications[15].id);
		});
		const markers = await markerCheck.exec();
		assert(markers && markers.every(([error, value]: [Error | null, unknown], index: number) => !error && value === expectedMarkers[index]), 'restored notification read markers');
	} finally { await redis.quit(); }
	const counts = (await dbQuery('SELECT (SELECT count(*) FROM "user")::int AS users, (SELECT count(*) FROM note)::int AS notes, (SELECT count(*) FROM following)::int AS follows, (SELECT count(*) FROM muting)::int AS mutes, (SELECT count(*) FROM blocking)::int AS blocks, (SELECT count(*) FROM note_reaction)::int AS reactions'))[0];
	for (const key of ['users', 'notes', 'follows', 'mutes', 'blocks', 'reactions']) assert.equal(counts[key], manifest.stats[key], `restored ${key}`);
	return counts;
}

async function launchServer(logPath: string) {
	const messages: any[] = [];
	const log = await open(logPath, 'a', 0o600);
	const server = Bun.spawn([process.execPath, runtime, 'server'], { cwd: backend, env: { ...process.env, ...runtimeEnv },
		stdout: log.fd, stderr: log.fd, ipc(message) { messages.push(message); } });
	children.add(server);
	await log.close();
	try {
		await waitForHealth(server, 'http://127.0.0.1:61912/healthz/', () => messages.some(item => item.type === 'ready'), abort.signal);
		return {
			child: server,
			async queues() {
				server.send({ type: 'queues' });
				const start = Date.now();
				while (Date.now() - start < 10000) {
					const index = messages.findIndex(item => item.type === 'queues');
					if (index >= 0) return messages.splice(index, 1)[0].counts;
					if (server.exitCode !== null || abort.signal.aborted) throw new Error('server unavailable');
					await sleep(10);
				}
				throw new Error('queue snapshot timeout');
			},
		};
	} catch (error) { await stop(server); throw error; }
}

function backlog(queues: any[]) { return queues.reduce((total, queue) => total + (queue.counts.waiting ?? queue.counts.wait ?? 0) + (queue.counts.active ?? 0) + (queue.counts.prioritized ?? 0), 0); }

async function state() {
	const counts = (await dbQuery('SELECT (SELECT count(*) FROM note)::int AS notes, (SELECT count(*) FROM following)::int AS follows, (SELECT count(*) FROM note_reaction)::int AS reactions'))[0];
	const redis = await redisClient();
	try {
		counts.readMarkers = (await redis.keys('benchmark:latestReadNotification:*')).length;
		counts.reactionBuffers = (await redis.keys('benchmark:reactionsBufferDeltas:*')).length;
	} finally { await redis.quit(); }
	return counts;
}

const report: any = { schemaVersion: 1, startedAt: new Date().toISOString(), loadModel: 'closed', options,
	environment: { bun: Bun.version, os: process.platform, osRelease: release(), arch: process.arch, cpu: cpus()[0]?.model, serverWorkers: 1, generatorProcesses: 1,
		rateLimit: 'disabled (NODE_ENV=benchmark)', outbound: 'HTTP transport disabled, no remote users, localOnly posts, no mail/webhook fixtures', search: 'sqlLike', daemons: false, fanoutTimeline: true, fanoutDbFallback: true, reactionsBuffering: true }, results: [] };
await mkdir(work, { recursive: true, mode: 0o700 });
const reportPath = resolve(work, `results/${Date.now()}.json`);
await mkdir(resolve(work, 'results'), { recursive: true, mode: 0o700 });

async function save() {
	await writeFile(reportPath, JSON.stringify(report, null, 2), { mode: 0o600 });
	if (values.json) await writeFile(resolve(values.json), JSON.stringify(report, null, 2), { mode: 0o600 });
}

let lock;
let failed = false;
try {
	lock = await open(resolve(work, 'lock'), 'wx', 0o600);
	await checkPort();
	try { await command(['docker', 'info', '--format', '{{.ServerVersion}}']); } catch { throw new Error('Docker is unavailable. Start Docker Desktop, then run bun run bench:backend again.'); }
	console.log('Preparing dedicated benchmark environment...');
	await command([...compose, 'up', '-d', '--wait']);
	await command([process.execPath, 'run', 'build:metadata']);
	await command([process.execPath, 'run', '--filter', 'i18n', '--filter', 'misskey-reversi', '--filter', 'misskey-js', 'build']);
	await command([process.execPath, 'run', '--bun', 'rolldown', '-c', '--benchmark'], { cwd: backend });
	await rm(resolve(work, 'storage'), { recursive: true, force: true });
	await mkdir(resolve(work, 'storage'), { recursive: true });
	await writeFile(configPath, JSON.stringify({ url: 'http://127.0.0.1:61912', port: 61912, id: 'aidx',
		db: { host: '127.0.0.1', port: 54412, db: 'misskey-benchmark', user: 'benchmark', pass: 'benchmark-local-only' },
		redis: { host: '127.0.0.1', port: 56412, prefix: 'benchmark' }, proxy: 'http://127.0.0.1:1', fulltextSearch: { provider: 'sqlLike' }, threadPoolSize: 1, clusterLimit: 1,
		logging: { level: 'error' } }), { mode: 0o600 });
	// Keep migrations immutable; their relative config lookup resolves inside this isolated copy.
	await mkdir(resolve(work, 'migration-repo/built'), { recursive: true });
	await cp(resolve(backend, 'migration'), runtimeEnv.MISSKEY_BENCH_MIGRATIONS, { recursive: true });
	await cp(configPath, resolve(work, 'migration-repo/built/.config.json'));
	await rm(resolve(work, 'migration-repo/packages/backend/node_modules'), { force: true });
	await symlink(resolve(backend, 'node_modules'), resolve(work, 'migration-repo/packages/backend/node_modules'), 'dir');
	await resetDb();
	const redis = await redisClient();
	await redis.flushall();
	await redis.quit();
	const manifestPath = resolve(work, 'manifest.json');
	await command([process.execPath, runtime, 'seed', values.fixture!, manifestPath], { cwd: backend, env: runtimeEnv });
	const manifest: Manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
	for (const scenario of scenarios) if (scenario.capacity) assert(options.requests + options.warmup <= scenario.capacity(manifest), `${scenario.name}: warmup + requests exceed fixture capacity ${scenario.capacity(manifest)}`);
	await rm(resolve(work, 'storage-baseline'), { recursive: true, force: true });
	await cp(resolve(work, 'storage'), resolve(work, 'storage-baseline'), { recursive: true });
	const { extension: _extension, users: _users, reactionNotes: _notes, noteIds: _noteIds, ...fixtureMetadata } = manifest;
	report.fixture = { ...fixtureMetadata, capacity: { deleteNotes: manifest.extension.deleteNotes.length, followPairs: manifest.extension.followTargets.length, notificationReaders: manifest.extension.notificationReaders.length }, coreUserDistribution: { normalUsers: 0.75, heavyUsers: 0.25, remoteUsers: 0, attachments: manifest.stats.files } };
	report.environment.git = (await command(['git', 'rev-parse', 'HEAD'])).toString().trim();
	report.environment.dirty = Boolean((await command(['git', 'status', '--porcelain'])).toString().trim());
	report.environment.postgres = (await dbQuery('SELECT version() AS version'))[0].version;
	const redisForSnapshot = await redisClient();
	const redisSnapshot: { key: string; value: string; ttl: number }[] = [];
	try {
		report.environment.redis = (await redisForSnapshot.info('server')).match(/redis_version:([^\r\n]+)/)?.[1];
		const keys: string[] = (await redisForSnapshot.keys('*')).sort();
		for (let offset = 0; offset < keys.length; offset += 256) {
			const batch = keys.slice(offset, offset + 256);
			const pipeline = redisForSnapshot.pipeline();
			for (const key of batch) { pipeline.dumpBuffer(key); pipeline.pttl(key); }
			const results = await pipeline.exec();
			assert(results && results.every(([error]: [Error | null, unknown]) => !error), 'Redis snapshot failed');
			batch.forEach((key, index) => {
				const value = results[index * 2][1] as Buffer | null;
				if (value) redisSnapshot.push({ key, value: value.toString('base64'), ttl: Number(results[index * 2 + 1][1]) });
			});
		}
	} finally { await redisForSnapshot.quit(); }
	await writeFile(resolve(work, 'redis-snapshot.json'), JSON.stringify(redisSnapshot), { mode: 0o600 });
	const dump = resolve(work, 'fixture.dump');
	await writeFile(dump, await dockerExec('postgres', ['pg_dump', '-Fc', '-U', 'benchmark', 'misskey-benchmark']), { mode: 0o600 });
	let restoreFailed = false;
	for (const scenario of scenarios) {
		if (abort.signal.aborted || restoreFailed) { report.results.push({ scenario: scenario.name, status: 'skipped' }); failed = true; continue; }
		console.log(`\n${scenario.name} (closed)`);
		const entry: any = { scenario: scenario.name, shape: scenario.shape, status: 'incomplete' };
		report.results.push(entry);
		try { entry.restoredCounts = await restore(manifest, dump, redisSnapshot); } catch (error) {
			restoreFailed = true; failed = true; entry.failure = 'fixture restore failed'; console.error(error); await save(); continue;
		}
		let server: Awaited<ReturnType<typeof launchServer>> | undefined;
		let worker: ReturnType<typeof Bun.spawn> | undefined;
		try {
			server = await launchServer(resolve(work, `${scenario.name.replace('/', '-')}.log`));
			// Preflight verifies authentication and each response using the same validation as the load generator.
			entry.preflightRequests = scenario.write ? 0 : manifest.users.length;
			for (let index = 0; index < manifest.users.length; index++) {
				const context = { manifest, url: 'http://127.0.0.1:61912', workerId: 0, requestIndex: index, phase: 'warmup' as const, signal: AbortSignal.timeout(options.timeout) };
				if (scenario.write) break;
				const res = await scenario.request(context);
				const bytes = await res.arrayBuffer();
				assert(res.ok, `preflight HTTP ${res.status}`);
				scenario.validate(JSON.parse(new TextDecoder().decode(bytes)), context);
			}
			let workerResult: WorkerResult | undefined;
			let warmupTask: Promise<void> | undefined;
			let phaseFailure = false;
			worker = Bun.spawn([process.execPath, resolve(root, 'scripts/benchmarks/backend/worker.ts'), scenario.name, manifestPath, JSON.stringify(options)], {
				cwd: root, stdout: 'ignore', stderr: 'inherit', ipc(message: any) {
					if (message.type === 'progress') entry.partialMeasure = appendProgress(entry.partialMeasure, message.result);
					if (message.type === 'fatal') phaseFailure = true;
					if (message.type === 'result') { workerResult = message.result; worker!.send({ type: 'ack' }); }
					if (message.type === 'warmup') {
						entry.warmup = message.result;
						warmupTask = (async () => {
							assert(!message.result.partial && outcomes.filter(item => item !== 'success').every(item => !message.result.samples[item].length), 'warmup failed');
							if (scenario.write) {
								const start = Date.now();
								while (backlog(await server!.queues()) > 0) { assert(Date.now() - start < 60000, 'queue warmup drain timeout'); await sleep(100); }
								entry.queueStart = await server!.queues();
								entry.stateStart = await state();
								entry.notesStart = entry.stateStart.notes;
							}
							worker!.send({ type: 'measure' });
						})().catch(error => { entry.failure = error instanceof Error ? error.message : 'warmup failed'; phaseFailure = true; worker!.send({ type: 'stop' }); });
					}
				},
			});
			children.add(worker);
			const exit = await worker.exited;
			await warmupTask;
			children.delete(worker);
			entry.worker = workerResult;
			entry.generatorExitCode = exit;
			if (!workerResult?.measure) {
				entry.failure ??= phaseFailure ? 'warmup or generator failed' : 'generator exited without measurement result';
				if (entry.partialMeasure) entry.summary = summary(entry.partialMeasure);
			}
			if (workerResult?.measure) {
				entry.summary = summary(workerResult.measure);
				delete entry.partialMeasure;
				entry.status = exit === 0 && !phaseFailure && !entry.summary.partial && entry.summary.counts.success === options.requests ? 'pass' : 'fail';
				console.log(`warmup ${options.warmup} | requests ${options.requests} | concurrency ${options.concurrency}`);
				console.log(`time ${(entry.summary.elapsedMs / 1000).toFixed(2)} s | throughput ${entry.summary.throughput.toFixed(1)} req/s`);
				console.table(Object.fromEntries(Object.entries(entry.summary.latencyMs).map(([key, value]) => [key, typeof value === 'number' ? `${value.toFixed(2)} ms` : 'n/a'])));
				console.log('outcomes', entry.summary.counts);
			}
			if (scenario.write) {
				entry.queueEnd = await server.queues();
				entry.stateEnd = await state();
				entry.notesEnd = entry.stateEnd.notes;
				await sleep(5000);
				entry.queueAfter5s = await server.queues();
				entry.stateAfter5s = await state();
				if (entry.status === 'pass') {
					assert(scenario.writeDelta, 'write scenario must declare its expected state delta');
					const { field, direction } = scenario.writeDelta;
					assert.equal(entry.stateAfter5s[field] - entry.stateStart[field], options.requests * direction, 'write state increment mismatch');
				}
			}
		} catch (error) { entry.status = 'incomplete'; entry.failure = error instanceof Error ? error.message : 'scenario failed'; console.error(entry.failure); } finally {
			if (worker) await stop(worker);
			if (server) await stop(server.child);
		}
		if (entry.status !== 'pass') failed = true;
		await save();
	}
} catch (error) {
	failed = true;
	report.failure = error instanceof Error ? error.message : 'setup failed';
	console.error(report.failure);
} finally {
	for (const child of children) await stop(child);
	if (lock) { await lock.close(); await rm(resolve(work, 'lock'), { force: true }); }
	report.finishedAt = new Date().toISOString();
	report.status = abort.signal.aborted ? 'partial' : failed ? 'fail' : 'pass';
	await save();
	console.log(`\nResult: ${report.status}\nJSON: ${reportPath}`);
	clearTimeout(interruptTimer);
	process.removeListener('SIGINT', stopped);
	process.removeListener('SIGTERM', stopped);
}
if (failed || abort.signal.aborted) process.exitCode = 1;
