/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { mkdtemp, writeFile, rm, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import * as tls from 'node:tls';
import * as net from 'node:net';
import * as dns from 'node:dns';
import pg from 'pg';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { resolveRustPostgresConfig } from './postgres-config.js';

const database = (extra?: Record<string, unknown>) => ({
	db: { host: 'db.test', port: 5432, db: 'dedicated-test', user: 'test-user', pass: 'test-only-password', extra },
	dbReplications: false,
	dbSlaves: undefined,
});

beforeEach(() => {
	vi.stubEnv('PGSSLMODE', 'disable');
	vi.stubEnv('PGSSLNEGOTIATION', 'postgres');
	vi.stubEnv('PGREPLICATION', undefined);
	vi.stubEnv('PGOPTIONS', undefined);
	vi.stubEnv('PGAPPNAME', undefined);
	vi.stubEnv('PGCLIENT_ENCODING', undefined);
	vi.stubEnv('NODE_TLS_REJECT_UNAUTHORIZED', undefined);
});
afterEach(() => { vi.unstubAllEnvs(); vi.restoreAllMocks(); });

test('normalization does not open a DB connection and preserves defaults', async () => {
	const connect = vi.spyOn(pg.Client.prototype, 'connect').mockImplementation(() => { throw new Error('Must not connect'); });
	expect(await resolveRustPostgresConfig(database())).toEqual({ targets: [{
		autoSelectFamily: net.getDefaultAutoSelectFamily(), autoSelectFamilyAttemptTimeoutMs: net.getDefaultAutoSelectFamilyAttemptTimeout(), dnsResultOrder: dns.getDefaultResultOrder(),
		host: 'db.test', port: 5432, database: 'dedicated-test', user: 'test-user', password: 'test-only-password',
		options: null, applicationName: null, statementTimeoutMs: 10_000, lockTimeoutMs: null,
		idleInTransactionSessionTimeoutMs: null, connectTimeoutMs: null, queryTimeoutMs: null,
		keepAlive: false, keepAliveInitialDelayMs: null, channelBinding: false, sslNegotiation: 'postgres', tls: null,
	}] });
	expect(connect).not.toHaveBeenCalled();
});

test('extra overrides base credentials and preserves startup and client deadlines', async () => {
	const result = await resolveRustPostgresConfig(database({
		host: '/run/test-postgres', port: '6543', user: 'override-user', password: 'override-test-only-password', database: 'override-db',
		application_name: 'test-app', options: '-c search_path=fixture,public', statement_timeout: '1234',
		lock_timeout: 2345, idle_in_transaction_session_timeout: '3456', connectionTimeoutMillis: 4567, query_timeout: '5678',
		keepAlive: true, keepAliveInitialDelayMillis: 6789, enableChannelBinding: true,
	}));
	expect(result?.targets[0]).toMatchObject({
		host: '/run/test-postgres', port: 6543, user: 'override-user', password: 'override-test-only-password', database: 'override-db',
		applicationName: 'test-app', options: '-c search_path=fixture,public', statementTimeoutMs: 1234,
		lockTimeoutMs: 2345, idleInTransactionSessionTimeoutMs: 3456, connectTimeoutMs: 4567, queryTimeoutMs: 5678,
		keepAlive: true, keepAliveInitialDelayMs: 6789, channelBinding: true,
	});
});

test('pg URL and environment precedence matches the existing client', async () => {
	vi.stubEnv('PGSSLMODE', 'require');
	vi.stubEnv('PGAPPNAME', 'environment-app');
	const result = await resolveRustPostgresConfig(database({
		connectionString: 'postgres://url-user:url-test-only-password@url.test:6432/url-db?sslmode=disable',
	}));
	expect(result?.targets[0]).toMatchObject({ host: 'url.test', port: 6432, user: 'url-user', password: 'url-test-only-password', database: 'url-db', applicationName: 'environment-app', tls: null });
	const envResult = await resolveRustPostgresConfig(database());
	expect(envResult?.targets[0].tls).toMatchObject({ rejectUnauthorized: true, servername: 'db.test', sni: true });
	expect(envResult?.targets[0].tls?.ca).toEqual(tls.getCACertificates('default'));
});

test('replication uses all configured read targets and extra applies to each', async () => {
	const config = {
		...database({ port: 6543 }), dbReplications: true,
		dbSlaves: [
			{ host: 'reader-1.test', port: 5432, db: 'reader-1', user: 'reader', pass: 'test-only-reader' },
			{ host: 'reader-2.test', port: 5434, db: 'reader-2', user: 'reader', pass: 'test-only-reader' },
		],
	};
	const result = await resolveRustPostgresConfig(config);
	expect(result?.targets.map(target => [target.host, target.port, target.database])).toEqual([
		['reader-1.test', 6543, 'reader-1'], ['reader-2.test', 6543, 'reader-2'],
	]);
	expect((await resolveRustPostgresConfig({ ...config, dbSlaves: [] }))?.targets[0].host).toBe('db.test');
	expect((await resolveRustPostgresConfig({ ...config, dbReplications: false }))?.targets[0].host).toBe('db.test');
});

test('TLS retains trust, client key, passphrase, verification and version settings', async () => {
	const ssl = {
		ca: [Buffer.from('test-ca-1'), 'test-ca-2'], cert: Buffer.from('test-cert'), key: Buffer.from('test-key'),
		passphrase: 'test-key-passphrase', rejectUnauthorized: false, minVersion: 'TLSv1.2', maxVersion: 'TLSv1.3',
		ciphers: 'ECDHE-RSA-AES128-GCM-SHA256', servername: 'ignored-for-dns.test',
	};
	Object.defineProperty(ssl, 'key', { enumerable: false });
	const result = await resolveRustPostgresConfig(database({ ssl }));
	expect(result?.targets[0].tls).toEqual({
		ca: ['test-ca-1', 'test-ca-2'], cert: 'test-cert', key: 'test-key', passphrase: 'test-key-passphrase',
		rejectUnauthorized: false, minVersion: 'TLSv1.2', maxVersion: 'TLSv1.3', ciphers: 'ECDHE-RSA-AES128-GCM-SHA256',
		servername: 'db.test', sni: true,
	});
	const ipResult = await resolveRustPostgresConfig(database({ host: '127.0.0.1', ssl: true }));
	expect(ipResult?.targets[0].tls).toMatchObject({ servername: 'localhost', sni: false, rejectUnauthorized: true });
	const sniResult = await resolveRustPostgresConfig(database({ host: '127.0.0.1', ssl: { servername: 'identity.test' } }));
	expect(sniResult?.targets[0].tls).toMatchObject({ servername: 'identity.test', sni: true });
});

test('TLS true and no-verify use pg semantics without plaintext fallback', async () => {
	for (const mode of ['prefer', 'require', 'verify-ca', 'verify-full']) {
		vi.stubEnv('PGSSLMODE', mode);
		const result = await resolveRustPostgresConfig(database());
		expect(result?.targets[0].tls?.rejectUnauthorized).toBe(true);
	}
	vi.stubEnv('PGSSLMODE', 'no-verify');
	expect((await resolveRustPostgresConfig(database()))?.targets[0].tls?.rejectUnauthorized).toBe(false);
	vi.stubEnv('PGSSLNEGOTIATION', 'direct');
	expect((await resolveRustPostgresConfig(database()))?.targets[0].sslNegotiation).toBe('direct');
});

test('TLS environment verification default is preserved and explicit true still wins', async () => {
	vi.stubEnv('NODE_TLS_REJECT_UNAUTHORIZED', '0');
	expect((await resolveRustPostgresConfig(database({ ssl: true })))?.targets[0].tls?.rejectUnauthorized).toBe(false);
	expect((await resolveRustPostgresConfig(database({ ssl: { rejectUnauthorized: true } })))?.targets[0].tls?.rejectUnauthorized).toBe(true);
});

test('pgpass lookup uses the existing file matching and permission rules without connecting', async () => {
	const directory = await mkdtemp(join(tmpdir(), 'misskey-rust-pgpass-'));
	const path = join(directory, 'pgpass');
	try {
		await writeFile(path, 'db.test:5432:dedicated-test:test-user:fixture-file-password\n', { mode: 0o600 });
		vi.stubEnv('PGPASSFILE', path);
		vi.stubEnv('PGPASSWORD', undefined);
		const result = await resolveRustPostgresConfig(database({ password: '' }));
		expect(result?.targets[0].password).toBe('fixture-file-password');
		await chmod(path, 0o644);
		const warnings = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
		expect((await resolveRustPostgresConfig(database({ password: '' })))?.targets[0].password).toBeNull();
		warnings.mockRestore();
	} finally { await rm(directory, { recursive: true, force: true }); }
});

test('unsupported or malformed effective configuration fails closed', async () => {
	for (const extra of [
		{ ssl: { checkServerIdentity: () => undefined } }, { ssl: { secureContext: {} } },
		{ password: () => 'must-not-evaluate' }, { stream: () => undefined }, { types: {} },
		{ port: -1 }, { query_timeout: -1 }, { client_encoding: 'latin1' }, { ssl: { ca: {} } },
		{ host: 'bad\ud800host' }, { password: 'bad\0password' },
	]) expect(await resolveRustPostgresConfig(database(extra))).toBeNull();
	expect(await resolveRustPostgresConfig()).toBeNull();
});

test('replication protocol settings from connectionString or PGREPLICATION fail closed', async () => {
	expect(await resolveRustPostgresConfig(database({ connectionString: 'postgres://fixture:fixture@db.test/fixture?replication=database' }))).toBeNull();
	vi.stubEnv('PGREPLICATION', 'database');
	expect(await resolveRustPostgresConfig(database())).toBeNull();
});

test('captures effective runtime network defaults without resolving or connecting', async () => {
	const previous = { family: net.getDefaultAutoSelectFamily(), timeout: net.getDefaultAutoSelectFamilyAttemptTimeout(), order: dns.getDefaultResultOrder() };
	const connect = vi.spyOn(pg.Client.prototype, 'connect').mockImplementation(() => { throw new Error('Must not connect'); });
	try {
		net.setDefaultAutoSelectFamily(false);
		net.setDefaultAutoSelectFamilyAttemptTimeout(75);
		dns.setDefaultResultOrder('ipv4first');
		expect((await resolveRustPostgresConfig(database()))?.targets[0]).toMatchObject({ autoSelectFamily: false, autoSelectFamilyAttemptTimeoutMs: 75, dnsResultOrder: 'ipv4first' });
		net.setDefaultAutoSelectFamily(true);
		dns.setDefaultResultOrder('verbatim');
		expect((await resolveRustPostgresConfig(database()))?.targets[0]).toMatchObject({ autoSelectFamily: true, autoSelectFamilyAttemptTimeoutMs: 75, dnsResultOrder: 'verbatim' });
		expect(connect).not.toHaveBeenCalled();
	} finally {
		net.setDefaultAutoSelectFamily(previous.family);
		net.setDefaultAutoSelectFamilyAttemptTimeout(previous.timeout);
		dns.setDefaultResultOrder(previous.order);
	}
});
