/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { createServer, type Server, type ServerResponse, type IncomingMessage } from 'node:http';
import { Readable } from 'node:stream';
import { setTimeout as delay } from 'node:timers/promises';
import { Response } from 'node-fetch';
import { UnrecoverableError } from 'bullmq';
import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from 'vitest';
import { HttpRequestService } from '@/core/HttpRequestService.js';
import { ApRequestService } from '@/core/activitypub/ApRequestService.js';
import { UserWebhookDeliverProcessorService } from '@/queue/processors/UserWebhookDeliverProcessorService.js';
import { SystemWebhookDeliverProcessorService } from '@/queue/processors/SystemWebhookDeliverProcessorService.js';
import { SensitiveMediaDetectionService } from '@/core/SensitiveMediaDetectionService.js';
import type { MiMeta, WebhooksRepository, SystemWebhooksRepository } from '@/models/_.js';
import type { QueueLoggerService } from '@/queue/QueueLoggerService.js';
import type { Job } from 'bullmq';
import type { UserWebhookDeliverJobData, SystemWebhookDeliverJobData } from '@/queue/types.js';
import { genRsaKeyPair } from '@/misc/gen-key-pair.js';
import type { Config } from '@/config.js';
import type { UserKeypairService } from '@/core/UserKeypairService.js';
import type { LoggerService } from '@/core/LoggerService.js';
import type { UtilityService } from '@/core/UtilityService.js';

const config = { userAgent: 'Misskey lifecycle test', url: 'https://local.example' } as Config;

// Real network I/O continues while only the request deadline is advanced by fake timers.
async function settleNetwork(predicate: () => boolean): Promise<void> {
	for (let i = 0; i < 100 && !predicate(); i++) await delay(5);
	expect(predicate()).toBe(true);
}

