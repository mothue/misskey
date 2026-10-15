# Local slacc

This MIT-licensed package contains the Rust sources from slacc v0.1.5
(commit `d2829db13db93682a3d9f710c94e797cc9c152a7`). Sources are maintained
in this repository; no submodule, fork checkout, or upstream remote is needed.
The original copyright notice is preserved in `LICENSE`.

## Build

Install Rust and Cargo, CMake, and a native C/C++ compiler. Windows also needs
the MSVC build tools. Run `bun run --filter slacc build` from the repository
root to build the host's release addon with the committed `Cargo.lock`.
The output is `built/slacc.node`; intermediate files are in `target/`.
Both directories are excluded from Git.

Normal Misskey builds, backend builds, backend development startup, and backend
tests build the addon automatically. Cargo reuses unchanged compilation outputs.
`bun install` does not build Rust or download slacc platform packages.

The backend keeps `slacc` external to its JavaScript bundle and resolves this
workspace's `index.js` (or `index.cjs` for CommonJS), which loads only the local addon.
Deploy this package (including both loaders, `LICENSE`, and `built/slacc.node`) alongside the backend
and its workspace dependency links. Build on the same OS, architecture, and
libc as the runtime. Docker builds the addon separately in its target-platform
stage and copies that binary into the runtime image.

After changing Rust sources, rebuild and restart the backend; Rust files are
not watched automatically. Run `bun run --filter slacc test` to verify word
matching, RSA signing, and ZIP extraction against the local addon.
