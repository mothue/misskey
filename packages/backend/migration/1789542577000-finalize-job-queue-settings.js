/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import fs from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const defaults = {
	deliverJobConcurrency: 128,
	inboxJobConcurrency: 16,
	deliverJobPerSec: 128,
	inboxJobPerSec: 32,
};

function loadConfigValues() {
	const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
	const configPath = resolve(repoRoot, 'built/.config.json');

	if (!fs.existsSync(configPath)) {
		throw new Error('Compiled configuration not found; refusing to migrate job queue settings without preserving existing YAML values.');
	}

	const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
	const values = {
		deliverJobConcurrency: config.deliverJobConcurrency,
		inboxJobConcurrency: config.inboxJobConcurrency,
		deliverJobPerSec: config.deliverJobPerSec,
		inboxJobPerSec: config.inboxJobPerSec,
	};

	for (const [key, value] of Object.entries(values)) {
		if (!Number.isInteger(value) || value < 1) values[key] = defaults[key];
	}

	return values;
}

export class FinalizeJobQueueSettings1789542577000 {
	name = 'FinalizeJobQueueSettings1789542577000';

	async up(queryRunner) {
		const values = loadConfigValues();

		await queryRunner.query(`
			UPDATE "meta"
			SET
				"deliverJobConcurrency" = COALESCE("deliverJobConcurrency", $1),
				"inboxJobConcurrency" = COALESCE("inboxJobConcurrency", $2),
				"deliverJobPerSec" = COALESCE("deliverJobPerSec", $3),
				"inboxJobPerSec" = COALESCE("inboxJobPerSec", $4)
		`, [
			values.deliverJobConcurrency,
			values.inboxJobConcurrency,
			values.deliverJobPerSec,
			values.inboxJobPerSec,
		]);

		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobConcurrency" SET DEFAULT 128`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobConcurrency" SET DEFAULT 16`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobPerSec" SET DEFAULT 128`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobPerSec" SET DEFAULT 32`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobConcurrency" SET NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobConcurrency" SET NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobPerSec" SET NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobPerSec" SET NOT NULL`);
	}

	async down(queryRunner) {
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobPerSec" DROP NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobPerSec" DROP NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobConcurrency" DROP NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobConcurrency" DROP NOT NULL`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobPerSec" DROP DEFAULT`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobPerSec" DROP DEFAULT`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "inboxJobConcurrency" DROP DEFAULT`);
		await queryRunner.query(`ALTER TABLE "meta" ALTER COLUMN "deliverJobConcurrency" DROP DEFAULT`);
	}
}
