/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

export class AddJobQueueSettings1789537944000 {
	name = 'AddJobQueueSettings1789537944000'

	async up(queryRunner) {
		await queryRunner.query(`ALTER TABLE "meta" ADD "deliverJobConcurrency" integer`);
		await queryRunner.query(`ALTER TABLE "meta" ADD "inboxJobConcurrency" integer`);
		await queryRunner.query(`ALTER TABLE "meta" ADD "deliverJobPerSec" integer`);
		await queryRunner.query(`ALTER TABLE "meta" ADD "inboxJobPerSec" integer`);
	}

	async down(queryRunner) {
		await queryRunner.query(`ALTER TABLE "meta" DROP COLUMN "inboxJobPerSec"`);
		await queryRunner.query(`ALTER TABLE "meta" DROP COLUMN "deliverJobPerSec"`);
		await queryRunner.query(`ALTER TABLE "meta" DROP COLUMN "inboxJobConcurrency"`);
		await queryRunner.query(`ALTER TABLE "meta" DROP COLUMN "deliverJobConcurrency"`);
	}
}
