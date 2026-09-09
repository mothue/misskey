import { createWriteStream, promises as fsp } from "node:fs";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { finished } from "node:stream/promises";
import { fileURLToPath } from "node:url";

import walk from "ignore-walk";
import { Pack } from "tar/pack";

import meta from "../package.json" with { type: "json" };

const cwd = fileURLToPath(new URL("..", import.meta.url));

const ignore = [
	"**/.git/**/*",
	"**/*ignore",
	"**/.gitmodules",
	// Exclude files you don't want to include in the tarball here
	".config/default.yml",
	".config/docker.env",
	".config/*.yml",
	".config/*.env",
	"files/**/*",
	"**/*.pem",
	"**/*.key",
	".ssh/**/*",
];

export async function buildTarball() {
	const outputDir = resolve(cwd, "built", "tarball");
	await mkdir(outputDir, { recursive: true });

	const pack = new Pack({ cwd, gzip: true });

	const patterns = await walk({
		path: cwd,
		ignoreFiles: [".gitignore"],
	});

	for await (const entry of fsp.glob(patterns, {
		cwd,
		ignore,
		dot: true,
	})) {
		pack.add(entry);
	}

	const output = createWriteStream(
		resolve(outputDir, `misskey-${meta.version}.tar.gz`),
	);

	pack.pipe(output);
	pack.end();

	await finished(output);
}
