/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { afterEach, expect, test } from 'bun:test';
import {
	mkdtemp,
	mkdir,
	readFile,
	readdir,
	realpath,
	rm,
	stat,
	symlink,
	writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runDevDocker } from './docker.mjs';

const directories = [];
afterEach(async () => {
	await Promise.all(
		directories
			.splice(0)
			.map((directory) => rm(directory, { recursive: true, force: true })),
	);
});

async function fixture() {
	const root = await realpath(
		await mkdtemp(join(tmpdir(), 'misskey-dev-docker-')),
	);
	directories.push(root);
	await mkdir(join(root, '.config'));
	await writeFile(
		join(root, '.config/default.yml'),
		'production config stays intact',
	);
	const calls = [];
	const inventory = { container: [], volume: [], network: [] };
	const execute = async (args, options) => {
		calls.push({ args, env: options.env });
		if (args[0] !== 'docker') return '';
		if (args[1] === 'info') return 'dev-daemon';
		if (args[1] === 'context') return 'unix:///local/docker.sock';
		if (args[1] === 'compose' && args.includes('up')) {
			ownResources({ inventory }, JSON.parse(await readFile(join(root, '.config/dev-docker/state.json'), 'utf8')));
		}
		if (args[2] === 'ls') return inventory[args[1]].map((item) => item.Id ?? item.Name).join('\n');
		if (args[2] === 'inspect') return JSON.stringify(
				inventory[args[1]].filter((item) =>
					args.slice(3).includes(item.Id ?? item.Name),
				),
			);
		return '';
	};
	const log = [];
	const options = {
		root,
		env: {},
		execute,
		confirm: async () => true,
		log: (message) => log.push(message),
	};
	return { root, calls, inventory, options, log };
}

async function state(f) {
	return JSON.parse(
		await readFile(join(f.root, '.config/dev-docker/state.json'), 'utf8'),
	);
}

function ownResources(f, s) {
	const labels = {
		'com.docker.compose.project': s.project,
		'org.misskey.dev-docker.owner': s.owner,
		'org.misskey.environment': 'development',
	};
	f.inventory.container = ['db', 'redis'].map((service) => ({
		Id: `dev-${service}-id`,
		Name: `/${s.project}-${service}-1`,
		Config: { Labels: { ...labels, 'com.docker.compose.service': service } },
		State: { Running: true },
		HostConfig: { PortBindings: {
			[service === 'db' ? '5432/tcp' : '6379/tcp']: [{ HostIp: '127.0.0.1', HostPort: service === 'db' ? '15432' : '16379' }],
		} },
		Mounts: [
			{
				Type: 'volume',
				Name: `${s.project}_${service === 'db' ? 'postgres' : 'redis'}`,
			},
		],
	}));
	f.inventory.volume = ['postgres', 'redis'].map((name) => ({
		Name: `${s.project}_${name}`,
		Labels: labels,
	}));
	f.inventory.network = [
		{ Id: 'dev-network-id', Name: `${s.project}_default`, Labels: labels },
	];
}

test.each(['setup', 'destroy', 'clean-cache'])(
	'%s cancellation does not run Docker or write config',
	async (action) => {
		const f = await fixture();
		await runDevDocker(action, { ...f.options, confirm: async () => false });
		expect(f.calls).toEqual([]);
		expect(await readdir(join(f.root, '.config'))).toEqual(['default.yml']);
	},
);

test.each(['setup', 'destroy', 'clean-cache', 'migrate', 'start'])(
	'%s refuses production mode before doing work',
	async (action) => {
		const f = await fixture();
		await expect(
			runDevDocker(action, { ...f.options, env: { NODE_ENV: 'production' } }),
		).rejects.toThrow('production');
		expect(f.calls).toEqual([]);
	},
);

