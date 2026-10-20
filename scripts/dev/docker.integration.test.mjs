/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { expect, test } from 'bun:test';
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runDevDocker } from './docker.mjs';

// 実Dockerを使う検証。専用の一時checkoutとランダムな公開ポートを使い、native install/buildだけ差し替えます。
test.skipIf(process.env.MISSKEY_DEV_DOCKER_INTEGRATION !== '1')('real Docker preserves foreign resources and recreates empty PostgreSQL/Redis', async () => {
	const root = await realpath(await mkdtemp(join(tmpdir(), 'misskey-dev-docker-integration-')));
	const env = { ...process.env };
	delete env.NODE_ENV;
	const nativeCommands = [];
	const execute = async (args) => {
		if (args[0] !== 'docker') {
			nativeCommands.push(args.slice(1));
			return '';
		}
		const child = Bun.spawn(args, { cwd: root, env, stdout: 'pipe', stderr: 'pipe' });
		const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
		if (code !== 0) throw new Error(`Docker command failed: ${stderr}`);
		return stdout.trim();
	};
	// Compose固有の所有者/env値はrunDevDockerから受け取り、それ以外のDockerコマンドも実行します。
	const options = {
		root, env, confirm: async () => true, log: () => {},
		execute: async (args, settings) => {
			if (args[1] !== 'compose') return execute(args);
			const child = Bun.spawn(args, { cwd: root, env: settings.env, stdout: 'pipe', stderr: 'pipe' });
			const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
			if (code !== 0) throw new Error(`Compose command failed: ${stderr}`);
			return stdout.trim();
		},
	};
	let state;
	let foreignVolume;
	try {
		await mkdir(join(root, '.config'));
		await mkdir(join(root, 'scripts/dev'), { recursive: true });
		await writeFile(join(root, '.config/default.yml'), 'production sentinel');
		const manifest = await readFile(join(import.meta.dir, 'compose.yml'), 'utf8');
		await writeFile(join(root, 'scripts/dev/compose.yml'), manifest.replace('127.0.0.1:15432:', '127.0.0.1:0:').replace('127.0.0.1:16379:', '127.0.0.1:0:'));
		await runDevDocker('setup', options);
		state = JSON.parse(await readFile(join(root, '.config/dev-docker/state.json'), 'utf8'));
		const psql = (query) => execute(['docker', 'exec', `${state.project}-db-1`, 'psql', '-U', 'misskey_dev', '-d', 'misskey_dev', '-Atc', query]);
		const redis = (...args) => execute(['docker', 'exec', `${state.project}-redis-1`, 'redis-cli', ...args]);
		await psql("CREATE TABLE dev_docker_probe (value text); INSERT INTO dev_docker_probe VALUES ('keep until destroy');");
		await redis('SET', 'dev_docker_probe', 'keep until destroy');

		foreignVolume = `${state.project}_foreign`;
		await execute(['docker', 'volume', 'create', '--label', 'org.misskey.environment=production', foreignVolume]);
		await expect(runDevDocker('destroy', options)).rejects.toThrow('ownership');
		expect(await psql('SELECT value FROM dev_docker_probe')).toBe('keep until destroy');
		expect(await redis('GET', 'dev_docker_probe')).toBe('keep until destroy');
		expect(await execute(['docker', 'volume', 'ls', '--quiet', '--filter', `name=^${foreignVolume}$`])).toBe(foreignVolume);
		await execute(['docker', 'volume', 'rm', foreignVolume]);
		foreignVolume = undefined;

		await runDevDocker('destroy', options);
		expect(await execute(['docker', 'container', 'ls', '--all', '--quiet', '--filter', `label=com.docker.compose.project=${state.project}`])).toBe('');
		expect(await execute(['docker', 'volume', 'ls', '--quiet', '--filter', `name=${state.project}_`])).toBe('');
		await runDevDocker('setup', options);
		expect(await psql("SELECT to_regclass('public.dev_docker_probe') IS NULL")).toBe('t');
		expect(await redis('EXISTS', 'dev_docker_probe')).toBe('0');
		expect(await readFile(join(root, '.config/default.yml'), 'utf8')).toBe('production sentinel');
		expect(nativeCommands).toEqual([['install', '--frozen-lockfile'], ['run', 'build'], ['install', '--frozen-lockfile'], ['run', 'build']]);
	} finally {
		if (foreignVolume) await execute(['docker', 'volume', 'rm', foreignVolume]);
		if (!state) {
			const stored = await readFile(join(root, '.config/dev-docker/state.json'), 'utf8').catch(() => null);
			if (stored) state = JSON.parse(stored);
		}
		if (state) await runDevDocker('destroy', options);
		await rm(root, { recursive: true, force: true });
	}
}, 240000);
