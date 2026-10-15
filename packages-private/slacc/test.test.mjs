import { describe, expect, test } from 'bun:test';
import { generateKeyPairSync, verify } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import slacc from './index.js';
import { createRequire } from 'node:module';

slacc.init(1);

describe('local slacc addon', () => {
	test('loads the same addon through CommonJS', () => {
		expect(createRequire(import.meta.url)('./index.cjs')).toBe(slacc);
	});

	test('matches words including Japanese text', () => {
		const matcher = slacc.AhoCorasick.withPatterns(['Misskey', '日本語']);
		expect(matcher.isMatch('Hello Misskey')).toBe(true);
		expect(matcher.isMatch('日本語の投稿')).toBe(true);
		expect(matcher.isMatch('unrelated')).toBe(false);
	});

	test('signs RSA messages', async () => {
		const { privateKey, publicKey } = generateKeyPairSync('rsa', {
			modulusLength: 2048,
			privateKeyEncoding: { type: 'pkcs8', format: 'pem' },
			publicKeyEncoding: { type: 'spki', format: 'pem' },
		});
		const message = Buffer.from('local Rust addon');
		const key = slacc.RsaKeyPair.fromPem(privateKey);
		const signature = await new Promise((resolve, reject) => {
			key.sign(message, (error, result) => error ? reject(error) : resolve(result));
		});
		expect(verify('RSA-SHA256', message, publicKey, signature)).toBe(true);
	});

	test('rejects invalid RSA keys', () => {
		expect(() => slacc.RsaKeyPair.fromPem('invalid')).toThrow();
	});

	test('extracts ZIP buffers', () => {
		const destination = mkdtempSync(join(tmpdir(), 'slacc-test-'));
		try {
			const archive = Buffer.from('UEsDBBQAAAAAAI8ZR104r0w/CwAAAAsAAAAJAAAAaGVsbG8udHh0bG9jYWwgc2xhY2NQSwECFAMUAAAAAACPGUddOK9MPwsAAAALAAAACQAAAAAAAAAAAAAAgAEAAAAAaGVsbG8udHh0UEsFBgAAAAABAAEANwAAADIAAAAAAA==', 'base64');
			const reader = slacc.ZipReader.withDestinationPath(destination);
			reader.viaBuffer(archive);
			expect(readFileSync(join(destination, 'hello.txt'), 'utf8')).toBe('local slacc');
			expect(() => reader.viaBuffer(Buffer.from('invalid'))).toThrow();
		} finally {
			rmSync(destination, { recursive: true, force: true });
		}
	});
});