test('setup creates isolated settings, pulls healthy databases and builds natively without migrating', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	const s = await state(f);
	const config = await readFile(
		join(f.root, '.config/dev-docker/default.yml'),
		'utf8',
	);
	expect(config).toContain('host: 127.0.0.1');
	expect(config).toContain('port: 15432');
	expect(config).toContain('port: 16379');
	expect(await readFile(join(f.root, '.config/default.yml'), 'utf8')).toBe(
		'production config stays intact',
	);
	expect(
		(await stat(join(f.root, '.config/dev-docker/state.json'))).mode & 0o777,
	).toBe(0o600);
	expect(s.project).toMatch(/^misskey-dev-[a-f0-9]{12}$/);
	const compose = f.calls.filter((call) => call.args[1] === 'compose');
	expect(compose.some((call) => call.args.includes('pull'))).toBe(true);
	expect(
		compose.some(
			(call) => call.args.includes('up') && call.args.includes('--wait'),
		),
	).toBe(true);
	for (const call of compose) expect(call.args).toContain(s.project);
	expect(
		f.calls
			.filter((call) => call.args[0] !== 'docker')
			.map((call) => call.args.slice(1)),
	).toEqual([
		['install', '--frozen-lockfile'],
		['run', 'build'],
	]);
	expect(f.log.join('\n')).toContain('bun run dev-docker:migrate');
	expect(f.log.join('\n')).toContain('bun run dev-docker:start');
	expect(f.calls.find((call) => call.args.includes('build')).env.NODE_ENV).toBe('production');
});

test('setup preserves credentials across runs and backs up modified development config', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	const originalState = await state(f);
	await writeFile(
		join(f.root, '.config/dev-docker/default.yml'),
		'custom development config',
	);
	await runDevDocker('setup', f.options);
	expect(await state(f)).toEqual(originalState);
	const backupRoot = join(f.root, '.config/dev-docker/backups');
	const backups = await readdir(backupRoot);
	expect(backups.length).toBeGreaterThan(0);
	expect(
		await readFile(join(backupRoot, backups.at(-1), 'default.yml'), 'utf8'),
	).toBe('custom development config');
});

test('destroy without ownership state never reaches Docker', async () => {
	const f = await fixture();
	await expect(runDevDocker('destroy', f.options)).rejects.toThrow('state');
	expect(f.calls).toEqual([]);
});

test('destroy removes only inspected development resources and never local production directories', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	const s = await state(f);
	ownResources(f, s);
	for (const directory of ['db', 'redis']) {
		await mkdir(join(f.root, directory));
		await writeFile(join(f.root, directory, 'production-data'), 'keep');
	}
	f.calls.length = 0;
	await runDevDocker('destroy', f.options);
	const removals = f.calls
		.filter((call) => call.args.includes('rm'))
		.map((call) => call.args);
	expect(removals).toEqual([
		['docker', 'container', 'rm', '--force', 'dev-db-id', 'dev-redis-id'],
		['docker', 'network', 'rm', 'dev-network-id'],
		['docker', 'volume', 'rm', `${s.project}_postgres`, `${s.project}_redis`],
	]);
	for (const directory of ['db', 'redis']) expect(
			await readFile(join(f.root, directory, 'production-data'), 'utf8'),
		).toBe('keep');
});

test.each(['container', 'volume', 'network'])(
	'destroy refuses a mismatched %s label before deleting anything',
	async (type) => {
		const f = await fixture();
		await runDevDocker('setup', f.options);
		ownResources(f, await state(f));
		const item = f.inventory[type][0];
		const labels = type === 'container' ? item.Config.Labels : item.Labels;
		labels['org.misskey.environment'] = 'production';
		f.calls.length = 0;
		await expect(runDevDocker('destroy', f.options)).rejects.toThrow(
			'ownership',
		);
		expect(f.calls.some((call) => call.args.includes('rm'))).toBe(false);
	},
);

test('destroy refuses another Docker daemon, copied state and production bind mounts', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	const s = await state(f);
	await expect(
		runDevDocker('destroy', {
			...f.options,
			execute: async (args, settings) => args[1] === 'info' ? 'production-daemon' : f.options.execute(args, settings),
		}),
	).rejects.toThrow('daemon');
	ownResources(f, s);
	f.inventory.container[0].Mounts = [
		{ Type: 'bind', Source: '/production/db' },
	];
	f.calls.length = 0;
	await expect(runDevDocker('destroy', f.options)).rejects.toThrow('mount');
	expect(f.calls.some((call) => call.args.includes('rm'))).toBe(false);
	await writeFile(
		join(f.root, '.config/dev-docker/state.json'),
		JSON.stringify({ ...s, root: '/production/checkout' }),
	);
	f.calls.length = 0;
	await expect(runDevDocker('destroy', f.options)).rejects.toThrow('state');
	expect(f.calls).toEqual([]);
});

