# PostgreSQL fault-source retention: cloud validation

Date: 2026-10-23 (Asia/Tokyo). Repository `mothue/misskey`, branch `feat/ts-to-rs`, starting clean HEAD `e43904c4b441df698d0bd1c3fd60e745bb4d4249`. Implementation commit: [698b15b5b3](https://github.com/mothue/misskey/commit/698b15b5b3774ede5942026f2a9ba31de4c24c53). Draft [PR48](https://github.com/mothue/misskey/pull/48) is reused; its title/body remain user-owned and unchanged. No GitHub CodeRabbit review is requested.

## Implemented boundary and acceptance limit

`storage-postgres` now retains actual typed driver sources before they become the existing opaque `Failure`. `QueryError::Fault(QueryFault)` distinguishes connection/startup, read-only session setup and query observations. Its accessors expose concrete IO kind/errno, PostgreSQL SQLSTATE/message, known TLS-source presence and explicitly configured connect deadlines. Unknown sources remain unknown. No Rust diagnostic-string parsing, additional lookup/connection, request replay, primary fallback or automatic restart is added. Existing opaque query/authentication/online-count callers still receive `Failure`.

The connection driver retains its terminal error before completion, synchronized with a query observing a closed connection. The short synchronous mutex covers each driver poll and capture, never an async wait. Connection selection, cancellation, deadline placement, read-only session setup and shutdown retain their previous behavior. Debug/Display and `Error::source()` do not expose retained errors, target configuration, credentials or SQL. New fault variants deliberately have no `legacy_message()`.

**This is preparation, not public connection-error parity or another API activation.** The 24 DB-read handlers remain held as described in [db-read-validation.md](db-read-validation.md). The separate four date-pagination handlers also remain held. Five business handlers remain native; the observed registry has 438 entries and 433 TS callbacks. Counts are observations, never dispatch constants. The common API execution foundation is already Rust, but TS business callbacks, Fastify and other backend services remain.

## Executed baseline research

The disposable loopback PostgreSQL-wire fixture records real exceptions from installed Bun1.4.4/Node24.19.0, pg8.23.0, pg-pool3.14.0 and TypeORM1.1.0. Node is a comparison, **Bun is the production oracle**. Actual `PostgresQueryRunner.query` is invoked with a real pool; application initialization is bypassed to isolate checkout/query failures. This is driver-level baseline evidence, not a public HTTP or Rust parity test.

| Status | Observation | Evidence under `/workspace/pg-fault-parity` |
| --- | --- | --- |
| BASELINE | 18 modes × 3 interfaces × 2 runtimes, all 108 expected observations verified | `bun-1.4.4-baseline.json`, `node-24.19.0-baseline.json` |
| BASELINE | 12 subsequent-query recovery checks after query FIN, reset and server ERROR pass | `*-recovery.json` |
| BASELINE | Bun TLS handshake FIN repeats the same exception in 15/15 observations | `bun-1.4.4-tls-disconnect-repeat.json` |
| PASS | Combined baseline validation: 120/120 checks | `baseline-validation.json`, `baseline-report.txt` |

TypeORM awaits connection acquisition outside its query try/catch. Connect errors therefore retain their original exception class; errors after checkout become `QueryFailedError`. The legacy public API exposes the caught exception's name/message as `info.e.code/message`, plus a new correlation UUIDv4. A normal500 caused deliberately by a broken test connection is an expected fault response, not evidence of an existing product bug.

The TypeORM-only36-observation table is also tracked in [postgres-fault-baseline.csv](postgres-fault-baseline.csv); its status is BASELINE, with no Rust-side equivalence claim.

Material observations:

- This cloud resolver returns `EAI_AGAIN` for `.invalid`, not measured `ENOTFOUND`. Bun calls this exception `DNSException`; Node calls it `Error`. Do not generalize the observed resolver result to NXDOMAIN.
- Untrusted-certificate message: Bun reports `unable to verify the first certificate`; Node additionally suggests system-CA options. Verification stays enabled; a healthy trusted private-CA/SAN control passes.
- TLS handshake FIN: Bun reports `Error` / `ERR_SOCKET_CLOSED` / `Socket is closed`; Node reports `ECONNRESET` with a different message.
- Direct Client connect timeouts differ from Pool/TypeORM connect timeouts. Read-query timeout is measured separately after startup.
- Successful next queries establish limited recovery for the tested failures, not arbitrary transaction recovery.

`tokio-postgres::Config::connect` hides resolved address/attempt ordering and fine-grained DNS/TCP/startup stages; `postgres-openssl` hides its handshake-error wrapper/verification-result fields. Actual retained Rust sources alone therefore do not establish all observed Bun classes/messages. No guessed formatter or additional compatibility exception is shipped. A future transport boundary must retain the necessary context during the original operation, then prove public parity; a second diagnostic connection/lookup would change behavior.

Unmeasured baseline cases remain SKIPPED: actual NXDOMAIN/ENOTFOUND, expired/self-signed/partial-chain certificates, arbitrary malformed protocol/authentication, application initialization, all database/runtime versions and transaction recovery. The Rust read-only session SET stage also needs explicit compatibility treatment; pg sends its timeout options during startup.

## Current implementation validation

Linux x86_64, Bun1.4.4, Rust1.99.0; isolated PostgreSQL18/Redis7. Dedicated database `online-adapter` is used for live adapter tests, `test-misskey` for sequential backend E2E, and `api-layer-runtime`/Redis DB12 for public HTTP/benchmarks. Redis live fixtures use DB14 with unique keys and no FLUSHDB. No shared or production service is contacted. Commands, cwd, exit codes, elapsed time and sanitized logs are recorded in `/workspace/pg-fault-parity/results.jsonl`.

| Status | Check | Exit | Evidence |
| --- | --- | --- | --- |
| PASS | Fresh independent source review of all three Rust files; exact hashes verified before commit | source review | `review-source.json` |
| PASS | `cargo fmt --all -- --check` |0 | `logs/rust-fmt.log` |
| PASS | `cargo test --locked --workspace`:100 passed,4 default-ignored |0 | `logs/rust-workspace-tests.log` |
| PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings` |0 | `logs/rust-workspace-clippy.log` |
| PASS | Four default-ignored adapter tests explicitly run with dedicated config |0 | `logs/live-adapters.log` |
| PASS | Bun backend E2E build, including Bun→Cargo native build |0 | `logs/e2e-build.log` |
| PASS | Backend/test/federation TS typechecks |0 | `logs/{backend,test,federation}-types.log` |
| PASS | Related TS unit tests:52 passed across8files |0 | `logs/related-ts-unit.log` |
| PASS |30 paired public HTTP cases,20 child-loss503 requests without respawn, legacy TS DB-refusal parity;7 parent exits0, no orphans |0 | `http-summary.json`, `http-comparison.json`, `lifecycle-*.json` |
| FAIL | Initial full backend E2E:35files pass,2fail;1470passed,5failed,2skipped,20TODO |1 | `full-backend-e2e.json`, `logs/full-backend-e2e.log` |
| PASS | Corrected loopback config: previously failing2files,11tests |0 | `loopback-e2e-retry.json` |
| PASS | Corrected-config full backend E2E:37files,1475passed,2existing skips,20TODO,0failed |0 | `full-backend-e2e-retry.json`, `logs/full-backend-e2e-retry.log` |
| PASS | Full federation:9files,92passed,13existing skips; source/native hashes unchanged, project cleanup succeeds |0 | `federation/evidence.json`, `federation/full-tests.txt` |
| PASS | Foundation benchmark:34,000measured +3,400warmup, zero errors;8servers exit0, all owned processes collected,142provenance hashes verified |0 | `benchmark/{summary,output-verification}.json`, [CSV](postgres-fault-benchmark.csv) |
| PASS | Shipping/SPDX/locale checks on this unit |0 | `logs/shipping-source.log` |
| SKIPPED | Public connection-fault formatter and 24 native read-handler activations | held | Exact error contract remains unresolved |
| SKIPPED | Product UI rerun | scope | No frontend product code, API result schema or ordinary body changed; current HTTP/E2E checks are separate from historical UI evidence |
| SKIPPED | GitHub CodeRabbit | user instruction | Never invoked |

Eleven new adapter tests bring that package to19 default passing tests plus its explicit live test. They cover real refusal errno, startup/session/query phase differences, concrete driver-cause retention on disconnect, redaction/opaque APIs, stalled-startup deadline and socket disposal, failed-replica isolation, shutdown during startup, later independent reconnect and query-timeout cancellation cleanup. Wire fixtures use current-thread runtimes; they are not a cross-thread scheduling stress test. Workspace counts exclude doc-tests with no cases.

Public HTTP compares raw body digests and meaningful headers, with only documented Date, validated ping time and correlation UUIDv4 normalization. It includes method rejection, missing/null/wrong types, defaults, ordering, numeric boundaries and UTF-16/lone-surrogate cases. The DB proxy failure deliberately affects only this runtime server and tests the existing TS callback, not the held native candidate.

The first backend E2E configuration omitted loopback proxy bypasses, sending dedicated Webhook traffic toward the enforced external proxy. Five Webhook cases timed out. The same11cases pass after a separate retry config adds only `127.0.0.1`, `localhost` and the private test hostname to `proxyBypassHosts`; external requests retain the enforced proxy. The first failed report is retained; the complete corrected-config rerun is recorded separately.

Early harness/command failures remain FAIL records rather than being deleted: baseline listener/package-metadata harness corrections; a service-name typo before dedicated DB creation (retry succeeded); and a commit attempt naming the then-untracked new file (staging and ordinary retry succeeded). No failed command is counted as a successful check. A copied rebuildable Cargo cache was removed from the immutable runtime baseline to conserve disk; executable/bundle hashes remain recorded. Private config/CA/key files are not report or archive contents. For federation capacity, only this task's rebuildable host Cargo target was relocated to the separate temporary filesystem. A no-network, read-only marker probe verified the daemon sees a new empty private `/tmp` directory; a project-owned named bind volume uses it solely for container Cargo outputs. Host and container native artifacts are never mixed. With that cache off the32GB overlay and private nginx caches in256MiB tmpfs, an explicit5GiB preflight override replaces the conservative6GiB default for this run. Continuous436samples observed minimum free overlay2,083,336,192bytes and temporary filesystem6,116,311,040bytes. All phases pass without ENOSPC and cleanup removes only this project's resources. See `cargo-cache-relocation.json`, `federation/{capacity-adaptation,execution-decision,evidence}.json` and its disk monitor.

## Remaining business inventory and next boundary

The source-only AST catalog covers all433 remaining callbacks, with overlapping dependencies:405 PostgreSQL,293 Redis,383 auth/viewer,221 file/external,298 queue/notification/federation,317 JS-library,391 shared-service and286 configuration/meta observations. These are conservative source reachability groups, not runtime traces or exact migration estimates. The full catalog, callback/schema hashes and manual review are in `api-inventory.{json,summary.json,plan.md,sha256}` in the evidence root. No complete operation was established safe for unconditional activation with current error boundaries.

The recommended next foundation phase is retaining DNS/address-attempt/TLS context during the original PostgreSQL transport operation, then comparing public errors with the measured Bun oracle. Scope: `storage-postgres` connection/TLS internals and tests, followed by verified `api-runtime` business-error mapping/tests; no24handler activation until the comparison passes. Investigate an explicit transport/TLS adapter rather than guessing from Display or making a second lookup/connection. Preserve read/primary selection, actual timeout phases, cancellation, shutdown and recovery. Acceptance requires exact public status/body/meaningful headers after validated UUID/Date normalization, normal and fault E2E, cloud benchmark, fresh independent review, commit/push and final-SHA CI. Unsupported DNS/TLS source shapes remain a hold, not an implicit exception.

The smallest independent business unit after its own error gate is `reversi/cancel-match`, after typed Redis business-error/recovery parity. It awaits one ZREM: a specific opponent's key/member, or both ordinary/`noIrregularRules` entries on the general matching key. Count is ignored; successful return is undefined→204. There is no HDEL or pubsub in this operation. Preserve missing/null `userId` branching, exact key/member strings, auth admission, await/error timing and recovery; do not create a helper RPC or another crate. Relevant files are `api-runtime/src/redis.rs` and a complete internal operation, existing dispatch/contracts/tests, and `backend/src/server/api/endpoints/reversi/cancel-match.ts`. WRONGTYPE/transport/TLS/DNS/offline/max-retry behavior must be measured on the same installed ioredis before activation.

`server-info` also uses live OS/systeminformation behavior; `meta` includes fresh ads, clocks and possible proxy-account creation; avatar decorations require DB/cache refill and role visibility. Snapshot-only or disabled-flag-only ports would be incomplete. `hashtags/trend` preserves insertion order, parseInt/nonfinite JSON behavior, inclusive Redis ranges, local/UTC date methods and its existing cumulative time-step algorithm. `notifications/flush` detaches work; awaiting it would change semantics. Queue operations require BullMQ semantics and ordering rather than isolated Redis helper translations.

Across subsequent units preserve null versus omission/undefined→204, binary64 ranges and int8 rounding, BigInt serialization errors, separate date samples and ID state, UTF-16/lone surrogates, stable ordering/numeric object keys, exact ApiError versus generic `info.e`, detached versus awaited effects and internal callers. Only the three previously approved common-kernel corrections are exceptions. Frontend/slacc/schema/AGENTS changes and the four held date-pagination APIs are outside this unit.

## Issue and benchmark reporting

The confirmed existing frontend API-console Integer→null problem is filed as [Issue52](https://github.com/mothue/misskey/issues/52), with executed a630 evidence distinguished from source-only d875 evidence. No frontend fix is included; Issues49/50/51 remain user-owned. Other intentional fault responses are not mislabeled as TS bugs.

The previously measured, **held** 24-read candidate has its own [English table-only benchmark PR comment](https://github.com/mothue/misskey/pull/48#issuecomment-6362808360). It does not describe the current foundation commit. Colors use paired direction consistency and max(5%, baseline CV, candidate CV) as a conservative display heuristic, not a significance test or acceptance gate. The current foundation is measured separately: immutable e439 bundle/native versus698b15, both exactly one Bun parent plus one Rust child. Five native APIs use100warmup/1000measured/C8 in three AB/BA/AB pairs (30,000measured); two legacy smoke scenarios use a separate single pair (4,000measured, no CV claim). All34,000measured and3,400warmup requests succeed, with no HTTP/timeout/transport/validation errors. The unmodified common runner is used;142source/bundle/binary/config/runner/harness provenance hashes and dependency links remain stable before/after runs. All8parents exit0 and no owned Rust process remains. [postgres-fault-benchmark.csv](postgres-fault-benchmark.csv) contains35metric rows (p50/p95/p99, throughput and sampled Bun/Rust/combined RSS), population variance/stddev/CV, ranges and all paired deltas. Latency ends after full body receipt; throughput includes validation. RSS is a sampled /proc value, not peak or unique physical memory. There is no CPU isolation or production scalability claim.

`endpoints` throughput median decreases from5410to4218req/s (-22.0%), meeting the conservative red heuristic. The other9 displayed throughput/p95 deltas are white/small-or-noisy. No performance improvement gate is imposed and no benchmark framework optimization is included. The first output audit treated a TIME_WAIT bind as a listener; the retained failure is followed by no-owned-PID/no-listener/reuse-address checks that pass. New measurements identify their own baseline/candidate; incomplete attempts cannot become results. The user reported a display error in the first comment: Markdown removed the percent escape before TeX parsing, so `%` commented out the closing brace. A display-only update moves percent units to column headers and retains all48 values/color decisions. The original parse error reproduces and all48 corrected cells/color attributes pass local MathJax3.2.2 SVG validation. GitHub round-trip source is verified; actual client rendering is SKIPPED because the cloud browser receives the page but cannot load its client assets/CSS. No successful GitHub visual check is claimed. New benchmark comments use the corrected format.

## Persistence and final acceptance

Implementation is committed/pushed separately from validation documentation, with independent review before every commit. The implementation commit698b15 has no failed/pending hosted workflow:17PR-family workflows succeed and Storybook is skipped; the separate push Rust workflow also succeeds. After the documentation commit, verify its final-SHA hosted CI separately and save `ci-final.json`; source-commit CI must not substitute for that result. Final cleanup and archive creation are pending at this document snapshot: save `cleanup.json`, then generate `/workspace/pg-fault-parity/pg-fault-evidence.tar.gz` with its SHA256 and verified file manifest. Include sanitized command logs/results/reviews, baseline/HTTP/federation/benchmark data and harnesses. Private settings/credentials, CA/keys, browser state, native outputs and package/Cargo caches are excluded. The archive is workspace-only and needs explicit retention before disposal; no remote persistence is implied by local PASS. Tracked source/docs survive checkout. Ignored builds/config, fixture processes, package caches, Docker data and workspace-only archives are disposable and must not be assumed available in a future environment. PR title/body and AGENTS remain unchanged; no merge/close/history rewrite occurs.

API type generation is SKIPPED for this internal adapter unit: public meta/paramDef/res and misskey-js sources are unchanged. TS typechecks and builds above are current executions. No frontend/slacc/schema/AGENTS or docs/superpowers change is included.

CHANGELOG candidate (not edited): Retain typed PostgreSQL fault sources for experimental Rust backend compatibility research, without activating held business handlers.
