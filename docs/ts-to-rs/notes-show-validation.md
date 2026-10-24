# Native notes/show slice (cloud, 2026-10-24)

The native operation and its dependencies are implemented as internal modules under `api-runtime/src/business/notes/`. The first implementation commit `0b0e74a3c7d42c7e3747935726d58e8fec3efb0f` held activation; all six representative scenarios have now passed the unchanged-runner gate, and `notes/show` is activated. The observed registry now has 28 native business handlers and 410 TS callbacks out of 438 entries; these are observations, not dispatch constants. The TypeScript business path remains available for endpoints outside the native activation list; no failure-triggered fallback, replay or automatic restart is added.

The native operation owns root lookup and its three ordinary API errors, viewer visibility, nested replies/renotes, UserLite, public attachments, channels, polls/votes, legacy and buffered reactions, remote emoji and instance representations. It reuses the existing native authentication/role evaluator, PostgreSQL source pools and Redis infrastructure. A separate effective reactions Redis configuration is transferred during bootstrap. No business/helper callback enters TS. Frontend product code, slacc, schema, endpoint metadata and public SDK types are unchanged.

## Evidence so far

Baseline: `57fdf2198bb66b1fe761ef23fa983e3d9d1e6865`, detached source at `/workspace/notes-native/baseline-source`; freshly bundled production TypeScript with its own copied baseline native executable. This is the existing hybrid API infrastructure with a TS notes/show body, not the original pure-TS backend. Measured candidate: implementation commit `0b0e74a3c7d42c7e3747935726d58e8fec3efb0f` plus the two native activation entries. Its product source SHA256 is `29ea760f062972ddc25c004fc75aca211ee1defa83b4956b9b7b88b8932f590a`; native binary SHA256 `8ab90a56e5b9782adf35d7b692e7c87ef710e9143b5887a7e189ad29b274b9a5`. Exact baseline/candidate complete bundle hashes and all source files are retained in the benchmark provenance.

| Check | Result | Evidence under `/workspace/notes-native/` |
| --- | --- | --- |
| Rust workspace | PASS, 233 passed, 8 ignored excluded, exit 0 | `logs/rust-workspace.log` |
| Dedicated Redis integration | PASS, 13 passed including the new cache invalidation regression, exit 0 | `logs/native-redis-integration.log` |
| Live PostgreSQL metadata refresh | PASS, 1 explicit ignored test executed, exit 0 | `logs/postgres-meta-regression.log` |
| Rust fmt / all-target Clippy with warnings denied | PASS after three style corrections, exit 0 | `logs/rust-format.log`, `logs/rust-clippy-retry.log` |
| Backend source/test/federation type checks | PASS, exit 0 | `logs/backend-typecheck.log` |
| API generation and production build | PASS, exit 0; no generated public SDK source changes | `logs/api-types-and-production-build.log` |
| Final rebuilt public HTTP differential | PASS, 105 cases, zero differences, exit 0 | `http-comparison.json`, `logs/final-candidate-http.log` |
| Focused backend E2E | PASS, 2 files / 133 tests, exit 0; final strengthened nested-presence assertions covered by full run below | `logs/focused-notes-e2e-retry.log` |
| Complete backend E2E | PASS, 37 files / 1478 passed, 2 skipped and 20 todo excluded, exit 0, 379.27s | `logs/full-backend-e2e.log` |
| Backend unit | Initial full run FAIL due to missing test Meilisearch; dedicated CI-version service supplied and SearchService 32/32 retry PASS. Clean full retry PENDING | `logs/backend-unit.log`, `logs/backend-search-unit-retry.log` |
| Performance and activation | PASS, 6/6 scenario gates; 36,000 measured + 3,600 warmup + 6 first-public-request checks, zero errors, exit 0, 85.22s | `benchmark-results.json`, `activation-gate.json`, `logs/benchmark-final-measure.log` |
| Final pushed-commit CI | PENDING | Not yet published |

The 105 cases compare status, semantic JSON (including absent/null distinctions and array order), and content-type/cache-control/vary/access-control-allow-origin headers. Date and transport lengths are excluded. Coverage includes anonymous/owner/other/follower/root viewers, public/home/followers/specified notes, user/server visitor restrictions, shallow replies and detailed renotes, channels, duplicate/missing attachment references, Unicode, public/private badge order, repeated avatar decorations, remote instance/emoji, reaction normalization collisions, buffered reactions, votes, duplicate poll labels and millisecond date truncation. Reactions use dedicated Redis DB11 and a different prefix from main DB13, proving the operation does not silently read the main instance. The SQL/API fixtures are private and dedicated; no shared or production service is used.

## Performance evidence

Three paired AB/BA/AB runs, concurrency 8, 100 warmup and 1000 measured requests per scenario/build/pair. Six scenarios cover anonymous/authenticated public notes, nested quotes, polls, repeated/missing attachments and follower-only notes. Each measured response is checked against the exact semantic TS baseline plus stable headers. Every scenario has median throughput at least baseline and median p95 no worse; measured throughput increases 208.0–283.2% and p95 decreases 65.6–72.0%. This is a small dedicated fixture on a shared cloud host, not a production scaling claim. Startup and one first public request per process are recorded separately; these are not a general cold-cache distribution. Sampled TS/Rust RSS is not peak or unique physical memory.

[Tracked measurements](notes-show-benchmark.csv) retain 42 metric rows for p50/p95/p99, throughput, sampled memory, run ranges/CV and paired deltas. The existing benchmark runner is unchanged. Source, complete bundles, native executables, fixture/config and runner/harness fingerprints are stable across the accepted run. All six server groups exit 0, collect their Rust child, and release the public port. No build/test/federation job overlaps measurement.

The first measurement attempt was rejected by the snapshot guard before any HTTP start: the final production rebuild changed the TS bundle after an older readiness run. That attempt has zero measured requests and is archived under `attempt-old-production-dry-run`. Prepare and readiness were repeated on the exact final artifacts before the accepted run.

## Corrections and limits

Independent review caught an anonymous-view regression: the packed UserLite omits its false signin flag, while the initial note visibility check required a boolean. It now handles the packed optional field explicitly, retaining strict DB decoding; a normal anonymous public-note unit covers the case. The new cache also guards in-flight reads/fills with invalidation generations and serializes Redis publication/invalidation. An explicit dedicated Redis regression proves an older fill is rejected. Replica-eligible reads retain the existing TypeORM source selection; this does not promise stronger database replication consistency than TS.

The first fixture seed expected poll voting to return 200; the existing API correctly returns 204. The fixture assertion was corrected. An added nested E2E initially compared a creation-time nested note to a later read, although creation increments its parent's counters after capturing that nested representation. Its intended detail-level contract is now asserted directly; the separate stable-data differential compares complete nested responses. These were validation-fixture assumptions, not TS product bug findings.

Infrastructure error details remain best effort under [compatibility-exceptions.md](compatibility-exceptions.md). All ordinary auth/security/status/error-code/response contracts remain strict. All six measured median throughput/p95 gates pass; no dependency exception is used. Existing two statistics performance holds and four date-pagination holds remain unchanged.

Commands, cwd, exit codes and sanitized logs are recorded in `/workspace/notes-native/results.jsonl`; independent batch review in `/workspace/notes-native/review.json`. Private evidence needs workspace retention. Rebuildable binaries, Cargo targets and test DB/Redis state can be cleaned when capacity is needed; source, logs, measurements and tracked records are retained.