test('setup refuses a preexisting production volume before writing development settings', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	ownResources(f, await state(f));
	f.inventory.volume[0].Labels = { 'org.misskey.environment': 'production' };
	f.calls.length = 0;
	await expect(runDevDocker('setup', f.options)).rejects.toThrow('ownership');
	expect(
		f.calls.some(
			(call) => call.args.includes('pull') || call.args.includes('up'),
		),
	).toBe(false);
});

test('clean-cache prunes only build cache, with confirmation describing the shared scope', async () => {
	const f = await fixture();
	let prompt = '';
	await runDevDocker('clean-cache', {
		...f.options,
		confirm: async (message) => {
			prompt = message;
			return true;
		},
	});
	expect(prompt).toContain('他プロジェクト');
	expect(f.calls.at(-1).args).toEqual([
		'docker',
		'builder',
		'prune',
		'--all',
		'--force',
	]);
	expect(
		f.calls.some(
			(call) => call.args.includes('volume') || call.args.includes('image'),
		),
	).toBe(false);
});

test.each(['migrate', 'start'])(
	'%s runs natively with isolated config and storage',
	async (action) => {
		const f = await fixture();
		await runDevDocker('setup', f.options);
		f.calls.length = 0;
		await runDevDocker(action, {
			...f.options,
			env: {
				MISSKEY_CONFIG_JSON: '/production/config.json',
				MISSKEY_STORAGE_ROOT: '/production/storage',
			},
		});
		const call = f.calls.at(-1);
		expect(call.args.slice(1)).toEqual(['run', action]);
		expect(call.env.NODE_ENV).toBe('development');
		expect(call.env.MISSKEY_CONFIG_YML).toBe('dev-docker/default.yml');
		expect(call.env.MISSKEY_CONFIG_JSON).toBe(
			join(f.root, '.config/dev-docker/compiled.json'),
		);
		expect(call.env.MISSKEY_STORAGE_ROOT).toBeUndefined();
		expect(call.env.MISSKEY_FILES_DIR).toBe(
			join(f.root, '.config/dev-docker/storage/files'),
		);
	},
);

test.each(['ssh://production.example', 'tcp://remote.example:2376'])(
	'setup refuses remote Docker endpoint %s before provisioning',
	async (endpoint) => {
		const f = await fixture();
		await expect(runDevDocker('setup', {
			...f.options,
			execute: async (args, settings) => args[1] === 'context' ? endpoint : f.options.execute(args, settings),
		})).rejects.toThrow('local Docker');
		expect(f.calls.some((call) => call.args.includes('pull') || call.args.includes('up'))).toBe(false);
	},
);

test('setup rejects a remote DOCKER_HOST and respects explicit context precedence', async () => {
	const f = await fixture();
	await expect(runDevDocker('setup', { ...f.options, env: { DOCKER_HOST: 'ssh://production.example' } })).rejects.toThrow('local Docker');
	await runDevDocker('setup', { ...f.options, env: { DOCKER_HOST: 'ssh://ignored.example', DOCKER_CONTEXT: 'local-desktop' } });
	expect(f.calls.some((call) => call.args[1] === 'context' && call.args.includes('local-desktop'))).toBe(true);
});

test.each(['missing', 'stopped', 'wrong-port'])(
	'native migration refuses %s development containers before connecting',
	async (problem) => {
		const f = await fixture();
		await runDevDocker('setup', f.options);
		if (problem === 'missing') f.inventory.container = [];
		if (problem === 'stopped') f.inventory.container[0].State.Running = false;
		if (problem === 'wrong-port') f.inventory.container[0].HostConfig.PortBindings['5432/tcp'][0].HostPort = '5432';
		f.calls.length = 0;
		await expect(runDevDocker('migrate', f.options)).rejects.toThrow('running development');
		expect(f.calls.every((call) => call.args[0] === 'docker')).toBe(true);
	},
);

