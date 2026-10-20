/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { createWriteStream, promises as fsp } from "node:fs";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

import walk from "ignore-walk";
import { Pack } from "tar/pack";

import meta from "../../package.json" with { type: "json" };

const cwd = fileURLToPath(new URL("../..", import.meta.url));

const ignore = [
	".git",
	".git/**",
	"**/.git",
	"**/.git/**",

	".env",
	".env.*",
	"**/.env",
	"**/.env.*",

	".config",
	".config/**",
	"**/.config",
	"**/.config/**",

	"files",
	"files/**",

	".ssh",
	".ssh/**",
	"**/.ssh",
	"**/.ssh/**",

	"**/*.pem",
	"**/*.key",

	"**/*ignore",
	"**/.gitmodules",
];

export async function buildTarball() {
	const outputDir = resolve(cwd, "built", "tarball");
	await mkdir(outputDir, { recursive: true });

	const pack = new Pack({ cwd, gzip: true });

	const output = createWriteStream(
		resolve(outputDir, `misskey-${meta.version}.tar.gz`),
	);

	const completion = pipeline(pack, output);

	const patterns = await walk({
		path: cwd,
		ignoreFiles: [".gitignore"],
	});

	for await (const entry of fsp.glob(patterns, {
		cwd,
		exclude: ignore,
	})) {
		pack.add(entry);
	}

	pack.end();
	await completion;
}

if (import.meta.main) {
	await buildTarball();
}
