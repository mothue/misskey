/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
export interface RecordResponse { status: number; headers: Record<string, string>; body: string; }
export function comparable(response: RecordResponse) { return { ...response, headers: Object.fromEntries(Object.entries(response.headers).map(([name, value]) => [name.toLowerCase(), value]).filter(([name]) => name !== 'date').sort(([a], [b]) => a.localeCompare(b))) }; }