test('setup refuses a symlinked settings directory', async () => {
	const f = await fixture();
	const outside = await mkdtemp(join(tmpdir(), 'misskey-production-config-'));
	directories.push(outside);
	await symlink(outside, join(f.root, '.config/dev-docker'));
	await expect(runDevDocker('setup', f.options)).rejects.toThrow('symlink');
	expect(await readdir(outside)).toEqual([]);
});

test('build failure does not run migration or start automatically', async () => {
	const f = await fixture();
	await expect(
		runDevDocker('setup', {
			...f.options,
			execute: async (args, options) => {
				if (args.includes('build')) throw new Error('build failed');
				return f.options.execute(args, options);
			},
		}),
	).rejects.toThrow('build failed');
	expect(
		f.calls.some(
			(call) => call.args.includes('migrate') || call.args.includes('start'),
		),
	).toBe(false);
});

test('setup/destroy cannot overlap in one checkout', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	await mkdir(join(f.root, '.config/dev-docker/operation.lock'));
	f.calls.length = 0;
	await expect(runDevDocker('destroy', f.options)).rejects.toThrow(
		'operation.lock',
	);
	expect(f.calls.some((call) => call.args.includes('rm'))).toBe(false);
});

test('native start refuses symlinked storage and modified development settings', async () => {
	const f = await fixture();
	await runDevDocker('setup', f.options);
	await symlink(
		join(f.root, '.config'),
		join(f.root, '.config/dev-docker/storage'),
	);
	await expect(runDevDocker('start', f.options)).rejects.toThrow('symlink');
	await rm(join(f.root, '.config/dev-docker/storage'));
	await writeFile(
		join(f.root, '.config/dev-docker/default.yml'),
		'db: { host: production }',
	);
	await expect(runDevDocker('migrate', f.options)).rejects.toThrow('config');
});

test.each(['n\n', '\n', '', 'yes\n'])(
	'CLI cancels on non-y input %j without needing Docker',
	async (input) => {
		const child = Bun.spawn(
			[process.execPath, join(import.meta.dir, 'docker.mjs'), 'setup'],
			{
				stdin: new TextEncoder().encode(input),
				stdout: 'pipe',
				stderr: 'pipe',
			},
		);
		const [code, stdout, stderr] = await Promise.all([
			child.exited,
			new Response(child.stdout).text(),
			new Response(child.stderr).text(),
		]);
		expect(code).toBe(0);
		expect(stdout).toContain('[y/N]');
		expect(stdout).toContain('中止');
		expect(stderr).toBe('');
	},
);

test('config compiler respects the isolated JSON destination without changing the default compiled config', async () => {
	const f = await fixture();
	const scriptDirectory = join(f.root, 'packages/backend/scripts');
	await mkdir(scriptDirectory, { recursive: true });
	await mkdir(join(f.root, 'built'));
	await writeFile(
		join(f.root, 'built/.config.json'),
		'production compiled config',
	);
	await writeFile(
		join(f.root, '.config/dev.yml'),
		'url: http://localhost:3000\n',
	);
	const original = await readFile(
		join(import.meta.dir, '../../packages/backend/scripts/compile_config.js'),
		'utf8',
	);
	await writeFile(join(scriptDirectory, 'compile_config.js'), original);
	await symlink(
		join(import.meta.dir, '../../node_modules'),
		join(f.root, 'node_modules'),
	);
	const target = join(f.root, '.config/dev-docker/compiled.json');
	const child = Bun.spawn(
		[process.execPath, join(scriptDirectory, 'compile_config.js')],
		{
			env: {
				...process.env,
				MISSKEY_CONFIG_YML: 'dev.yml',
				MISSKEY_CONFIG_JSON: target,
			},
			stdout: 'pipe',
			stderr: 'pipe',
		},
	);
	const [code, stdout, stderr] = await Promise.all([
		child.exited,
		new Response(child.stdout).text(),
		new Response(child.stderr).text(),
	]);
	expect(stderr).toBe('');
	expect(code).toBe(0);
	expect(stdout).toContain('Configuration compiled');
	expect(await readFile(join(f.root, 'built/.config.json'), 'utf8')).toBe(
		'production compiled config',
	);
	expect(JSON.parse(await readFile(target, 'utf8')).url).toBe(
		'http://localhost:3000',
	);
});
