# PostgreSQL original-operation transport context

Cloud evidence collected on 2026-10-23 (UTC), starting from clean `feat/ts-to-rs` at `614c6f7421a9f6168e4345409d1b5090958be9d6`. Source commit: `e6eb9d488c1ea4ced63844ddf003cfeaa1e86353`. This is an internal adapter foundation change. It adds no native business API activation or public error formatter. The five existing native business handlers and 433 TS business callbacks remain; the 24 database-read activations and four date-pagination APIs remain held.

## Implementation and limits

`storage-postgres/src/transport.rs` owns the selected TCP target's original resolution after TLS configuration. Numeric IPv4/IPv6 bypass the hostname resolver. Hostnames use one Tokio lookup, keep the returned order and duplicates, and supply each IP through a fresh `Config::hostaddr` clone. Every actual attempt still uses native `Config::connect`; first success stops traversal and the final failure remains authoritative. Unix sockets retain the native path. No probe, second lookup, callback replay, replica-to-primary fallback or new public RPC is introduced.

Private context retains the resolver host/count, current/final attempted IP/index, TLS servername and actual OpenSSL preverification boolean/code/depth/current certificate. The verification callback returns its incoming boolean unchanged; disabled verification installs no observation callback. Cancellation owns a separate observer and keeps the native successful-IP CancelToken route, TLS policy and existing total 500 ms bound. Connection establishment, session SET placement, query deadlines, driver-cause capture and shutdown remain intact. Error context is boxed privately to keep typed result errors small. Debug/Display and Error::source remain redacted; credentials, SQL and certificate/key contents are not formatted.

These are observed driver facts, not Bun error strings. `legacy_message()` remains absent for transport faults. The adapter does not retain the full resolver answer/prior-attempt history, a Unix path snapshot, handshake-entry events or the SSLRequest reply byte. A factory reached without a verification callback does not identify SSL reply N versus E, and a Tokio I/O error does not establish a portable EAI code. No public fault parity or permission to reactivate the held APIs follows from these tests.

## Environment, isolation and provenance

Linux Debian 13 x86_64; Bun 1.4.4, Rust 1.99.0, Node 24.19.0. Dedicated project `cloud-pg-transport-context` uses PostgreSQL 18 at loopback 54312 and Redis 7 at loopback 56312. Private JSON settings explicitly select `test-misskey`/Redis DB14 for backend E2E, `online-adapter` for the adapter lifecycle, and `api-layer-runtime`/Redis DB12 for public HTTP and benchmarks. Live Rust Redis tests use only Redis DB15 and their own keys. No shared or production service is used. Loopback fixture DNS/proxy exceptions are explicit; other traffic keeps the managed network policy and proxy.

Before rebuilding, 64 backend bundle files and the native binary were privately copied from the starting build; a 65-entry SHA256 manifest binds that immutable baseline. Dependency/static links are checked separately. The final four source hashes exactly match the independent review and source commit. `final-build-provenance.json` binds the final native build; public comparison after that build is separate from the earlier baseline capture. The full local backend E2E ran on the first transport build before the private context boxing amendment; the final source separately passed Rust/live checks, build and public HTTP. Final-SHA CI must be checked separately rather than relabeling that earlier local artifact.

Detailed commands, working directories, durations, exit codes and sanitized logs are under `/workspace/pg-transport-context/results.jsonl` and `logs/`; benchmark and federation have separate command ledgers. Private configs are not repository artifacts and must be excluded from export.

## Executed validation

