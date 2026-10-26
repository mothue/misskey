# Complete native Bubble Game ranking

`bubble-game/ranking` moves its complete business body to Rust, reusing the existing PostgreSQL lane and UserLite formatter. Public HTTP/auth/validation remain in the existing common runtime; Fastify continues to own the public socket. No frontend product, slacc, schema, crate, pool, cache, helper RPC or TS business callback is added. The first valid paired measurement passes every preregistered throughput/p95 gate. Fresh Linux federation also passes; exact final pushed-SHA CI is recorded separately after publication.

## Preserved contract

The installed TypeORM joined take10 path executes two SELECTs on one selected slave QueryRunner without a transaction. Native `ReadLease` checks out one existing read session: DISTINCT id/score selection orders score DESC then id ASC for membership; a nonempty selection triggers the joined load with the same mode/cutoff and selected IDs, ordered only by score DESC. Empty results skip that second query. Actual response tie order is preserved rather than sorted to first-query IDs. Per-statement limits, healthy release, cancellation retirement and source budgets reuse the existing adapter machinery; the former one-query path shares its extracted execution helper.

The seven-day cutoff is sampled once and uses strict greater-than. Verification flags, remote/suspension/block state introduce no new filters. Signed int4 scores remain exact, including negative/extreme values. Arbitrary mode strings, empty/Unicode and over-varchar-length read parameters remain valid. Lone UTF-16 surrogates receive the same UTF-8 replacement at the PG bind boundary. The existing NUL input500 and nested QueryFailedError details are preserved and tracked as [mothue/misskey#62](https://github.com/mothue/misskey/issues/62); no new compatibility exception is taken.

Loaded joined users reuse the existing sixteen-field projection and formatter; there is no ID refetch or duplicate-author deduplication. Packing starts eagerly and keeps source input/result order. The callback always uses anonymous UserLite even for authenticated/admin viewers, preserving public-only badges. Response order and key order remain id,score,user; an unmatched source find omits user rather than returning null. Artificial unmatched source-oracle markers establish assembly semantics only, not a valid stored-row outcome. Concrete query/row/JSON/formatter causes remain retained with bounded operation/kind logs.

## Fresh cloud verification

Private root: `/workspace/native-queue/bubble-game-ranking`. Ordinary commands/exits/durations are in results.jsonl and logs; immutable benchmark execution uses phase/benchmark-prep/results.jsonl and its logs. Actual baseline is published7d924930b99d0a7471464d3c950db340cb5a4c1f: TS ranking business behind the hybrid Rust API kernel, not pure-TS develop. Candidate source is9a5f259cfdd2c85c995f92ff45cbffebea2240a8 plus reviewed local selection, native SHA256 f49a2c838d5f581de8e252056e783570f8beb3b5ee7a8dd584639d22c1f90cb9.

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Actual-source oracle | PASS |18 orchestration cases and6 installed pg serializer string cases; controlled formatter markers are not DTO/privacy evidence. |
| TS HTTP baseline and native differential | PASS |56 same-input pairs:45success,9input400,1auth401,1NUL500. Full bodies/key order/stable headers match; diagnosticUUID normalized only on error comparison copies.83actualTSqueries establish two statements/same runner and one statement for empty. |
| Rust fmt/clippy/tests | PASS |332passed,11ignored; locked all-target clippy-Dwarnings. Extra style checks preserve unrelated existing debt. |
| TS checks and production/API generation | PASS |Source typecheck33.54s, changed metadata lint8.90s, APIgen/build45.17s; generated SDK source unchanged. Closest7units pass and are also within the full suite. |
| Related backend E2E | PASS |Four new contract regressions;3files254tests, first run15.92s. New-test types/lint pass; stable6/8day HTTP controls, exact cutoff equality only source oracle. |
| Full backend units | PASS |First unfiltered run80files951passed2skipped,158.208s. Dedicated Meili1.49 health/down pass; owned resources0 and57712released. |
| Fresh SDK/semantic comparison | PASS parity/coverage; SDK BASELINE |16fresh sides/8groups456pairs;330success=265SDK PASS65identical inherited BASELINE,126errors match. Native coverage40/40 and exactly two TS controls. Nine new GET cases lack generated declarations; POST schemas pass. |
| Actual product UI | PASS main views |Retained TS NORMAL1/SQUARE10 and native four actual GET200 via ordinary mode buttons; final raw bodies/params/rendered scores agree. Initial setup/routing failures retained. Two authorized Later0 setup actions change only the root preference insert/updatedAt; ranking authors/data/auth stay strict, root activity is baseline-anchored. |
| Child-loss lifecycle | PASS |Observed1→0 Rust child; ranking+ping/test/endpoint/endpoints503, no restart, hostexit0, children0/portreleased. |
| First paired performance gate | PASS |All9strict gates,54k measured+5.4k warm+6descriptive first requests; independent63metric recomputation passes. |
| Independent batch review | PASS source/runtime/metrics |Exact core/staged/test postimages and actual HTTP/SDK/UI/lifecycle/units reviewed independently; final documentation review follows. |
| Fresh Linux federation | PASS |Clean5a6aa39a selection; Linux build117.037s,Compose test85.386s,9files92passed13skipped; first runtime attempts and owned cleanup pass. |
| Final pushed-SHA CI | PENDING at this documentation commit |Earlier7d924930 CI is not reused; final exact-SHA records collected after publication. |

The dedicated bubble-game-runtime DB is a verified clone of an idle owned test DB; source twelve-table hashes remain unchanged. Ranking50rows, authors, badges/assignments, credentials/scopes, schema and configuration are fixed. Actual frontend setup precedes baseline freezing. Later UI wizard setup is separately proved with exact scoped root registry rows, all other rows unchanged, and original root activity reconstruction. No SQL repair or product preference override occurs. No shared or production service is used.

## Attempt provenance and limits

Initial bundle-count/sample badge assertions, unbundled SQL observer and socket TIME_WAIT checks were private harness failures, retained separately from product results. Actual scoped bundled query observation records no auth bindings. SDK private prefix/State adapter mistakes precede affected HTTP; twelve valid original side captures remain hash-pinned and the four missing sides explicitly continue that session. Four redundant interrupted-session sides remain archived and excluded. The final comparison is complete, not an uninterrupted first-attempt claim. Initial UI setup obstruction and invalid router-remount attempt remain linked; valid TS views are retained and the targeted native mode-button correction supplies the actual successful comparison.

File-only benchmark finalization initially rejected the already-reviewed three-field SDK continuation. The repaired guard admits only that exact original-to-repaired snapshot delta. First executable preparation then rejected legitimate repeated same-input TS controls; the correction requires every matching raw/status/header oracle to agree before selecting its known original case. Previous five control files and finalized DAG are archived with original hashes/mtimes. These setup failures sent no benchmark requests. The valid replacement prepare and first dry pass; dry totals36warm+2first/0measured. The first measured attempt is neither retried nor selected from alternatives.

## Paired cloud measurement

Run4920a4e2-4d98-4bdd-92bc-2363c4000408 completes in205.15s. The unchanged common runner uses C8,100warm-ups and1,000measured requests for each condition/side in three AB/BA/AB pairs. Nine preregistered POST workloads cover0/1/10records under anonymous/account/scoped-app viewers. POST responses use private,max-age=0; actual GET60s cache headers are verified separately and repeated source GET still queries the DB. Every full success response byte and five stable headers match the fresh TS oracle; all54,000measured and5,400warm-ups succeed. Six first requests after readiness are descriptive, not system-wide cold-cache claims.

All nine median throughput values improve18.74–117.19%, p95 decreases21.34–51.23%. [Generated63-row metrics](bubble-game-ranking-benchmark.csv) include p50/p95/p99, throughput and sampled TS/native/combined VmRSS medians,min/max,CV and paired deltas. Population SD is available in the private JSON summary. VmRSS is sampled, not peak or unique physical memory. Raw per-request latency samples are not exported. Latency ends after complete bytes; full-byte validation remains outside latency and inside throughput identically on both sides.

The first-write measurement lock, raw aggregates, provenance and all historical attempts remain preserved. All4,014file identities/22links, canonical source/build trees, configuration, current raw rows/meta/preferences and prior case/oracle files remain exact through the single measured attempt. Both dry and all six measured host lifetimes collect their one native child and release61912, exit0. No build/E2E/other runtime overlaps measurement; no exception, omitted condition or favorable rerun is taken.

## Fresh Linux federation

Clean activation HEAD5a6aa39ae1e844823f976f6663f883f3923bce3f is frozen with source,host JS and eleven harness inputs. Dedicated project cloud-bubble-ranking-fed builds slacc/backend-rust in fresh Linux-only outputs, masking host native artifacts and reusing only existing Linux compilation caches. Build exits0 in117.037s (orchestration117.068s). Database/application/proxy setup, actual test and initial teardown all exit0 on their first attempts. Nine files produce92passed,13declared skipped and no failures: Compose test85.386s,Vitest83.76s; the complete services wrapper is129.795s. No runtime stage is retried.

The original metadata-summary wrapper exits1 because its regex does not remove ANSI escapes from the successful Vitest summary. That FAIL report/log remain untouched. execution-summary-final.json derives the corrected counts from an in-memory ANSI-stripped copy of the identical raw log; it does not rerun or relabel a failing test. Raw test SHA256 is9c71123c5f549fb2f2a49e4e2b24100013ef41fc895d8caa78b8f08e9a693d68.

Minimum available overlay1,543,012,352bytes,tmp3,552,448,512bytes and effective memory2,492,725,616bytes exceed the1GiB guard. Initial teardown20.677s is archived unchanged as down-001 before owned cleanup. Cleanup succeeds with0containers/volumes/networks and nine empty output directories; shared Linux cache identities remain intact. Source,JS and harness pins remain unchanged through final checks. Main dedicated queue PG/Redis remains available.

Exact commands, elapsed-time definitions, source/harness/native ELF hashes, first teardown and resource cleanup are retained in /workspace/native-queue/bubble-game-ranking/phase/federation/execution-summary-final.json and adjacent immutable first-attempt logs. Independent audit precedes the final documentation commit. Final CI uses the final pushed SHA; this pre-publication document does not claim CI success.
