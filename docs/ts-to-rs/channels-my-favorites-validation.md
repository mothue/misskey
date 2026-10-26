# Channel favorites candidate: performance hold

`channels/my-favorites` retains its complete TS business operation. The complete native candidate matches58 actual public HTTP pairs, but the first paired throughput/p95 gate passes five of eight conditions and fails three. All conditions were measured; no exception, omitted condition or favorable measurement rerun is used. The candidate remains implemented and absent from native selection. The existing Rust API infrastructure and Fastify public socket continue to serve the TS business handler.

## Contract and shared implementation

The endpoint requires credentials and `read:channels`, declares an empty object parameter schema, and returns the viewer's own favorites. Extra limits, date bounds and viewer IDs remain ignored. The metadata-derived owner-restricted favorite query LEFT JOINs channels without sorting, limiting, filtering archived/sensitive/ownerless channels, or reconstructing favorite insertion order. Output preserves actual repository row order and eager `Promise.all` result positions. GET remains empty405; existing accepted PUT remains200 through the shared method/validation pipeline.

The new internal Channel formatter follows the actual per-object `ChannelEntityService.pack` operation: optional banner lookup, independent following/favorite/muting reads, and the source unused pinned-note read. Even expired mutes count here. Pinned rows are discarded without note/reaction/date decoding or packing. Independent existence reads are retained rather than inferring favorite=true from the first snapshot. `hasUnreadNote` remains false and `pinnedNotes` is omitted. Banner URLs reuse the existing native public file URL implementation and current metadata/configuration; no full DriveFile DTO or UserLite packing is added.

A separate behavior-preserving commit extracts the existing Gallery stored-text-array and Notes State projected-layout helpers. Rectangular PostgreSQL multidimensional arrays and null elements retain their source shapes and parameter semantics. Bootstrap exports actual TypeORM channel/favorite/following/muting/file layouts; identifiers are validated and never hand-copied into SQL. Scalars use strict borrowed UTF-16/null/string/Boolean/int4 checks at source evaluation points. Existing ID/date conversion and ordered public Value machinery preserve null versus omission, empty values, pre-epoch/submillisecond handling, expanded years and existing invalid-Date500 responses. No new connection family, cache, schema, helper RPC, TS callback, fallback, retry or child restart is added by the candidate.

An independent review found that a banner deleted after the favorite snapshot must retain ordinary TS `EntityNotFoundError` details; this is not an infrastructure-fault exception. An isolated actual TS public HTTP deletion oracle returns500 with `MiDriveFile` criteria. The native empty-row-to-response unit fails first with Cargo101, then passes after the typed missing-banner mapping is fixed. A native public HTTP deletion-race execution is **SKIPPED**; it is not part of the58-case HTTP claim. Scalar type-validation findings are also resolved.

## Executed checks

Evidence is retained under `/workspace/native-queue/channels-my-favorites/`. Commands, cwd, exits and durations are recorded in `results.jsonl`; source/runtime artifacts and command logs remain private. Earlier results are not relabeled as fresh executions.

| Result | Check | Actual result / evidence |
| --- | --- | --- |
| PASS | Actual extracted TS callback/Channel.pack oracle |15 controlled source cases; `source-oracle.json` |
| PASS | TS-first `channel.ts` public E2E |6 tests; reviewed membership assertion avoids promising an unspecified global SQL order; `phase/new-e2e/` |
| PASS | `cargo fmt --all --check`; locked workspace all-target clippy and tests |349 passed,11 existing ignored; all commands exit0; `phase/channels-review-*` |
| PASS | Candidate runtime package tests |188 passed,6 ignored; final exit0; `logs/candidate-selection-tests-attempt04.log` |
| PASS | Backend typecheck; API generation; candidate production build |25.40s/40.08s/31.75s, exit0; generated SDK unchanged; `root-checks.json` |
| PASS | Related public E2E: channel/endpoints/note/api-visibility |4 files265 tests,0 skips, exit0; wrapper19.03s/Vitest18.77s; actual42-handler readiness |
| PASS | Public HTTP success, auth/scope/input errors, order, empty values and date/file/array boundaries |58 pairs:52 main+3 external-proxy+3 proxy-off; full body/order/raw UTF-8/hex/meaningful headers match; `candidate-http-validation.json` |
| PASS | Actual product chooser UI and native child lifecycle |Paired rendered favorites, links/order/flags; child1→0→0,503after loss, no restart, host exit0; `main-verification-results.json` |
| BASELINE | UI assets |Three identical blocked external placeholder images; target API/DOM checks pass, page/ancillary API errors0 |
| PASS | Fresh whole-native differential coverage |657 pairs/26 sides:444 successful payload pairs+213 error pairs;42 selected names covered, no missing or differential failure; `sdk/sdk-final-results.json` |
| BASELINE | Generated SDK declarations |Successful payload validations370PASS/74BASELINE,0FAIL; inherited71 raw hashes/diagnostics remain identical. All58 new responses:50PASS/8BASELINE; these are source-equal declaration failures, not passing type validations |
| PASS | Independent code, image, HTTP, UI/lifecycle and SDK review |Findings resolved; exact report/hash links under `phase/` |
| SKIPPED | Full local TS unit suite |Related E2E/types/generation/Rust checks run; full TS units await final pushed-SHA CI |