| Status | Command / fixture | Exit | Result / log under evidence root |
| --- | --- | ---: | --- |
| PASS | `cargo fmt --all -- --check` | 0 | Final source; `logs/rust-fmt-boxed.log` |
| PASS | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | `logs/rust-workspace-clippy-fixed.log` |
| PASS | `cargo test --workspace --locked` | 0 | 113 passed, four explicitly ignored live fixtures; `logs/rust-workspace-tests-boxed.log` |
| PASS | `cargo test --workspace --locked -- --ignored --test-threads=1` with explicit dedicated settings | 0 | All four live fixtures passed separately; `logs/live-adapters-boxed.log` |
| PASS | `bun run build:e2e` | 0 | Final Linux Rust/slacc/backend build; `logs/backend-final-build.log` |
| PASS | `bun run --bun tsc --noEmit -p tsconfig.json`, `test/tsconfig.json`, `test-federation/tsconfig.json` | 0 each | `logs/backend-types.log`, `test-types.log`, `federation-types.log` |
| PASS | `bun ./node_modules/vitest/vitest.mjs run --config vitest.config.unit.ts src/server/api/endpoint-base.test.ts src/server/rust` | 0 | 52 passed in 8 files; `logs/related-ts-unit.log` |
| PASS | `python3 http-foundation.py` | 0 | 30 paired public HTTP cases, exact body/order/meaningful headers; `logs/public-http-final-boxed.log`, `http-comparison.json` |
| PASS | `python3 lifecycle-public.py child-loss` | 0 | Five APIs twice/build remain503 after SIGKILL; no respawn; parent exits0; `lifecycle-child-loss.json` |
| PASS | `python3 lifecycle-public.py db-refusal` | 0 | Dedicated proxy refusal: native count503 and legacy business500 with matching `info.e`; `lifecycle-db-refusal.json` |
| PASS | `bun --preload <evidence>/localhost-dns-preload.mjs ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts --execArgv=--preload --execArgv=<evidence>/localhost-dns-preload.mjs --reporter=verbose --reporter=json --outputFile=<evidence>/full-backend-e2e.json` | 0 | 37 files, 1,475 passed, 2 skipped, 20 TODO; first-build provenance limit above; `logs/full-backend-e2e.log` |
| PASS | `python3 federation/execute-with-monitor.py --execute --minimum-free-gib 5 --reason <recorded-capacity-rationale>` | 0 | 9 files, 92 passed, 13 skipped; all phases/cleanup exit0; `federation/evidence.json` |
| PASS | `bun benchmark/measure.ts --execute-authorized` through the reviewed recorder | 0 | 34,000 measured +3,400 warmup, zero errors; `benchmark/results.json`, `summary.json`, [all 35 metrics](postgres-transport-benchmark.csv) |
| PASS | Independent fresh-context source review | n/a | No open findings; `review-source.json` binds all four files, including the boxing amendment |
| PASS | `node scripts/checks/shipping.mjs --base 614c6f7421a9f6168e4345409d1b5090958be9d6` | 0 | SPDX and locale safety; ESLint SKIPPED because no target TS/Vue files |

The 13 new default PostgreSQL tests cover injected resolver failures/no answers, IPv4 and actual supported IPv6 numeric routes, ordered duplicate/distinct addresses, first-success/final-error selection, lookup cancellation and later independent recovery, opaque callers, Unix bypass and session deadline placement. Real loopback TLS fixtures generate keys/certificates only in memory and compare against callback-disabled native controls: trusted/untrusted/wrong DNS/wrong IP/verification disabled, SNI, ALPN, postgres/direct modes, per-attempt isolation and native Drop cancellation. Loopback IPv6 support was independently checked on this cloud host. Injected resolver errors establish seam behavior, not a newly measured DNS EAI result. Fake PostgreSQL capability fixtures do not establish actual server-side lock release; the dedicated real PostgreSQL lifecycle test covers cancellation separately.

Only Date response headers, range-checked current ping timestamps and validated fresh inner UUIDv4 values are normalized for HTTP comparisons. Null/omission, additional properties, UTF16 lone surrogates, numeric rounding/nonfinite conversion, duplicate/numeric property ordering, input errors, unknown routes/endpoints and statuses remain strict.

Initial formatting check failed (exit1) and was corrected. Initial clippy failed (exit101) because the new inline context made result errors 184 bytes; private boxing fixed the warning without suppression. Their original logs remain. These are corrected validation failures, not baseline product defects. No new confirmed TS product bug or migration regression was established by this unit. Previously recorded Bun/Node/TypeORM fault baselines remain historical in [postgres-fault-validation.md](postgres-fault-validation.md), not new cloud observations.


