# First Rust port validation

This document preserves the original three-handler validation. The later cloud `endpoints` unit, including its own reruns and limits, is recorded in [endpoints-validation.md](endpoints-validation.md). Its results must not be inferred from the local runs below.

Baseline: `d875b7e3c737b7f9f75331fffb691f6a2b97f055`. Candidate: `feat/ts-to-rs`, validated as an uncommitted working tree. After validation the user authorized splitting the changes into commits and pushing the branch. The authoritative scope and contract are [PORTS.md](PORTS.md).

## Evidence and reproduction

Raw local artifacts are retained under ignored `built/ts-to-rs/`; the baseline source copy is `built/ts-to-rs/baseline`. Fixtures and the HTTP capture/benchmark programs are versionable source files. Logs are execution evidence for this checkout, not a guarantee about other hosts.

| Check | Result | Evidence |
| --- | --- | --- |
| Public HTTP equivalence | PASS, 474 requests including all 438 endpoint descriptions | `baseline-http-expanded.json`, `candidate-http-expanded.json`; [behavior.csv](behavior.csv), [baseline details](baseline-evidence.md) |
| Rust tests | PASS, 3 core and 7 bridge/process tests | `rust-final-test.log` |
| Rust formatting and Clippy | PASS, locked workspace/all-targets, warnings denied | `rust-final-fmt.log`, `rust-final-clippy.log` |
| Backend TypeScript | PASS | `task3-tsc.log`; baseline also passes in `baseline-typecheck.log` |
| Backend unit suite | PASS, 70 files, 891 passed / 2 skipped | `candidate-backend-unit-isolated.log`; baseline 68 files, 879 passed / 2 skipped in `baseline-backend-unit.log` |
| Final transport/shutdown tests | PASS, 17 tests in 2 files; isolated 10-second startup timeout also passed | `task3-final-fault-tests.log`, `task3-startup-timeout-final.log` |
| Other workspace unit suites | PASS: frontend 211, misskey-js 14, diagnostics shared/backend/frontend 63/20/17, changelog checker 17 | `candidate-workspace-tests-serial.log` (backend result superseded by the isolated run above) |
| Build helper and benchmark/harness tests | PASS, 2 build-helper tests and 22 harness/runner tests | task reports and `task1-tests.log` |
| Complete root build | PASS | `candidate-build.log` |
| API type generation | PASS, no changes under `packages/misskey-js/src/autogen/` | `api-generation.log` and final Git diff |
| Federation E2E | PASS, all 9 files, 92 passed / 13 existing skips | [Federation conditions and evidence](federation-evidence.md) |
| Complete backend E2E | BASELINE BLOCKER, attempted but process exited before a complete summary | [Exact baseline reproduction and coverage limits](backend-e2e-evidence.md) |
| Backend E2E per-file audit | 28 of 29 files PASS; `move.ts` BASELINE blocker. Completed files: 1,273 passed / 2 skipped / 20 todo | `e2e-per-file/coverage-summary.json` and [per-file table](backend-e2e-evidence.md) |
| Scoped backend timelines E2E | PASS, 378 tests | `candidate-timelines-diagnostic.log` |
| Browser API console | PASS, real production server and IAB | `browser-api-console.png`; details below |
| Actual fault and shutdown behavior | PASS, selected API/health 503 isolation, normal shutdown, parent SIGKILL/EOF and cluster cleanup | [Runtime evidence](runtime-evidence.md) |
| Development restart | PASS, 7 scenarios | `.benchmark/watch-check/result.json` and `watch.log` |
| Benchmark infrastructure and paired runs | PASS, 6,000 smoke + 18,000 paired measured requests, zero errors | [Full metrics and overhead](benchmark-evidence.md), [machine-readable results](benchmark-results.json) |
| Source archive | PASS | `source-tarball-contents.txt`; Cargo sources, lock and `.cargo/config.toml` present, native build artifacts/config/secrets excluded |
| Shipping checks | PASS, changed backend file lint, SPDX, locale safety | `shipping-final.log` |
| Independent source reviews | PASS after fixing HTTP/WebSocket shutdown ordering | `.superpowers/sdd/PORTS/review-*.md`, `final-review.md` |
| CodeRabbit | FAILED TO COMPLETE, connection failure on both attempts | `coderabbit-review.ndjson`, `coderabbit-retry.ndjson`; not counted as a successful review |

