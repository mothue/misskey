# Database-read API candidate: cloud validation and hold

Date: 2026-10-23. Repository `mothue/misskey`, branch `feat/ts-to-rs`, starting clean HEAD `a6306ed7617a7c10873ec2f76699cd0ccf4d9c9b`. The existing draft PR is [#48](https://github.com/mothue/misskey/pull/48). Its title/body are user-owned; this work does not edit them. No GitHub CodeRabbit review is requested.

## Acceptance decision

**The 24 new native business handlers are held, not accepted.** They were implemented and pushed as candidate commit [9d823f665f](https://github.com/mothue/misskey/commit/9d823f665ffaf3f76622b5dd0374f7083047d4b4), and passed the ordinary/validation comparisons. A later live database fault test found a real public error-contract regression. A subsequent normal commit restores public dispatch and all 24 TS bodies; no published history is rewritten and no error-triggered TS retry/fallback is added.

The final active native business inventory therefore remains the previous five handlers (`ping`, `test`, `endpoint`, `endpoints`, `get-online-users-count`). The observed registry has 438 entries and 433 TS business callbacks. These are observations, never dispatch constants. The previously migrated common API infrastructure still runs in Rust; Fastify retains the public socket/non-API/WebSocket ownership.

The safe hold follows the user's instruction to defer newly discovered behavior concerns and collect decisions at the end. Success of ordinary E2E, UI or benchmarks does not close the fault regression. The tested typed PostgreSQL error adapter and 19 behavioral E2E tests are retained as preparation for a future migration.

## Candidate and separate date-pagination hold

The candidate groups were:

- Discovery7: `emoji`, `emojis`, `hashtags/list`, `hashtags/search`, `hashtags/show`, `username/available`, `retention`.
- Registry/app10: `i/registry/get`, `get-all`, `get-detail`, `keys`, `keys-with-type`, `scopes-with-domain`; `app/show`, `my/apps`, `i/apps`, `i/authorized-apps`.
- Role/admin7: `roles/list`, `roles/show`, `admin/roles/list`, `admin/roles/show`, `admin/relays/list`, `admin/get-index-stats`, `admin/get-table-stats`.

Four earlier candidates (`admin/emoji/list`, `admin/emoji/list-remote`, `admin/ad/list`, `admin/announcements/list`) were excluded before acceptance at the user's explicit choice. Separate TS/Rust ID generators can produce a different comparison suffix in the same millisecond and change date-pagination inclusion. Their TS sources remain byte-identical to a6306ed; no new ID generator/shared state is shipped. This is a migration compatibility concern, not evidence that those original APIs are broken.

## Confirmed fault regression

The same isolated test is run against an immutable a6306ed bundle/Rust binary and the native candidate. Each server starts successfully through a dedicated loopback PostgreSQL TCP proxy; a healthy `hashtags/search` returns200. Closing only that proxy's listener and existing sockets makes the next native business query reconnect to a refused test port. Backend E2E and federation services are unaffected.

| Result | Version | Public response |
| --- | --- | --- |
| BASELINE | a6306ed |500 `INTERNAL_ERROR`, including `info.e.message = "connect ECONNREFUSED 127.0.0.1:54313"`, `info.e.code = "Error"`, fresh UUIDv4 `info.e.id` |
| FAIL | native candidate |500 with the same outer error but **no `info.e`** |

The old500 is the expected response to the deliberate DB fault, not proof of an existing product bug. The missing details are a new business-path regression. All24 handlers use `rows → Database.query_typed → QueryError::Unavailable`, so reverting only `hashtags/search` would leave the unsafe shared boundary. An independent reviewer confirmed this scope.

Server-reported PostgreSQL errors (negative/overflowing offsets, NUL strings) and query timeouts were already repaired/tested separately. Connection setup, DNS, TLS and non-server I/O errors are still collapsed by the existing connection layer. Rust driver Display strings or guessed Node messages are not accepted as compatibility evidence. No additional compatibility exception has been approved.

Evidence: `/workspace/db-read-ports/db-fault.json`, `db-fault.py`, `logs/db-connection-fault.log`, and `decisions.json`. The fault comparison command exited0 because it captured both responses; its **semantic comparison is FAIL**, not PASS. Final safety verification reruns the public fault test with restored TS dispatch and records that result separately.

## Executed candidate validation

Evidence root `/workspace/db-read-ports`; every command's argv, cwd, exit code, elapsed time and sanitized log location are recorded in `results.jsonl`. Private settings/credentials are not report contents. Commands below use Bun1.4.4/Rust1.99.0 on Linux x86_64, dedicated PostgreSQL18/Redis7, and the explicit config paths documented below.

| Status | Candidate check | Exit | Evidence |
| --- | --- | --- | --- |
| PASS | Cargo workspace tests before last missing-app correction:110passed,4default-ignored; final affected runtime tests after correction separately pass |0 | `logs/rust-workspace.log`, `logs/rust-native-unit-reviewed.log` |
| PASS | Cargo workspace clippy all-targets `-D warnings`, plus affected runtime rerun after correction |0 | `logs/rust-clippy.log`, `logs/rust-native-clippy.log` |
| PASS | PostgreSQL adapter8unit tests and explicit dedicated ignored live test |0 | `logs/postgres-unit.log`, `logs/postgres-live.log` |
| PASS | Native Bun→Cargo build |0 | `logs/native-build-final.log` |
| PASS | API type generation/build; no public autogen source change |0 | `logs/api-types.log` |
| PASS | Backend, test and federation TS typechecks |0 | `logs/{backend,test,federation}-types.log` |
| PASS | Related TS unit tests54 across9files (includes the unshipped NativeEndpoint prototype tests) |0 | `logs/related-unit.log` |
| PASS | Paired public HTTP:138cases, zero differences after documented volatile normalizations |0 | `baseline-http.json`, `candidate-http.json`, `http-comparison.json` |
| PASS | Added focused E2E:19passed |0 | `focused-e2e.json` |
| PASS | Full backend E2E:37files,1475passed,2existing skips,20TODO,0failed |0 | `full-backend-e2e.json`, `logs/full-backend-e2e.log` |
| PASS | Full federation:9files,92passed,13existing skips,0failed; Linux-native outputs built inside container |0 | `federation/evidence.json`, `federation/full-tests.txt` |
| PASS | Product UI baseline/candidate login/API-console: ping,test,emojis,roles/list,registry keys,my/apps;6same-origin API checks; no page errors |0 | `browser-{baseline,candidate}.json`, `logs/browser-ui-pair-success-inputs.log` |
| PASS | Rust child termination: all24API return503, no restart, TS exits0 and child collected |0 | `lifecycle.json`, `logs/candidate-lifecycle.log` |
| FAIL | Deliberate connection-refused behavior comparison |capture0 | `db-fault.json` |
| FAIL | Independent native acceptance review: later common connection-fault regression remains open |source/fault review | `review-a-native-final.json`, `decisions.json`, `db-fault.json` |
| PASS | Independent behavioral E2E and benchmark harness review |source review | `review-b-compatibility-tests.json`, `benchmark-review.json` |
| SKIPPED | GitHub CodeRabbit |not invoked | User explicitly forbids it |

The local historical `move.ts` unhandled-response abort is not reproduced by this full cloud run: its cloud cases pass. This does not disprove the old local evidence or establish its cause. Issues49/50/51 remain user-owned and unchanged.

Ordinary HTTP normalization excludes Date; validates/replaces only the generated `error.info.e.id` UUID; compares exact credential-containing body digests while redacting readable fixture credentials; and validates ordered table-stat shape while normalizing live count/storage values. The table-stat E2E additionally uses SQL as its value oracle. The fault case is separate and is not hidden by those normalizations.

The UI uses existing product Vite assets and localization, not a fresh full frontend build. Two signin-flow steps require the existing production one-second minimum interval. API-console's generated optional integer fields need explicit valid pagination values. Earlier automation attempts without these preparations failed against the baseline; they are preserved as test-harness failures and superseded by the successful paired run, without changing frontend product behavior.

## Type and boundary evidence

The candidate kept complete operations as internal modules, using the existing PostgreSQL adapter; it did not create helper-by-helper RPCs or a low-value extra crate. The TS public metadata, input schemas and SDK types stayed unchanged. Final public business logic is restored to TS pending the fault contract.

Covered behavior includes omitted vs null flags/registry values, all JSON kinds, integer-index enumeration and `__proto__` assignment, duplicate apps/order, owner-only app secret visibility, user/app token registry isolation, raw PostgreSQL int8→JS Number rounding, offset errors, Date millisecond rounding and pre-epoch timestamps, six existing ID date parsers, lowercase/NFKC/SQL LIKE escaping, UTF-16 lone surrogates, and exact known/legacy error shapes with fresh correlation IDs.

Bun1.4.4 uses Unicode17. Candidate Unicode18 normalization data produced68 scalar mismatches; pinning unicode-normalization0.1.25 to Unicode17 yielded zero differences over all1,112,064 Unicode scalars. This pin/helpers belong to the held candidate; final active code is unchanged. An IDNA experiment was only for the four excluded date APIs and was removed before candidate acceptance tests; it is not a shipped IDNA migration. See `unicode-validation.json`.

Schema-invalid raw DB values such as Date infinity/out-of-range dates or null elements in a text[] policy setting remain unverified/error-shape concerns, not diagnosed regressions. They are not silently declared compatible. Do not broaden normalizers to hide failures.

## Reproduction

Dedicated Compose project `cloud-read-ports` exposes PostgreSQL127.0.0.1:54312 and Redis127.0.0.1:56312. Backend E2E uses `test-misskey` and port61812. HTTP/UI/benchmark uses `api-layer-runtime`, RedisDB12 and port61912. Adapter tests use `online-adapter`. Distinct effective hostnames isolate pubsub; database-reset suites are sequential. No shared/production service is connected.

From `packages/backend-rust`, use `.cargo/config.toml` target settings:

```sh
cargo fmt --all -- --check
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --workspace
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo clippy --locked --workspace --all-targets -- -D warnings
```

From repo root, with PATH containing the specified Bun/Cargo:

```sh
MISSKEY_CONFIG_YML=/workspace/db-read-ports/test-config.json \
MISSKEY_CONFIG_JSON=/workspace/db-read-ports/api-generation-config.json \
  bun run generate:api-types
node scripts/checks/shipping.mjs --base a6306ed7617a7c10873ec2f76699cd0ccf4d9c9b
```

From `packages/backend`, after generation:

```sh
bun run --bun tsc --noEmit -p tsconfig.json
bun run --bun tsc --noEmit -p test/tsconfig.json
bun run --bun tsc --noEmit -p test-federation/tsconfig.json
NODE_ENV=test MISSKEY_CONFIG_JSON=/workspace/db-read-ports/test-proxy-config.json \
  bun --preload /workspace/db-read-ports/localhost-dns-preload.mjs \
  ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts \
  --execArgv=--preload --execArgv=/workspace/db-read-ports/localhost-dns-preload.mjs \
  --reporter=verbose --reporter=json \
  --outputFile=/workspace/db-read-ports/full-backend-e2e.json
```

The preload resolves localhost only for the dedicated test Webhook fixture; proxy/CA policy otherwise stays enforced. Private JSON configs substitute for the pre-existing empty default.yml and must be recreated explicitly in a clean workspace. Federation uses separate fresh DB/Redis/data volumes in `cloud-read-ports-fed`, read-only source mounts and private writable config/storage overlays, and external Linux-native cache volumes. The full run verifies1,921source/runtime hashes, host native output hashes and removes services/data/networks afterwards.

## Candidate cloud benchmarks

PASS for measurement completeness, **not functional acceptance**:24 APIs ×3baseline/candidate pairs (AB/BA/AB),100warmup and1000measured requests per API/side, concurrency8. All144,000measured and14,400warmup requests succeeded with zero status, validation, timeout or transport errors. Existing `scripts/benchmarks/backend/runner.ts` phase/summary is unmodified. Every measured side exits0 and both processes are collected. Source, bundle, binary, config and fixture snapshots remain unchanged across the measured run. Builds/tests/federation/UI do not overlap the quiet measurement window.

The baseline is a6306ed; the candidate is9d823f665f plus the uncommitted TS marker/removal patch preserved in `candidate-integration.patch`. These are results for the **held candidate**, not a speedup claim for the final restored branch. No performance optimization or improvement threshold is used. The aggregate equal-weight geometric mean of endpoint median throughput ratios is0.899×;13endpoints improve and11regress. This is not traffic-weighted. Table/index statistics regress substantially and must not be hidden behind an aggregate.

Medians of three runs are shown below. Full p50/p95/p99, throughput, TS/Rust/combined sampled RSS, min/max and CV are preserved in the credential-free [tracked CSV](db-read-benchmark.csv). Population standard deviation is additionally preserved in `benchmark-summary.json`; raw `benchmark-results.json` and `benchmark-compact.json` are in the evidence bundle.

| API | Baseline p95 ms | Candidate p95 ms | p95 delta | Baseline req/s | Candidate req/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| `admin/get-index-stats` | 50.207 | 91.079 | +81.4% | 232 | 98 |
| `admin/get-table-stats` | 14.106 | 27.218 | +93.0% | 886 | 325 |
| `admin/relays/list` | 5.481 | 3.884 | -29.1% | 2197 | 3014 |
| `admin/roles/list` | 13.646 | 16.560 | +21.4% | 981 | 623 |
| `admin/roles/show` | 9.117 | 8.117 | -11.0% | 1307 | 1322 |
| `app/show` | 5.183 | 4.252 | -18.0% | 2272 | 2873 |
| `emoji` | 5.634 | 4.320 | -23.3% | 2323 | 2363 |
| `emojis` | 4.444 | 4.877 | +9.8% | 2355 | 2397 |
| `hashtags/list` | 5.985 | 5.290 | -11.6% | 2187 | 2141 |
| `hashtags/search` | 5.390 | 5.949 | +10.4% | 2248 | 2198 |
| `hashtags/show` | 5.481 | 4.740 | -13.5% | 2301 | 2485 |
| `i/apps` | 7.574 | 6.089 | -19.6% | 1609 | 1742 |
| `i/authorized-apps` | 15.366 | 20.032 | +30.4% | 820 | 487 |
| `i/registry/get` | 5.548 | 3.945 | -28.9% | 2033 | 2574 |
| `i/registry/get-all` | 5.104 | 4.445 | -12.9% | 2063 | 2285 |
| `i/registry/get-detail` | 5.069 | 4.275 | -15.7% | 2340 | 2385 |
| `i/registry/keys` | 6.207 | 4.254 | -31.5% | 2303 | 2401 |
| `i/registry/keys-with-type` | 5.787 | 4.690 | -19.0% | 1898 | 2129 |
| `i/registry/scopes-with-domain` | 6.363 | 5.156 | -19.0% | 2159 | 2229 |
| `my/apps` | 7.007 | 9.984 | +42.5% | 1582 | 1019 |
| `retention` | 8.287 | 7.338 | -11.5% | 1650 | 1521 |
| `roles/list` | 8.100 | 8.362 | +3.2% | 1424 | 1250 |
| `roles/show` | 8.834 | 8.492 | -3.9% | 1404 | 1234 |
| `username/available` | 7.835 | 6.946 | -11.3% | 1872 | 1526 |

Startup is separate from request latency. Sampled endpoint-median RSS ranges are baseline TS406.1–573.0MiB plus Rust20.8–22.9MiB, candidate TS293.6–383.0MiB plus Rust19.0–19.1MiB. Samples are Linux `/proc` VmRSS after a scenario, not peak or unique physical memory. Three runs on small fixtures cannot establish production scalability: throughput CV reaches17.1%, p99 CV40.0%; there is no CPU isolation. Latency runs through complete body receipt and excludes validation; throughput includes validation, matching the existing runner.

Header validation excludes Date/Connection/Keep-Alive because the original urllib reference closes its connection and Bun fetch reuses it. CORS/cache/content-type/content-length remain strict; dynamic table-stat values/content-length have the documented SQL-oracle exception. All other successful response bodies are checked by their exact digest. Explicit fixture-only UPSERTs reset token timestamps before each side; no reset is run against another test DB and no shared service is used.

The first dry-run stopped before HTTP because UI setup had added one browser-owned registry preference. This remains FAIL/preflight evidence. After proving that row's owner differed from root/user/other fixture users, exactly that disposable browser preference was removed. The next dry-run validated96requests and matched the exact measured source/build/config/fixture snapshot. Source/summary review used exact harness SHA256 `e04f99d16a8d9d4f19efb804da4e8263f5316d31515e83b7d96c978d928a96fb`.

## Final safety-restored state

The safe final dispatch is committed as `54cebf82d6`: all1,000 backend/src files match a630 byte-for-byte, including the24candidate endpoints and the4earlier date-pagination holds. Relative to the starting checkout, production changes are only the two typed PostgreSQL adapter files. The19new behavioral E2E tests remain. The native candidate is preserved in commit history and the integration patch, not enabled behind an undocumented runtime switch.

| Status | Final-state check | Exit | Evidence |
| --- | --- | --- | --- |
| PASS | Cargo fmt/clippy(workspace,all targets),89default tests;4live ignored adapter tests explicitly run and all pass |0 | `logs/safety-{fmt,rust-tests,rust-clippy,live-adapters}.log` |
| PASS | API type generation/backend build and backend/test/federation typechecks; no public autogen source diff |0 | `logs/safety-{api-types,backend-types,test-types,federation-types}.log` |
| PASS | Public HTTP138cases plus the original connection-refused comparison now match a630 |0 | `safety-http-comparison.json`, `safety-db-fault.json` |
| PASS | Full backend E2E37files:1475passed,2existing skips,20TODO,0failed |0 | `safety-full-backend-e2e.json`, `logs/safety-full-backend-e2e.log` |
| FAIL | First final-state federation attempt:9files failed,4tests failed,101skipped; nginx could not create cache due ENOSPC |1 | `federation/evidence-safety-attempt1.json` |
| PASS | Final-state federation retry:9files,92passed,13existing skips,0failed; native build/tester/cleanup exit0,1,911source/build hashes unchanged |0 | `federation/evidence-safety-retry.json` |
| PASS | Independent safety-restore and19test review, hashes checked before commit |source review | `review-a-safety-restore.json`, `review-b-compatibility-tests.json` |

The federation failure is a proven cloud capacity problem, not evidence of a product regression: the running builder/apps/nginx/tester exhausted the32GB overlay (0available; inodes remained available). Idle free-space measurements missed that peak. Only the private nginx cache was moved to a256MiB tmpfs and this task's rebuildable708MiB host Cargo incremental cache was removed. Source/native executable outputs stayed unchanged; other projects were not pruned. The complete rerun passed and the failed attempt remains distinct. Live Redis fixtures use explicit DB14, unique keys and no FLUSHDB; live PostgreSQL uses the separate online-adapter database.

Final related TS unit tests also pass:52tests across8files, exit0, `logs/safety-related-unit.log`. Candidate-only NativeEndpoint prototype tests are not counted in that final total.

## Resumption conditions and next decision

The recommended next unit is **connection-fault compatibility in the existing PostgreSQL adapter**, before activating another business handler. Retain typed source errors at the connection/query phase where they occur; do not attempt to recover information after it becomes opaque `Failure`. Scope: `storage-postgres` internals/tests and the future `api-runtime` business-error mapping/tests. Keep successful SQL target selection, cancellation, deadlines and shutdown unchanged; preserve the existing opaque API for migrated authentication callers. Frontend/slacc/schema changes, the24activations, the4date APIs and benchmark-framework changes remain outside that preparatory unit.

Use the immutable TS baseline and the same dedicated fixtures to compare connection refusal, name-resolution failure, timeout, TLS trust/hostname failure, mid-connect/mid-query disconnect and recovery; retain actual message/class, absent-vs-null `info.e`, outer API identifiers and generated UUIDv4 semantics. No connection credentials or raw private config may enter logs. A Rust driver string is not automatically the Node contract. Unknown/unmatched failure shapes remain a blocker instead of becoming an implicit behavior exception.

Acceptance requires unchanged ordinary behavior and exact public fault status/body/meaningful-header parity after documented UUID/Date normalization, direct connection-layer and public HTTP tests, independent fresh-context review, full required checks, normal commit/push and final-SHA CI confirmation. Only then propose reactivating the24native bodies as a subsequent approved unit. If preserving a supported failure shape is infeasible, collect that exact difference and ask for an explicit contract decision at the end; the current three approved exceptions do not cover DB errors.

The separate4date-pagination APIs remain held even if connection faults are solved. Their same-millisecond TS/Rust ID-state boundary needs its own preservation design or explicit user decision; do not couple their reactivation to the24read APIs. Schema-invalid raw DB date/policy values need an explicit testable scope before claiming exhaustive compatibility.

## Git, CI and persistence

Commits are reviewed independently before creation, hooks are not bypassed, and all pushes are ordinary pushes to `feat/ts-to-rs`. `d5ca2e9804` retains typed PostgreSQL query error support; `9d823f665f` preserves the held native implementation; `54cebf82d6` restores existing production dispatch and retains19behavioral E2E tests. This documentation/data follows in a separate reviewed commit. Author/committer email uses the repository's existing GitHub noreply identity; GitHub resolves new commits to `mothue`. No published history or persistent git identity config is rewritten.

Hosted CI must be checked on the final pushed SHA and saved separately in `ci-final.json`; the tracked local validation does not predict hosted-CI results. The existing draft PR is reused and attached without editing its title/body, requesting GitHub CodeRabbit, merging or closing it.

The credential-free archive is `/workspace/db-read-ports/db-read-evidence.tar.gz` with SHA256 and a file manifest. It contains sanitized logs/reviews/results, benchmark data, source/binary hashes and the held integration patch, excluding private settings/credentials/browser state/keys, native outputs and docs/superpowers. `cleanup.json` records test process, private service/data and credential-file cleanup. The archive is a workspace artifact and needs explicit retention before workspace disposal; no remote persistence is implied. Tracked commits/docs/CSV survive checkout. Ignored builds, caches, Docker objects and private fixtures are disposable and must not be assumed in a later workspace.


CHANGELOG candidate (not edited): Add compatibility tests and typed PostgreSQL query errors for the experimental Rust API migration; hold DB-read handler activation pending connection-error parity.