The52-case main candidate capture executes at `ba06becc8b7a5edf40589b6e78304c215d8cd501`. Removing one test-only held-selection assertion at `f596a9cc111d6ce6dfd8cb9a6ae56d8b81a950d7` yields byte-identical complete production JS/native images, independently verified. Original capture metadata and bytes are retained, not relabeled or rerun. Six URL-variant cases execute freshly at `f596`. Only validated v4 diagnostic `error.info.e.id` UUIDs are substituted on comparison copies; original raw bodies remain untouched.

SDK execution was interrupted after20 sides when work paused. A reviewed continuation verifies original logs, captures, source/build/storage/input identities and complete data/schema snapshots, then executes only six missing sides in the same session. All26 captures remain the first capture of their side, with no overwrite or recapture. Continuation exits0 in341.88s; closure exits0 in35.18s. The eight new declaration baselines are null/multidimensional stored pin arrays(2), existing string-valued Fastify400 errors(2), undeclared GET405(3), and the actual accepted PUT200(1). Rust responses and diagnostics equal TS in each case. Overall SDK status remains BASELINE while semantic/error parity and coverage are PASS.

Original failed harness attempts remain preserved: wrong package/path/configuration assumptions, incorrect SQL-order expectation, an overstrict worktree guard, string-valued error comparator handling, a test-only held assertion, and the initial UI ancillary allowlist. The corrected UI attempt retains the original activity window and exact saved source rows; it does not learn new baseline data. Original failures remain FAIL_HARNESS, not product BASELINE or success.

## First paired measurement

Run `12e6c66f-fb1f-4044-8ba2-f9d79fa96211` uses the existing runner at concurrency8,100warmups and1,000measured requests per side/condition in three AB/BA/AB pairs. Eight preregistered conditions cover0/1/10compact/10mixed favorites with account/application credentials. All48,000measured,4,800warm and six descriptive first requests succeed with exact expected responses. The baseline is published `099efc274852896de50139aea0737d6f03c551bd`: TS business on the common Rust infrastructure, **not pure-TS develop**.

Preparation30.55s and first dry126.67s pass; dry has32warm requests and no measured requests. First measurement exits0 in397.78s; summary exits0; strict gate exits1. Every condition requires median candidate throughput≥TS and median p95≤TS without tolerance.

| Condition | Median throughput delta | Median p95 delta | Gate |
| --- | ---: | ---: | --- |
| empty-account | +53.62166130% | -24.33200776% | PASS |
| empty-app | +21.34581370% | -19.41258511% | PASS |
| one-compact-account | +38.32961000% | -23.17356482% | PASS |
| one-compact-app | +10.52700012% | -16.83491833% | PASS |
| ten-compact-account | -8.64028915% | -4.22090978% | FAIL |
| ten-compact-app | -23.41121850% | +54.94756911% | FAIL |
| ten-mixed-account | +21.22994448% | -15.15641496% | PASS |
| ten-mixed-app | -2.91358711% | +19.06704416% | FAIL |

The unaltered [56-row CSV](channels-my-favorites-benchmark.csv) records p50/p95/p99, throughput and sampled TS/Rust/combined RSS, three run aggregates, min/max/CV and both ratio-of-medians and paired deltas. The table uses ratio-of-medians deltas. Variation is not a confidence interval; sampled VmRSS is not peak or unique physical memory. An independent review recomputes all56 JSON/CSV metric rows, all eight gates and archived source/build byte identities. All eight conditions are included. Raw/summary/gate/lock/provenance and all five unchanged controls remain under `phase/benchmark-prep/`; no overlapping build, test or browser session runs during timing.

## Held state and remaining acceptance

The measured42-handler candidate source1129files and64built files are archived and byte-verified in `measured-candidate-preserved/`, together with the original working selection patch. Only the temporary active-selection line is removed. The held production build exits0 in30.89s. Fresh held verification exits0 in197.97s: all58 HTTP pairs match, readiness advertises41 native handlers and excludes this endpoint, and lifecycle again confirms1→0→0 children,503after loss, no restart and host exit0. Original complete domain/schema/storage snapshots and browser activity anchor remain exact. The port is released and no owned Rust child remains. Evidence is `held-verification/held-final-results.json`. The held binary SHA256 is `ac967760d40d36586cf9a210eda9e922d32fed946980b9a562aaf96354fef8ba`. No repeated UI, SDK or measurement is claimed for the held selection.

The held capture retains its actual `f596` source/build tuple. The subsequent hold-manifest, generated status-table and validation-record edits are metadata/documentation changes after that execution; no capture is relabeled to a later commit. The generated registry records41 native,383 TS and14 held endpoints from source, without fixed-count dispatch.

Fresh final-state Linux federation and final pushed-SHA CI are pending. The performance decision remains Held(TS) regardless of those acceptance checks. PR title/body and AGENTS.md are unchanged; CodeRabbit is not invoked. No public listener, queue worker, frontend product, slacc source or schema change is included.
