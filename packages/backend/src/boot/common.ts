/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { NestFactory } from '@nestjs/core';
import { init } from 'slacc';
import { NestLogger } from '@/NestLogger.js';
import { envOption } from '@/env.js';
import type { Config } from '@/config.js';

let slaccInitialized = false;
let activeServer: { dispose(): Promise<void> } | undefined;
export async function stopServer(): Promise<void> { await activeServer?.dispose(); }

export function initExtraThreadPool(config: Config) {
	if (slaccInitialized) return;

	const threadPoolSize = Math.max(config.threadPoolSize ?? 1, 1);

	init(threadPoolSize);

	slaccInitialized = true;
}

export async function server() {
	const { MainModule } = await import('../MainModule.js');
	const { ServerService } = await import('../server/ServerService.js');

	const app = await NestFactory.createApplicationContext(MainModule, {
		logger: new NestLogger(),
	});

	const serverService = app.get(ServerService);
	activeServer = serverService;
	try { await serverService.launch(); } catch (error) { await serverService.dispose(); throw error; }

	if (process.env.NODE_ENV !== 'test' && !envOption.noDaemons) {
		const { ChartManagementService } = await import('../core/chart/ChartManagementService.js');
		const { QueueStatsService } = await import('../daemons/QueueStatsService.js');
		const { ServerStatsService } = await import('../daemons/ServerStatsService.js');

		app.get(ChartManagementService).start();
		app.get(QueueStatsService).start();
		app.get(ServerStatsService).start();
	}

	return app;
}

export async function jobQueue() {
	const { QueueProcessorModule } = await import('../queue/QueueProcessorModule.js');
	const { QueueProcessorService } = await import('../queue/QueueProcessorService.js');
	const { ChartManagementService } = await import('../core/chart/ChartManagementService.js');

	const jobQueue = await NestFactory.createApplicationContext(QueueProcessorModule, {
		logger: new NestLogger(),
	});

	jobQueue.get(QueueProcessorService).start();
	if (!envOption.noDaemons) {
		jobQueue.get(ChartManagementService).start();
	}

	return jobQueue;
}