Federation froze 1,915 source/runtime files, two host native artifacts and 14 harness inputs; every hash remained unchanged. Its fresh container-native Cargo/slacc outputs were isolated with nocopy volumes, never seeded from host binaries. A fresh no-network marker probe confirmed the new separate `/tmp` Cargo backing path. Only this task's 2.6 GiB host Cargo cache was temporarily relocated to another `/tmp` path, then restored. The documented 5 GiB release guard replaced the conservative 6 GiB default at 6.06 GB overlay free, with 0.5 s capacity sampling, a separate empty container Cargo cache and bounded 512 MiB combined nginx caches. Minimum sampled free space remained 2,118,909,952 bytes overlay and 5,818,187,776 bytes `/tmp`; no ENOSPC occurred. Both dedicated projects and volumes were collected with exit0.

Benchmark baseline 614c6f7 versus candidate e6eb9 used the unchanged shared runner: 100 warmup, 1,000 measured/API/build, concurrency8, three AB/BA/AB pairs plus two TS smoke callbacks. All eight servers exited0 and collected their Rust child/process group. Provenance binds complete copied baseline/current bundles, both native binaries and actual child executables, all four committed/reviewed source files, full independent-review artifact, candidate pin, five harness files and private config hash. The recorder separately passed normal/SIGINT/SIGTERM/forced-cleanup fixtures; deliberately interrupted fixture commands are recorded as failures, never successful benchmarks.

| API | Baseline req/s | Candidate req/s | Throughput change (%) | Baseline p95ms | Candidate p95ms | p95 change (%) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| ping | 4460 | 4083 | -8.4 | 3.661 | 3.581 | -2.2 |
| test | 6018 | 5402 | -10.2 | 2.323 | 2.899 | +24.8 |
| endpoint | 5881 | 6826 | +16.1 | 2.283 | 2.041 | -10.6 |
| endpoints | 5219 | 5380 | +3.1 | 3.716 | 2.894 | -22.1 |
| get-online-users-count | 2557 | 2672 | +4.5 | 6.094 | 4.812 | -21.0 |

All ten displayed deltas are white/small-or-noisy under the recorded heuristic: all three paired directions must agree and absolute median-of-runs change must exceed max(5%, baseline population CV, candidate CV) for green/red. This is no significance test or performance acceptance gate; even a large percentage is not established improvement/regression if those repeatability checks fail. CSV includes p50/p95/p99, throughput, sampled TS/Rust/combined RSS, ranges, variance, standard deviation, CV and all paired deltas. The small empty fixture/shared cloud host does not establish production scaling or peak-memory results.

## Explicit pending / skipped work

Final pushed-SHA CI is checked after this evidence commit and recorded in the cloud `ci-final.json`/final report; this document does not predeclare it successful. Local product UI is SKIPPED because no frontend or UI flow changes; public HTTP checks and final CI frontend checks do not prove exhaustive visual equivalence. API type generation is SKIPPED because meta/paramDef/res and public schemas did not change. GitHub CodeRabbit was not invoked; PR title/body and AGENTS.md remain user-owned. No new performance acceptance gate is introduced.

Next implementation work retains the original SSLRequest/handshake/read-write events with one pass-through transport, preserving native socket options, TLS/channel binding, successful-IP cancellation and all deadlines as one unit. Public formatting and held activations require additional Bun differential evidence, including unsupported/unobserved error shapes. Independent Redis-operation work captured 12 actual TS-body assertions using the imported endpoint, ReversiService, ApiCallService and real ioredis. Default retry-limit refusal rejected after about10.5s as `MaxRetriesPerRequestError`; a lost reply plus intervening rejoin caused two actual ZREM commands and removed that rejoined member. The140source hashes stayed unchanged, own DB11 keys were deleted, and clients/proxies closed. These are body-level BASELINE observations in `/workspace/pg-transport-context/reversi-baseline`, not public HTTP/auth/bootstrap or Rust parity. Native activation needs equivalent queue/retry/reply/error/recovery behavior; nominal ZREM idempotence is insufficient.
