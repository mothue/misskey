/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import * as tls from 'node:tls';
import { createPrivateKey } from 'node:crypto';
import type { Config } from '@/config.js';
import { object } from './protocol.js';

export type RustRedisConfig = {
	host: string; port: number; path: string | null; db: number;
	username: string | null; password: string | null; keyPrefix: string;
	connectTimeoutMs: number; tls: { ca: string[]; cert: string | null; key: string | null; insecure: boolean } | null;
};

/** Credentials are returned only to the child bootstrap, never to an HTTP peer. */
export function resolveRustRedisConfig(source: Config['redis']): RustRedisConfig {
	const fail = () => new Error('Unsupported Rust Redis configuration');
	const text = (value: unknown): string => {
		if (typeof value !== 'string' || !value.isWellFormed() || value.includes('\0')) throw fail();
		return value;
	};
	const pem = (value: unknown): string => text(Buffer.isBuffer(value) ? value.toString('utf8') : value);
	const supported = new Set(['host', 'port', 'path', 'db', 'username', 'pass', 'password', 'prefix', 'keyPrefix', 'family', 'tls', 'connectTimeout', 'enableReadyCheck', 'lazyConnect', 'retryStrategy', 'maxRetriesPerRequest', 'enableOfflineQueue', 'showFriendlyErrorStack', 'keepAlive', 'noDelay', 'connectionName']);
	if (Object.keys(source).some(key => !supported.has(key))) throw fail();
	if (source.retryStrategy != null || source.connectionName != null || (source.family != null && source.family !== 0)) throw fail();
	const host = text(source.host ?? '127.0.0.1');
	const port = source.port ?? 6379;
	const db = source.db ?? 0;
	const connectTimeoutMs = source.connectTimeout ?? 10_000;
	if (!Number.isInteger(port) || port < 1 || port > 65535 || !Number.isInteger(db) || db < 0 || !Number.isInteger(connectTimeoutMs) || connectTimeoutMs <= 0) throw fail();
	let security: RustRedisConfig['tls'] = null;
	if (source.tls != null) {
		const options: unknown = source.tls;
		if (!object(options) || Object.keys(options).some(key => !['ca', 'cert', 'key', 'passphrase', 'rejectUnauthorized', 'servername'].includes(key))) throw fail();
		if (options.servername != null && options.servername !== host) throw fail();
		if (options.rejectUnauthorized != null && typeof options.rejectUnauthorized !== 'boolean') throw fail();
		let key: string | null = options.key == null ? null : pem(options.key);
		if (key != null) key = createPrivateKey({ key, passphrase: options.passphrase == null ? undefined : text(options.passphrase) }).export({ format: 'pem', type: 'pkcs8' }).toString();
		security = {
			ca: options.ca == null ? tls.getCACertificates('default') : (Array.isArray(options.ca) ? options.ca.map(pem) : [pem(options.ca)]),
			cert: options.cert == null ? null : pem(options.cert), key,
			insecure: options.rejectUnauthorized === false || (options.rejectUnauthorized == null && process.env.NODE_TLS_REJECT_UNAUTHORIZED === '0'),
		};
	}
	return { host, port, db, path: source.path == null ? null : text(source.path), username: source.username == null ? null : text(source.username), password: source.password == null ? null : text(source.password), keyPrefix: text(source.keyPrefix ?? ''), connectTimeoutMs, tls: security };
}