describe('HTTP response lifecycle', () => {
	let server: Server;
	let url: string;
	let service: HttpRequestService;
	let reply: ServerResponse;
	let closed: boolean;
	let handler: (req: IncomingMessage, res: ServerResponse) => void;

	beforeEach(async () => {
		closed = false;
		handler = (_req, res) => res.flushHeaders();
		server = createServer((req, res) => {
			reply = res;
			res.on('close', () => { closed = true; });
			handler(req, res);
		});
		server.keepAliveTimeout = 0;
		await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
		const address = server.address();
		if (address == null || typeof address === 'string') throw new Error('Missing loopback address');
		url = `http://127.0.0.1:${address.port}/`;
		service = new HttpRequestService(config, { deliverJobConcurrency: 1 });
		vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
	});

	afterEach(async () => {
		vi.useRealTimers();
		vi.restoreAllMocks();
		service.httpAgent.destroy();
		service.httpsAgent.destroy();
		server.closeAllConnections();
		await new Promise<void>(resolve => server.close(() => resolve()));
	});

	test('returns headers before the GET body, then clears the deadline after consumption', async () => {
		const res = await service.send(url, { timeout: 1000 });
		expect(res.status).toBe(200);
		expect(res.bodyUsed).toBe(false);
		expect(vi.getTimerCount()).toBe(1);
		reply.end('{"ok":true}');
		await expect(res.json()).resolves.toEqual({ ok: true });
		expect(vi.getTimerCount()).toBe(0);
	});

	test('keeps the deadline active while a GET body stalls after headers', async () => {
		const res = await service.send(url, { timeout: 1000 });
		const rejected = expect(res.text()).rejects.toMatchObject({ name: 'AbortError' });
		await vi.advanceTimersByTimeAsync(1000);
		await rejected;
		await settleNetwork(() => closed);
		expect(vi.getTimerCount()).toBe(0);
	});

	test('does not extend the absolute deadline when a slow GET receives more bytes', async () => {
		const res = await service.send(url, { timeout: 1000 });
		const rejected = expect(res.text()).rejects.toMatchObject({ name: 'AbortError' });
		reply.write('first');
		await vi.advanceTimersByTimeAsync(900);
		reply.write('second');
		await vi.advanceTimersByTimeAsync(100);
		await rejected;
		await settleNetwork(() => closed);
	});

	test.each([{ method: 'HEAD', status: 200 }, { method: 'GET', status: 204 }])('clears the deadline for unread $method / $status responses', async ({ method, status }) => {
		handler = (_req, res) => { res.writeHead(status); res.end(); };
		const res = await service.send(url, { method, timeout: 1000 });
		await settleNetwork(() => vi.getTimerCount() === 0);
		expect(res.bodyUsed).toBe(false);
	});

	test('clears the deadline when fetching fails before headers', async () => {
		handler = req => req.socket.destroy();
		await expect(service.send(url)).rejects.toMatchObject({ name: 'FetchError' });
		expect(vi.getTimerCount()).toBe(0);
	});

	test.each([{ status: 400, retryable: false }, { status: 429, retryable: true }, { status: 503, retryable: true }])('discards rejected HTTP $status without changing retry classification', async ({ status, retryable }) => {
		handler = (_req, res) => { res.writeHead(status); res.flushHeaders(); };
		await expect(service.send(url)).rejects.toMatchObject({ name: 'StatusError', statusCode: status, isRetryable: retryable });
		expect(vi.getTimerCount()).toBe(0);
		await settleNetwork(() => closed);
	});

	test('preserves the validator exception while cancelling its discarded response', async () => {
		const failure = new Error('Invalid content type');
		await expect(service.send(url, {}, {
			throwErrorWhenResponseNotOk: true,
			validators: [() => { throw failure; }],
		})).rejects.toBe(failure);
		expect(vi.getTimerCount()).toBe(0);
		await settleNetwork(() => closed);
	});

	test('leaves a non-OK body readable when status errors are disabled', async () => {
		handler = (_req, res) => { res.writeHead(503); res.end('retry later'); };
		const res = await service.send(url, {}, { throwErrorWhenResponseNotOk: false });
		await expect(res.text()).resolves.toBe('retry later');
		expect(vi.getTimerCount()).toBe(0);
	});

	test('explicit disposal promptly closes an unread response and is idempotent', async () => {
		const res = await service.send(url);
		service.disposeResponse(res);
		service.disposeResponse(res);
		expect(vi.getTimerCount()).toBe(0);
		await settleNetwork(() => closed);
		expect(res.bodyUsed).toBe(false);
	});

	test('disposal never throws for null or already destroyed bodies', () => {
		expect(() => service.disposeResponse(new Response(null))).not.toThrow();
		const res = new Response('done');
		service.disposeResponse(res);
		expect(() => service.disposeResponse(res)).not.toThrow();
	});

	test('clears the deadline when a consumer cancels the body directly', async () => {
		const res = await service.send(url);
		if (!(res.body instanceof Readable)) throw new Error('Expected a Node response stream');
		res.body.destroy();
		await settleNetwork(() => closed && vi.getTimerCount() === 0);
	});

	test('enforces the configured body size and releases the failed download', async () => {
		const res = await service.send(url, { size: 4 });
		const rejected = expect(res.text()).rejects.toMatchObject({ type: 'max-size' });
		reply.write('too large');
		await rejected;
		await settleNetwork(() => closed && vi.getTimerCount() === 0);
	});

	test('clears the deadline after a response stream fails', async () => {
		const res = await service.send(url);
		const rejected = expect(res.text()).rejects.toThrow();
		reply.socket?.destroy();
		await rejected;
		await settleNetwork(() => vi.getTimerCount() === 0);
	});

	test('disposes compressed response streams without waiting for their body', async () => {
		handler = (_req, res) => { res.setHeader('content-encoding', 'gzip'); res.flushHeaders(); };
		const res = await service.send(url);
		service.disposeResponse(res);
		expect(vi.getTimerCount()).toBe(0);
		await settleNetwork(() => closed);
	});

	test('leaves successful POST JSON responses available to their callers', async () => {
		handler = (_req, res) => res.end('{"accepted":true}');
		const res = await service.send(url, { method: 'POST', body: '{}' });
		await expect(res.json()).resolves.toEqual({ accepted: true });
		expect(vi.getTimerCount()).toBe(0);
	});

	test('follows redirects and preserves the final response URL and readable body', async () => {
		handler = (req, res) => {
			if (req.url === '/') {
				res.writeHead(302, { location: '/final' });
				res.end();
			} else {
				res.end('final body');
			}
		};
		const res = await service.send(url);
		expect(res.url).toBe(`${url}final`);
		await expect(res.text()).resolves.toBe('final body');
		expect(vi.getTimerCount()).toBe(0);
	});

	test('releases failed media detection responses while retaining the fallback result', async () => {
		handler = (_req, res) => { res.writeHead(503); res.flushHeaders(); };
		const detector = new SensitiveMediaDetectionService({
			sensitiveMediaDetectionApiUrl: url,
			sensitiveMediaDetectionApiKey: null,
			sensitiveMediaDetectionTimeout: 5000,
			sensitiveMediaDetectionMaxImagesPerRequest: 4,
		} as MiMeta, service, {
			getLogger: () => ({ warn: () => {} }),
		} as unknown as LoggerService);
		await expect(detector.detectSensitiveMany([Buffer.from('image')])).resolves.toEqual([null]);
		expect(vi.getTimerCount()).toBe(0);
		await settleNetwork(() => closed);
	});

	describe.each([UserWebhookDeliverProcessorService, SystemWebhookDeliverProcessorService])('%s', Processor => {
		test.each([202, 400, 503])('disposes responses while preserving delivery result for HTTP %s', async status => {
			handler = (_req, res) => { res.writeHead(status); res.flushHeaders(); };
			let latestStatus: unknown;
			const repository = {
				update: async (_where: unknown, update: { latestStatus: unknown }) => { latestStatus = update.latestStatus; },
			} as unknown as WebhooksRepository & SystemWebhooksRepository;
			const logger = { debug: () => {}, error: () => {} };
			const processor = new Processor(config, repository, service, {
				logger: { createSubLogger: () => logger },
			} as unknown as QueueLoggerService);
			const result = processor.process({ data: { to: url, webhookId: 'hook', content: {} } } as Job<UserWebhookDeliverJobData & SystemWebhookDeliverJobData>);
			if (status === 202) await expect(result).resolves.toBe('Success');
			else if (status === 400) await expect(result).rejects.toBeInstanceOf(UnrecoverableError);
			else await expect(result).rejects.toThrow('503');
			expect(latestStatus).toBe(status);
			expect(vi.getTimerCount()).toBe(0);
			await settleNetwork(() => closed);
		});
	});

	describe('ActivityPub delivery', () => {
		let keypair: Awaited<ReturnType<typeof genRsaKeyPair>>;
		beforeAll(async () => { keypair = await genRsaKeyPair(); });

		function apService(): ApRequestService {
			return new ApRequestService(config,
				{ getUserKeypair: async () => keypair } as unknown as UserKeypairService,
				service,
				{ getLogger: () => ({}) } as unknown as LoggerService,
				{} as UtilityService,
			);
		}

		test('accepts a signed POST once and closes an unread successful response without waiting', async () => {
			let deliveries = 0;
			handler = (req, res) => {
				deliveries++;
				expect(req.method).toBe('POST');
				expect(req.headers.signature).toContain('rsa-sha256');
				res.writeHead(202);
				res.flushHeaders();
			};
			await expect(apService().signedPost({ id: 'alice' }, url, { type: 'Create' })).resolves.toBeUndefined();
			expect(vi.getTimerCount()).toBe(0);
			await settleNetwork(() => closed);
			expect(deliveries).toBe(1);
		});

		test('accepted delivery stays successful when response destruction fails', async () => {
			// Inject only the cleanup failure: a network pipeline also calls destroy on its own.
			const body = new Readable({ read: () => {} });
			const response = new Response(body, { status: 202 });
			vi.spyOn(body, 'destroy').mockImplementation(() => { throw new Error('cleanup failed'); });
			vi.spyOn(service, 'send').mockResolvedValue(response);
			await expect(apService().signedPost({ id: 'alice' }, url, {})).resolves.toBeUndefined();
			expect(vi.getTimerCount()).toBe(0);
		});

		test('signed GET still consumes valid ActivityPub JSON', async () => {
			handler = (_req, res) => {
				res.setHeader('content-type', 'application/activity+json');
				res.end(JSON.stringify({ id: url, type: 'Person' }));
			};
			await expect(apService().signedGet(url, { id: 'alice' })).resolves.toEqual({ id: url, type: 'Person' });
			expect(vi.getTimerCount()).toBe(0);
		});

		test('signed GET rejects an invalid content type and promptly discards its body', async () => {
			handler = (_req, res) => { res.setHeader('content-type', 'text/plain'); res.flushHeaders(); };
			await expect(apService().signedGet(url, { id: 'alice' })).rejects.toThrow();
			expect(vi.getTimerCount()).toBe(0);
			await settleNetwork(() => closed);
		});
	});
});
