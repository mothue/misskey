import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const root = import.meta.dirname;
const env = { ...process.env };
delete env.CARGO_BUILD_TARGET;
const result = spawnSync('cargo', [
	'build', '--locked', '--release', '--lib', '--target-dir', join(root, 'target'),
], {
	cwd: root,
	stdio: 'inherit',
	env,
});
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);
const library = process.platform === 'win32' ? 'slacc.dll'
	: process.platform === 'darwin' ? 'libslacc.dylib' : 'libslacc.so';
mkdirSync(join(root, 'built'), { recursive: true });
copyFileSync(join(root, 'target', 'release', library), join(root, 'built', 'slacc.node'));
