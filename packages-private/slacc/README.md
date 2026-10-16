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
tests build the addon automatically. Cargo reuses unchanged compilation outputs. The addon is published with an
atomic rename so rebuilding in a shared workspace preserves libraries already
loaded by running servers.
`bun install` does not build Rust or download slacc platform packages.

The backend keeps `slacc` external to its JavaScript bundle and resolves this
workspace's `index.js` (or `index.cjs` for CommonJS), which loads only the local addon.
Deploy this package (including both loaders, `LICENSE`,
`THIRD-PARTY-NOTICES.txt`, and the entire `built/` directory) alongside the backend
and its workspace dependency links. Build on the same OS, architecture, and
libc as the runtime. Docker builds the addon separately in its target-platform
stage and copies that binary into the runtime image.

After changing Rust sources, rebuild and restart the backend; Rust files are
not watched automatically. Run `bun run --filter slacc test` to verify word
matching, RSA signing, and ZIP extraction against the local addon.

## Third-party licenses

`THIRD-PARTY-NOTICES.txt` preserves license texts and copyright notices for the
complete `Cargo.lock` graph, including build-only and other-platform crates.
This deliberately includes more than the code linked into a particular addon.
Native bundled licenses (for example AWS-LC and Zstandard) are included recursively.
It covers slacc's Cargo dependencies, not every component of the Docker base image.

Distribute this file together with `LICENSE` and the addon. Both are committed,
so the source archive and Docker runtime's repository copy include them.
Every addon build checks that the notices match the locked dependencies.
It also copies `LICENSE` and `THIRD-PARTY-NOTICES.txt` beside `built/slacc.node`.
`built/rust-licenses/` contains the standard library copyright notices and license
texts shipped with the exact Rust compiler used for that build. Keep this
entire directory when distributing the addon. Rust's standard library is outside
Cargo's dependency graph; it is covered by these additional build-time notices.
Docker copies the entire `built/` directory, including these files.

After updating Rust dependencies:

```sh
bun run --filter slacc licenses
bun run --filter slacc licenses:check
bun run --filter slacc test:licenses
```

Generation uses `cargo metadata --locked` and the original files from downloaded
crates; Cargo may download crates for other platforms. It does not fetch license
texts from moving upstream branches. Crates missing packaged license files use
committed supplements from the exact upstream commits recorded in their
`.cargo_vcs_info.json`; their sources are listed in `license-policy.json`.
Review any new license expression and missing text before updating that policy.
`OR` expressions use the alternatives selected in the policy; `AND` requirements
are retained. Original license texts can still mention unselected alternatives.
