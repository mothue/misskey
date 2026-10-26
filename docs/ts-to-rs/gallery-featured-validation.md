# Gallery featured candidate: TS retained after measured failure

`gallery/featured` moves the complete business operation to Rust using the existing main Redis read lane, PostgreSQL read lane, Gallery formatter, UserLite batch formatter and public-file map. No TS business callback, helper RPC, new connection, schema, frontend, slacc or internal crate is added. The complete candidate remains off the native selection after a valid paired performance gate failure. The public operation continues through the TS business body; this is a selection hold, with the common Rust API foundation still required.

The per-Runtime cache holds ranking IDs only. It preserves the30-minute wall-clock interval, exact zero sentinel, strict expiry, clock rollback, post-await timestamp and independently completing concurrent refreshes. No lock is held over ranking I/O; no singleflight, timer, invalidation or cached viewer response is added. Current and previous three-day windows use inclusive ZRANGE0..100 in a nontransactional pipeline. Individual command errors leave the other window intact; scores do not control endpoint selection.

Selection uses UTF-16 descending comparison, then optional untilId filtering and limit(default10, maximum100). The metadata-quoted bound IN query has no ORDER BY and does not refill missing posts. Response order follows returned rows. Author batching completes before post Date conversion/file/like packing; the actual viewer and shared hints are retained. Synchronous preparation errors take their observed precedence while valid per-post I/O starts eagerly. Nonwinning read cancellation timing remains best-effort infrastructure behavior, with no business writes in this operation.

Checked integer parsing replaces narrowing casts. Existing signed int4, null/omission, nested/null PostgreSQL arrays, file order/duplicates/missing filtering, Date error class/message and viewer-specific isLiked remain part of the shared contract. Ranking IDs are opaque strings until the selected rows are packed; no full ID generation or date-pagination boundary is introduced.

## Fresh cloud evidence

Private root `/workspace/native-queue/gallery-featured`. Root commands/cwd/exits/durations/logs are in `results.jsonl`; delegated captures carry their own source/build/config/DB/Redis/storage guards. TS business baseline is isolated hybrid `8b8d2adb53bd0ed3b4dbc1f4ef2116cbf50db991`, native executable084c29eb…, rather than pure-TS develop. Measured active candidate executable45b37f92…. Final held executable5ed279fb…. The original active executable, source files, untracked fixtures and patch are archived privately before changing selection. Runtime uses dedicated notes-clips-runtime, Redis4/3 and new Gallery-featured prefixes. Serial DB-reset E2E uses separate test-misskey/Redis14. API generation uses dedicated users-lists-list-runtime and port61913, separate from runtime61912.

| Check | Result | Scope / limit |
| --- | --- | --- |
| Source-derived TS oracle | PASS |13 controlled scenarios/21 callback calls plus two narrow rejected bulk controls. Actual source imported with zero network/process/real-repository/ID-generation guard use. Stub UserLite/file payloads and controlled completion are not HTTP/wire proof. |
| Actual normal TS/native HTTP | PASS |79 identical requests, including empty/small/default12/max100, method/auth/validation, selection/order/non-refill, real empty-cache hit, valid infinite scores and one-window WRONGTYPE. Full body/array order/stable headers match. |
| Actual competing ordinary errors | PASS, with first comparison guard retained |A matches TypeError. Initial candidate B is SKIPPED before its native request because fixture restoration changed natural DB row order; it is not product RED or PASS. A new serial TS/native B pair keeps the mutated state and actual TypeORM row sequence fixed and matches RangeError/Invalid Date. All temporary fields/IDs restore exactly. |
| Rust | PASS |fmt; locked all-target clippy -Dwarnings;317 passed/11 ignored. Extra style found three new warnings, fixed and rechecked; old unrelated warnings remain distinct. |
| TS types/builds/API generation | PASS |Backend/source/test/federation typechecks, production/API generation/E2E builds and changed-file ESLint. Generated SDK source unchanged; two closest TS unit files7 passed. |
| Related backend E2E | PASS after test-input correction |Initial three-file command245 passed/1 failed because the new test incorrectly treated alphanumeric untilId bad as invalid. Replacing only that input with ! yields endpoints99 passed; unchanged note/visibility147 passed on the same native binary. Initial FAIL remains retained. |
| Independent review | PASS |Fresh-context source/runtime/measurement/final-held/docs audits pass. All42 observed metric rows and six gates independently recomputed; valid performance failure confirmed. Audit PASS does not turn the performance gate into PASS. |
| Candidate SDK response validation | Coverage/parity PASS; declarations BASELINE |Fresh38/38 coverage;289 success pairs retained:233 conforming and56 identical inherited discrepancies.102 error pairs compared separately;12 fresh captures/13 commands all exit0. This describes the active candidate, not final held-state activation. |
| Candidate product UI/lifecycle | Main parity/lifecycle PASS; UI BASELINE |Five actual Gallery-index pairs/10 pages; cards0/3/3/12/15, images/order/links/likes/badges match. UI paginator requests15; max100 correctness is a separate HTTP case. Matching app notifications403 is the only ancillary limitation, with no500s. One child becomes zero; featured+four core APIs return503, no restart, host exits0. |
| Paired measurement/gate | FAIL; five conditions SKIPPED |Two profiles/six conditions measured on their first attempts: empty3/3 PASS, small2/3 PASS and one throughput FAIL. Default10/max100 five conditions remain unmeasured. No retry, optimization or dependency exception. |
| Fresh Linux federation | PASS, exit0;13 skipped |Final held29a1136e9a, fresh Linux build113.650s; nine files92 passed/13 skipped; test84.382s. This does not activate the native candidate or change the failed performance decision. |
| Final pushed-SHA CI | PASS |`a13cc4ed36535d01ca729209e0e3eb83bf26d3ed`:18 successful workflows and one skipped Storybook, all first attempt. The TS performance hold remains unchanged. |

