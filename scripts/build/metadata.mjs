/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import * as fs from 'node:fs';

const rootDir = import.meta.dirname + '/../..';

const packageJsonPath = rootDir + '/package.json'

function build() {
	try {
		const json = fs.readFileSync(packageJsonPath, 'utf-8')
		const meta = JSON.parse(json);
		fs.mkdirSync(rootDir + '/built', { recursive: true });
		fs.writeFileSync(rootDir + '/built/meta.json', JSON.stringify({ version: meta.version }), 'utf-8');
	} catch (e) {
		console.error(e)
	}
}

build();

if (process.argv.includes("--watch")) {
	fs.watch(packageJsonPath, (event, filename) => {
		console.log(`update ${filename} ...`)
		build()
	})
}