The complete backend E2E suite is not recorded as passing: its HTTP-response-disposal crash reproduces on the exact baseline. Separate per-file runs completed the 29-file inventory without suppressing that crash: 28 files passed, while `move.ts` crashed after its first three local-alias tests. Its remaining cases are unverified. There were no additional failures or per-file timeouts. This document does not treat incomplete coverage as a pass.

For these very small handlers, loopback RPC increased median p50 by about 0.16 ms (median paired change +57–61%) and reduced throughput by 31–33%. There is no performance-improvement claim. The benchmark report preserves each run, p95/p99, spread, TS/Rust RSS and separate startup/build timing qualifications.

## Browser verification

The unchanged production frontend was served at `http://127.0.0.1:61912/api-console`, using the dedicated benchmark database. Selecting `test` generated `required`, `string`, `default`, `nullableDefault` and `id` fields through `/api/endpoint`. Sending `{"required":true,"extra":{"message":"Rust経由","items":[1,null,false]}}` returned the object, retained the nested extra property and supplied the existing `hello` defaults. Selecting `ping` generated `{}` and returned an integer `pong` timestamp. A screenshot of the final ping result is saved at `built/ts-to-rs/browser-api-console.png`.

The fixture account's temporary local login password was restored to its original value after testing. The browser tab was closed before measurement. No frontend product source was changed. The initial launch used an inappropriate storage override and could not find frontend assets; the accepted launch used the normal production asset path.

## Review and distribution boundaries

The whole-change reviewer had no implementation conversation context and reviewed the final diff, Rust/TS boundary, lifecycle, Docker/CI and harness. The initial TS review found that an unresponsive upgraded WebSocket could delay HTTP closure; the fix closes HTTP concurrently, destroys remaining sockets at the total five-second deadline and always stops Rust. A real Fastify/WebSocket regression test and independent re-review passed.

API-specific review confirmed the existing registrations and unchanged `meta`, `paramDef` and response schemas. Only the three callback bodies and DI dependencies changed. Generated public types remain unchanged. The common unavailable error is explicitly a server error with HTTP 503. No entity/migration, frontend product, slacc source or locale changes are included. A production Docker image was source-reviewed; the federation builder actually compiled and ran the Rust binary on Linux.

The source archive was generated with `bun run build:tarball` and inspected. It contains the new workspace, Cargo.lock, pinned toolchain and build configuration, and excludes `packages/backend-rust/built`, local configuration, benchmark storage and dependency directories.

Changelog candidate (not applied): Backend: run ping, test and endpoint handlers in a required Rust subprocess while preserving the public API contract.

## Discarded attempts and environment isolation

Initial baseline E2E and candidate unit tests mistakenly overlapped the same local test DB. PostgreSQL duplicate catalog errors from those runs are invalidated evidence, not a baseline defect or a migration regression. Subsequent DB suites run sequentially. A workspace retry also hit one setup timeout. The later isolated full backend unit suite passed all 891 tests.

Federation Docker mounts produced an empty default compiled configuration on the host. Host checks therefore explicitly set `MISSKEY_CONFIG_JSON=$PWD/built/ts-to-rs/host-test-config.json`. The product configuration loader was not modified to hide this test-environment issue. After the dedicated federation containers were stopped, only that zero-byte generated file was removed. Federation-specific subnet/volume/rate-limit adjustments, discarded diagnostic runs, and the clean accepted run are recorded in its separate report.

Useful commands from the repository root:

```sh
bun run build
bun run generate:api-types
MISSKEY_CONFIG_JSON="$PWD/built/ts-to-rs/host-test-config.json" bun run --filter backend test
MISSKEY_CONFIG_JSON="$PWD/built/ts-to-rs/host-test-config.json" bun run --filter backend test:e2e
bun scripts/benchmarks/ports/capture.ts http://127.0.0.1:61912 built/ts-to-rs/candidate-http-expanded.json built/ts-to-rs/baseline-http-expanded.json
bun scripts/benchmarks/ports/paired.ts measure
node scripts/checks/shipping.mjs --base d875b7e3c737b7f9f75331fffb691f6a2b97f055
```

Run Cargo commands inside `packages/backend-rust`: `cargo test --locked --workspace`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo fmt --all -- --check`. Backend tests require the dedicated test dependencies and `.config/test.yml`. Do not run two DB-resetting suites against the same test database concurrently. Benchmark servers must exclusively own port 61912, and timed measurements must run without concurrent tests/builds.