The earlier Gallery-show invalid-data fixtures were restored transactionally in six owned rows, with zero deletes and unchanged historical capture/measurement evidence. Subsequent live captures use the restored data and fresh manifests; historical71-response evidence is not reused as a current result.

## Paired cloud measurement and hold

The unchanged runner uses C8,100 warm-ups and1,000 measured requests, three AB/BA/AB pairs per scenario/side. The first empty run is `2c46a9bb-c531-46d5-b8b7-885d60460543`; the first small run is `65986b36-e277-4cbb-b808-1f37a6329cce`. All36,000 measured+3,600 warm-up+12 descriptive cold responses match; zero errors/partial runs, all12 measured server sides exit0 and collect their children. The4-profile prepare/dry stage separately passes44 warm-up+8 cold checks; these are not measured requests.

For small/other-reader, median throughput changes630.6164267625835→602.7765595395589 req/s, **−4.414706950459102%**; p95 changes19.742168000000675→19.338910999998916ms, **−2.0426176091792203%**. All three paired throughput deltas are negative:−16.1395/−0.98083/−1.11428%. Throughput CV is3.00% baseline and5.27% candidate. The finite workload establishes a gate failure, without proving its cause or production significance. No favorable rerun or performance exception is taken.

[Observed42-row metrics](gallery-featured-benchmark.csv) contain six measured conditions only. Five planned conditions/35 metric rows remain SKIPPED, rather than zeros or successful measurements. Private `benchmark-prep/held-benchmark-summary.json` explicitly has INCOMPLETE status; `held-benchmark-gate.json` is FAIL. Per-run summaries preserve descriptive variation and sampled TS/Rust/combined VmRSS, not peak/unique physical memory. Individual request latency samples are not retained. The English PR table includes N/A/SKIPPED for unmeasured conditions and keeps percent units outside colored math.

A transient exec transport disconnect occurs before measurement. After recovery, workspace/tmp/binary and live dedicated services persist and canonical source/build/HEAD/input guards match. An extra Python preflight initially compares TS-only hashes with a full TS+Rust hash; its failure and unintended continuation into the first empty prepare are preserved separately. Canonical full-inventory recomputation matches the existing evidence without weakening the harness. All actual measurements then use one first attempt per executed profile.

The API remains held. The selected native/TS/held state is generated from the registry and the explicit performance hold. Final held-state HTTP, lifecycle, endpoints E2E and fresh Linux federation are checked separately below; historical active-candidate results are not substituted for them.

| Final held-state check | Result | Scope |
| --- | --- | --- |
| Public HTTP/lifecycle | PASS, exit0 |81 fresh comparisons against unchanged TS baseline.17 actual unchanged child readiness frames select37 and exclude featured; one child becomes zero, featured+four core APIs503, host exits0. All controls restored; historical artifacts unchanged. |
| Endpoints E2E | PASS, exit0 |99 cases on final held executable, dedicated test-misskey/Redis14. |
| Rust final held selection | PASS, exit0 |Workspace317 passed/11 ignored after selection removal. |
| Fresh Linux federation | PASS, exit0 |Container-only Rust/slacc outputs; source1,212/host JS195/harness and29a1136e9a HEAD remain fixed, owned environment removed. |

## Final held Linux environment

Project `cloud-gallery-featured-fed` uses fresh native output mounts with host application artifacts masked, retaining only Linux compilation caches. Build113.650s and federation84.382s complete on first attempts. All setup/log/initial teardown commands exit0;9files92PASS/13SKIPPED. All1,212 selected source files,195host JS artifacts, harness and committed29a1136e9a HEAD remain unchanged.

The first cleanup preserves original down evidence, records both Linux native artifact hashes, resets only the nine owned output directories and leaves zero owned containers/volumes/networks. No retry or capacity guard stop occurs. Minimum overlay free space is1,774,211,072bytes, tmp3,563,749,376bytes, effective available memory2,103,935,600bytes; all exceed1GiB. Cleanup returns5.52GiB overlay and3.87GiB tmp free. Main dedicated PG/Redis services are retained for the ongoing queue. Stage commands/exits/logs/pins are in private `federation/execution-summary.json`.
