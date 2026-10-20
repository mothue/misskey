/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import type { TelemetryService } from '@/core/telemetry/TelemetryService.js';
import { serializeLogError } from '@/logging/LogNormalizer.js';
import type { Job } from 'bullmq';

type QueueTelemetryService = Pick<TelemetryService, 'startSpan'>;

/** Queueのprocessorを実行し、失敗処理をSpan内で行います。 */
export function runQueueJob<T>(
	telemetryService: QueueTelemetryService,
	spanName: string,
	processJob: () => T | Promise<T>,
	onError: (error: Error) => void,
	job?: Pick<Job, 'log' | 'opts'>,
): Promise<T> {
	return telemetryService.startSpan(spanName, async (): Promise<T> => {
		try {
			return await processJob();
		} catch (error) {
			// 失敗イベントを待たず、processor Spanがactiveな間にログと通知を行います。
			const normalizedError = error instanceof Error ? error : new Error(String(error));
			try {
				onError(normalizedError);
			} catch {
				// 失敗ログの処理が例外を投げても、Queueへは元のエラーを返します。
			}
			if (job) {
				try {
					// 更新前のジョブを手動再試行した場合も、直近20件までに制限します。
					const keepLogs = job.opts.keepLogs;
					job.opts.keepLogs = keepLogs != null && keepLogs > 0 ? Math.min(keepLogs, 20) : 20;
					// 既存の管理者向けジョブログAPIから取得できる形で保存します。
					await job.log(JSON.stringify({
						eventName: 'queue.job.failed',
						timestamp: new Date().toISOString(),
						error: serializeLogError(normalizedError, { limits: { maxBytes: 16 * 1024 } }),
					}));
				} catch {
					// 保存先の障害によって本来の失敗理由や再試行の判定を変えません。
				}
			}
			throw error;
		}
	});
}
