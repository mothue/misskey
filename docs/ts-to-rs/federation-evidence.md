# Federation validation evidence (2026-10-23)

## Local fixture isolation

The generated, ignored `built/ts-to-rs/federation-compose.json` resolves the repository federation Compose configuration. Local-only adjustments were needed on macOS:

- Networks use 172.28/29/30/31 ranges to avoid unrelated active Docker networks. No unrelated networks were removed.
- Fresh project-owned named PostgreSQL volumes replace host bind directories that failed PostgreSQL ownership checks.
- `slacc` is built in Linux using the existing federation tester image (Bun 1.4.4, Rust 1.99.0), with its existing `licenses:check` and build scripts. Separate named built/target volumes protect host macOS artifacts; apps mount the Linux addon read-only, tester mounts it writable. No slacc source was changed.
- Backend Rust uses its existing independent one-shot builder and named binary volume; app mounts remain read-only.
- Tester explicitly writes compiled configuration to `/tmp/federation-tester-config.json` using `MISSKEY_CONFIG_JSON`. The existing nested app configuration mount creates an empty host `built/._config_.json` placeholder; host suites therefore need an explicit distinct compiled configuration path while the mounts exist.

## Diagnostic run (not acceptance)

`federation-isolated-tests.log` is diagnostic only. It passed initial cases then stalled in top-level `fetchAdmin` because existing signin rate limiting allows 10 attempts/hour per IP; federation bulk account creation exceeds that. One `FLUSHALL` was performed against **only** container `test-federation-redis.test-1` during the run, clearing all databases/keys in that dedicated fixture. This unblocked further cases temporarily, but the limiter recurred. Because fixture state changed during execution, this run is discarded for acceptance and was stopped.

## Clean acceptance run

Before the new run, all project containers were stopped. New `db-a-clean`, `db-b-clean`, and `redis-clean` named volumes started empty; cached Linux native binaries were retained. Generated per-app configuration copies add only `enableIpRateLimit: false`, avoiding bulk-fixture signin throttling. No production source configuration was edited. This validates federation compatibility with IP rate limiting disabled for fixture creation, and provides no rate-limit behavior coverage. No fixture reset or state mutation is permitted during this run.

First clean run: **FAIL**, 9 files failed; 4 failed and 101 skipped tests (43.87 s). Logs: `built/ts-to-rs/federation-clean-up.log`, `federation-clean-tests.log`, `federation-clean-services.log`. Server logs identify `Blocked address: 172.28.0.4`: the generated app configuration still allowed the original 172.20.0.0/16 subnet, while the local isolated Compose network uses 172.28.0.0/16. This is an environment configuration mismatch, not evidence of a Rust business-port regression.

Final clean rerun uses another fresh pair of PostgreSQL volumes and fresh Redis volume. Local app configuration additionally changes `allowedPrivateNetworks` from `[127.0.0.1/32, 172.20.0.0/16]` to `[127.0.0.1/32, 172.28.0.0/16]`, matching the relocated test network. Result: **PASS**, exit 0: all 9 test files passed; 92 tests passed and 13 tests skipped (105 total), duration 98.97 s. Logs: `federation-final-up.log`, `federation-final-tests.log`, `federation-final-services.log`. This was a complete invocation of the unmodified `bun run --filter backend test:fed`, including the slacc license/build prerequisite. Skipped tests remain unverified. No fixture reset or state mutation occurred during the final run.

`federation-final-process-a.txt` and `federation-final-process-b.txt` record exactly one `/misskey/packages/backend-rust/built/bin/misskey-backend-rust` child per concrete app; both apps and nginx were healthy (`federation-final-status.txt`). The existing Linux slacc builder completed successfully; native source and host macOS artifacts were untouched.

After evidence capture, only this `test-federation` project was brought down. Named volumes and logs were retained; no unrelated containers, networks, or volumes were removed.
