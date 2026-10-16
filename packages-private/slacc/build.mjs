import { spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, mkdirSync, rmSync } from 'node:fs';
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
for (const name of ['LICENSE', 'THIRD-PARTY-NOTICES.txt']) {
	copyFileSync(join(root, name), join(root, 'built', name));
}

// The standard library is linked into the addon but is outside Cargo.lock.
// Query a dependency-free crate through Cargo to respect compiler overrides
// without invalidating slacc's incremental build fingerprint.
const sysroot = spawnSync('cargo', [
	'rustc', '--offline', '--locked', '--release', '--lib',
	'--manifest-path', join(root, 'license-query', 'Cargo.toml'),
	'--target-dir', join(root, 'target', 'license-query'),
	'--', '--print', 'sysroot',
], {
	cwd: root, encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'inherit'],
});
if (sysroot.error) throw sysroot.error;
if (sysroot.status !== 0) throw new Error('Failed to locate the compiler standard library notices');
const rustDocs = join(sysroot.stdout.trim(), 'share', 'doc', 'rust');
const rustNotices = join(root, 'built', 'rust-licenses');
rmSync(rustNotices, { recursive: true, force: true });
mkdirSync(rustNotices, { recursive: true });
copyFileSync(join(rustDocs, 'COPYRIGHT-library.html'), join(rustNotices, 'COPYRIGHT-library.html'));
cpSync(join(rustDocs, 'licenses'), join(rustNotices, 'licenses'), { recursive: true });
