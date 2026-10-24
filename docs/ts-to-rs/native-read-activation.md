# Measured native read activation (cloud,2026-10-24)

The user's speed-first policy releases the former24read infrastructure-fault hold. Approved differences, including legacy `info.e`, are collected only in [compatibility-exceptions.md](compatibility-exceptions.md). Ordinary auth/authorization/roles/scopes, rate limits, validation, status/error codes and response shapes remain strict; security fails closed.

## Selected complete native slices

Activate22: `admin/relays/list`, `admin/roles/list`, `admin/roles/show`, `app/show`, `emoji`, `emojis`, `hashtags/list`, `hashtags/search`, `hashtags/show`, `i/apps`, `i/authorized-apps`, `i/registry/get`, `i/registry/get-all`, `i/registry/get-detail`, `i/registry/keys`, `i/registry/keys-with-type`, `i/registry/scopes-with-domain`, `my/apps`, `retention`, `roles/list`, `roles/show`, `username/available`.

Keep on TS for performance: `admin/get-index-stats`, `admin/get-table-stats`. Both enter the existing TS path before any native business work; no error-triggered fallback or SQL replay is added. Four date-pagination APIs retain their separate ordinary response-membership hold. All22activated bodies use native authentication, PostgreSQL queries, existing required Redis behavior and response encoding, without invoking a TS business callback. Dispatch names and the TS callback rejection guard agree. Native functions are internal modules, not helper RPCs or a new low-value crate.

The observed registry has438entries:27native business handlers including the earlier five, and411TS callbacks. These are observed counts, not dispatch constants. TS retains Fastify's public socket/non-API/WebSocket ownership; this is not a claim that every backend operation has migrated.

## Fresh differential and performance evidence

Baseline is a fresh production build from detached commit `40b7580d3548d698e6794c50fa17e80531fc156e`: native common infrastructure plus24TS business bodies. Candidate `d818d54a033e3ae247184a0ff3e5e6bb7615313a` is a fresh production build of all24native candidate bodies. This measures business migration relative to the current hybrid TS-body baseline, not the original pure-TS backend. Source, distinct complete production bundles, native binaries and fixture identities are recorded in `/workspace/speed-native-reads/final24/`. Final activation only removes the two slower dispatch entries; the22measured native bodies remain unchanged.

- PASS:138same-input public HTTP comparisons, zero differences, including ordinary errors, authorization/ownership and type/null/order boundaries.
- PASS: unchanged common runner, three AB/BA/AB pairs, concurrency8,100warmup/1000measured requests perAPI/build,144000measured and14400warmup requests; zero HTTP/timeout/transport/validation errors; exit0,135.98s.
- PASS: all six benchmark server groups exited0, collected Rust children and released their ports. No builds or other tests overlapped measurement.
- Gate: median throughput at least TS and median p95 no worse than TS.22pass; no dependency exception is used. Their throughput improves27.4–103.6% and p95 falls12.8–54.0%. Index stats is4.6%slower with5.0%higher p95. Table stats is0.20%slower despite lower p95 and conservatively remainsTS. This is a sample-based gate, not a statistical-significance or production-scaling claim.

[Tracked metrics](native-read-benchmark.csv) preserve168rows for p50/p95/p99, throughput, sampled TS/Rust/combined VmRSS, ranges, CV and paired deltas. Private raw results, population variance/stddev and immutable source/build manifests remain in `final24/`. Sampled VmRSS is not peak or unique physical memory. The English PR table is validated with MathJax; percent units occur in headers rather than numeric expressions.

## Corrections and retained failed attempts

The first candidate differential had135passes and3username/available failures. Redis startup refreshes native metadata, whose SQL projection omitted preservedUsernames. The fix extends that projection; a dedicated PostgreSQL regression uses an isolated temporary table and verifies both initial refresh and a later DB update. Ordinary differential checks now pass. This is a fixed migration regression, not an approved infrastructure exception or an old TS bug.

The first API-generation attempt failed because ignored default.yml was empty; retry uses a dedicated private YAML/JSON config and passes without public SDK source changes. The first candidate benchmark used an older TS production bundle because build:e2e only updates built-test. Its preliminary12API selection is withdrawn and is never used for activation. Both sides were then freshly production-built; final24 contains the accepted comparison and gate. Previous failures/provisional results remain under `/workspace/speed-native-reads/` and leader logs, rather than being overwritten or counted as success.

## Final selected-build validation

Selected source commit: `6e8644fec91837ef01b6d8d1bc3ac7bfa9951c41`, following candidate `d818d54a03`; ordinary pushes only. The same independent batch review covers restored operations, the metadata fix and final activation selection.

| Check | Result | Evidence under `/workspace/speed-read-leader/` unless noted |
| --- | --- | --- |
| Selected production build | PASS, exit0,27.48s | `logs/native-read-final-production-build.log` |
| Selected E2E build | PASS, exit0 | `logs/native-read-final-e2e-build.log` |
| Selected public HTTP comparison | PASS,138cases,exit0 | `/workspace/speed-native-reads/selected22-http/completion.json`; distinct final-selected bundle/binary identities; held2TS paths also match |
| Rust workspace | PASS,213passed;7explicit ignores excluded | `logs/rust-selected-workspace.log`,exit0 |
| Metadata refresh DB regression | PASS,1explicit dedicated PostgreSQL test | `/workspace/api-native-phase/speed-read-implementation/speed-read-meta-live-retry.log`,exit0 |
| Rust formatting / all-targets Clippy with warnings denied | PASS,exit0 | `logs/rust-selected-format.log`, `logs/rust-selected-clippy.log` |
| API type generation | PASS after private-config retry; no public SDK source diff | `logs/generate-api-types-selected-retry.log`,exit0 |
| Backend source/test/federation TS checks | PASS,exit0 | `logs/backend-selected-typecheck.log` |
| Related E2E | PASS,3files/19tests,exit0,11.1s | `logs/focused-selected-e2e.log` |
| Complete backend E2E | PASS,37files/1475passed;2skipped and20todo excluded,exit0,366.26s | `logs/full-selected-e2e.log` |
| Changed-file ESLint/SPDX/locale checks | PASS,exit0 | `logs/shipping-selected.log` |
| Independent batch review | PASS, zero outstanding actionable findings in the related batch | `/workspace/speed-read-leader/review-speed-batch.json` identifies the reviewed snapshot |
| Final documentation-head CI | PENDING | Source commit already pushed; final results recorded separately after publication |

Earlier candidate checks do not stand in for final-selected checks. No new fault-injection matrix or wire-validation unit is introduced. Local federation capacity remains the previously documented environment limitation; remote final-SHA federation checks are reported separately.

Evidence roots are `/workspace/speed-read-leader` (commands/cwd/exits/logs), `/workspace/speed-native-reads` (baseline/differential/benchmarks) and `/workspace/api-native-phase/speed-read-implementation` (unit/meta-regression evidence). Private configs/credentials are excluded from Git and published tables. Tracked documentation/CSV survive checkout; ignored builds, tmp targets, Docker state and private evidence need workspace retention and are disposable.

The complete E2E uses explicit dedicated test-misskey/Redis14 settings and a loopback-only localhost DNS fixture, as in the previous cloud run. Tests that reset this DB run sequentially. The performance fixture uses a different speed-read-runtime DB and Redis13. No shared/production service is contacted. The full commands, cwd, exit codes and log locations are in `/workspace/speed-read-leader/results.jsonl`. All test/benchmark HTTP processes are collected; main dedicated services are stopped after validation. PR title/body are unchanged; the English table-only benchmark comment is https://github.com/mothue/misskey/pull/48#issuecomment-6378995209.
