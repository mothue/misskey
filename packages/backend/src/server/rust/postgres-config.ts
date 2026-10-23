/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { isIP } from 'node:net';
import * as tls from 'node:tls';
import pg from 'pg';
import type { Config } from '@/config.js';
import { object } from './protocol.js';

export type RustPostgresTls = {
	rejectUnauthorized: boolean;
	ca: string[];
	cert: string | null;
	key: string | null;
	passphrase: string | null;
	servername: string;
	sni: boolean;
	minVersion: tls.SecureVersion;
	maxVersion: tls.SecureVersion;
	ciphers: string;
};

export type RustPostgresTarget = {
	host: string;
	port: number;
	user: string;
	password: string | null;
	database: string;
	options: string | null;
	applicationName: string | null;
	statementTimeoutMs: number | null;
	lockTimeoutMs: number | null;
	idleInTransactionSessionTimeoutMs: number | null;
	connectTimeoutMs: number | null;
	queryTimeoutMs: number | null;
	keepAlive: boolean;
	keepAliveInitialDelayMs: number | null;
	channelBinding: boolean;
	sslNegotiation: 'postgres' | 'direct';
	tls: RustPostgresTls | null;
};

export type RustPostgresConfig = { targets: RustPostgresTarget[] };
type DatabaseConfig = Pick<Config, 'dbReplications' | 'dbSlaves'> & {
	db: Omit<Config['db'], 'extra'> & { extra?: Record<string, unknown> };
};

const invalid = () => new Error('Unsupported Rust PostgreSQL configuration');
const tlsKeys = new Set(['rejectUnauthorized', 'ca', 'cert', 'key', 'passphrase', 'servername', 'minVersion', 'maxVersion', 'ciphers']);
const extraKeys = new Set([
	'host', 'port', 'user', 'password', 'database', 'connectionString', 'ssl', 'sslnegotiation',
	'options', 'application_name', 'fallback_application_name', 'statement_timeout', 'lock_timeout',
	'idle_in_transaction_session_timeout', 'query_timeout', 'connectionTimeoutMillis', 'client_encoding',
	'keepAlive', 'keepAliveInitialDelayMillis', 'enableChannelBinding',
	// These size/lifetime settings belong to the existing TS pool. The Rust adapter
	// owns at most one shared connection per read target, with its own bounded lifetime.
	'max', 'min', 'idleTimeoutMillis', 'allowExitOnIdle', 'maxUses', 'maxLifetimeSeconds',
]);

function text(value: unknown): string {
	if (typeof value !== 'string' || !value.isWellFormed() || value.includes('\0')) throw invalid();
	return value;
}

function optionalText(value: unknown): string | null {
	return value == null || value === '' ? null : text(value);
}

function milliseconds(value: unknown): number | null {
	if (value == null || value === false || value === 0 || value === '') return null;
	const parsed = typeof value === 'string' ? Number(value) : value;
	if (typeof parsed !== 'number' || !Number.isInteger(parsed) || parsed < 0 || parsed > 0xffffffff) throw invalid();
	return parsed === 0 ? null : parsed;
}

function pem(value: unknown): string {
	return text(Buffer.isBuffer(value) ? value.toString('utf8') : value);
}

function version(value: unknown): tls.SecureVersion {
	if (value !== 'TLSv1' && value !== 'TLSv1.1' && value !== 'TLSv1.2' && value !== 'TLSv1.3') throw invalid();
	return value;
}

function normalizeTls(value: unknown, host: string): RustPostgresTls | null {
	if (value === false || value == null) return null;
	const options = value === true ? {} : value;
	if (!object(options) || Object.getOwnPropertyNames(options).some(key => !tlsKeys.has(key))) throw invalid();
	if (options.rejectUnauthorized !== undefined && typeof options.rejectUnauthorized !== 'boolean') throw invalid();
	const ca = options.ca == null ? tls.getCACertificates('default') : (Array.isArray(options.ca) ? Array.from(options.ca, pem) : [pem(options.ca)]);
	return {
		rejectUnauthorized: options.rejectUnauthorized ?? process.env.NODE_TLS_REJECT_UNAUTHORIZED !== '0',
		ca: Array.from(ca, text),
		cert: options.cert == null ? null : pem(options.cert),
		// pg makes this property non-enumerable, including on the shared config object.
		// Explicitly extracting it avoids silently dropping client authentication.
		key: options.key == null ? null : pem(options.key),
		passphrase: options.passphrase == null ? null : text(options.passphrase),
		// pg supplies SNI for DNS names. With an IP, Node's socket-only tls.connect
		// verifies its default hostname (localhost), without sending SNI.
		servername: isIP(host) ? text(options.servername || 'localhost') : host,
		sni: !isIP(host) || Boolean(options.servername),
		minVersion: version(options.minVersion ?? tls.DEFAULT_MIN_VERSION),
		maxVersion: version(options.maxVersion ?? tls.DEFAULT_MAX_VERSION),
		ciphers: text(options.ciphers ?? tls.DEFAULT_CIPHERS),
	};
}

