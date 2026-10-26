/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
import { Module } from '@nestjs/common';
import { RustBackendService } from './RustBackendService.js';
@Module({ providers: [RustBackendService], exports: [RustBackendService] })
export class RustBackendModule {}