async function resolvePassword(client: pg.Client, parameters: Record<string, unknown>): Promise<string | null> {
	if (parameters.password !== null && parameters.password !== undefined) return text(parameters.password);
	// Use pg's existing pgpass lookup (no socket or DB connection), including its
	// file-permission checks. A timeout/error never exposes the path or credentials.
	const getPassword: unknown = Reflect.get(client, '_getPassword');
	if (typeof getPassword !== 'function') throw invalid();
	await new Promise<void>((resolve, reject) => {
		const timer = setTimeout(() => finish(invalid()), 1000);
		const onError = () => finish(invalid());
		const finish = (error?: Error) => {
			clearTimeout(timer);
			client.removeListener('error', onError);
			if (error) reject(error); else resolve();
		};
		client.once('error', onError);
		try { getPassword.call(client, () => finish()); } catch { finish(invalid()); }
	});
	return parameters.password == null ? null : text(parameters.password);
}

async function normalizeTarget(config: DatabaseConfig, source: Omit<Config['db'], 'extra'>): Promise<RustPostgresTarget> {
	const extra = config.db.extra ?? {};
	if (Object.keys(extra).some(key => !extraKeys.has(key))) throw invalid();
	const options = {
		host: source.host, port: source.port, user: source.user, password: source.pass, database: source.db,
		statement_timeout: 10_000, ...extra,
	};
	// Reuse pg's precedence, URL parsing, PG* environment handling and SSL file
	// loading. Constructing a Client does not connect to PostgreSQL.
	const client = new pg.Client(options);
	const parameters: unknown = Reflect.get(client, 'connectionParameters');
	const startupMethod: unknown = Reflect.get(client, 'getStartupConf');
	if (!object(parameters) || typeof startupMethod !== 'function') throw invalid();
	const startup: unknown = startupMethod.call(client);
	if (!object(startup)) throw invalid();
	const startupKeys = new Set(['user', 'database', 'options', 'application_name', 'statement_timeout', 'lock_timeout', 'idle_in_transaction_session_timeout']);
	// Replication protocol can also enter through a URL or PGREPLICATION. It is
	// not a SQL read connection and must never be silently turned into one.
	if (Object.keys(startup).some(key => !startupKeys.has(key))) throw invalid();
	if (parameters.client_encoding && !['utf8', 'utf-8', 'UTF8', 'UTF-8'].includes(text(parameters.client_encoding))) throw invalid();
	const host = text(parameters.host);
	const port = parameters.port;
	if (typeof port !== 'number' || !Number.isInteger(port) || port < 1 || port > 65535) throw invalid();
	const negotiation = parameters.sslnegotiation ?? 'postgres';
	if (negotiation !== 'postgres' && negotiation !== 'direct') throw invalid();
	return {
		host, port, user: text(parameters.user), database: text(parameters.database),
		password: await resolvePassword(client, parameters),
		options: optionalText(startup.options), applicationName: optionalText(startup.application_name),
		statementTimeoutMs: milliseconds(startup.statement_timeout), lockTimeoutMs: milliseconds(startup.lock_timeout),
		idleInTransactionSessionTimeoutMs: milliseconds(startup.idle_in_transaction_session_timeout),
		connectTimeoutMs: milliseconds(Reflect.get(client, '_connectionTimeoutMillis')),
		queryTimeoutMs: milliseconds(parameters.query_timeout),
		keepAlive: Boolean(Reflect.get(options, 'keepAlive')),
		keepAliveInitialDelayMs: milliseconds(Reflect.get(options, 'keepAliveInitialDelayMillis')),
		channelBinding: Boolean(Reflect.get(options, 'enableChannelBinding')),
		sslNegotiation: negotiation, tls: normalizeTls(parameters.ssl, host),
	};
}

/** Unsupported settings fail closed for this operation; catalog operations remain available. */
export async function resolveRustPostgresConfig(config?: DatabaseConfig): Promise<RustPostgresConfig | null> {
	if (!config) return null;
	try {
		const sources = config.dbReplications && config.dbSlaves?.length ? config.dbSlaves : [config.db];
		const result = { targets: await Promise.all(sources.map(source => normalizeTarget(config, source))) };
		// Keep credential/bootstrap growth from preventing the unrelated catalog startup.
		return Buffer.byteLength(JSON.stringify(result)) <= 8 * 1024 * 1024 ? result : null;
	} catch { return null; }
}
