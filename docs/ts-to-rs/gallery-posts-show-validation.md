# Complete native gallery/posts/show

`gallery/posts/show` executes the complete business operation in Rust through the existing PostgreSQL read lane, UserLite formatter and public-file formatter. The public listener and non-API routes remain TS. No TS business callback, new connection, cache, internal crate, schema, frontend or slacc change is added. All eight paired performance conditions pass; fresh federation also passes, and final pushed-SHA CI remains separately pending below.

The query is compiled from actual entity layouts, with quoted identifiers and bound values. Missing posts retain NO_SUCH_POST400. The endpoint has no required kind: a valid application token without a required scope is still admitted. UserLite receives the actual viewer. Anonymous isLiked is omitted; authenticated false/true, stored likedCount and array order remain unchanged. All attachment rows are packed before reconstructing original top-level string keys, preserving duplicates and missing filtering. Legal multidimensional PostgreSQL arrays, null members and empty-array omission follow actual TS hydration/Map behavior rather than a stronger invented schema.

Checked int4 conversion preserves signed range without truncation. Date infinities, dates outside the JS range and malformed attachment IDs retain the observed legacy error class and message. Typed layout/query/row/parse/UTF16 causes remain internal. The shared attachment-map extraction is a separate behavior-free commit; the shared invalid-Date correction is another commit backed by actual pure-TS/published-native/current-native notes/show HTTP.

## Fresh cloud evidence

Private root: `/workspace/native-queue/gallery-posts-show`. `results.jsonl` records root commands, cwd, exit, duration and logs. `verification-results.json` records delegated runtime results, exact artifact hashes and cleanup. The business baseline is isolated hybrid `a15daf3926ad8ad391735e8c8ce1c90e47a9f41f`, where this handler is TS; it is not pure-TS develop. The shared attachment regression additionally uses actual pure-TS `d875b7e`. Dedicated `notes-clips-runtime`, Redis4/3 and owned Gallery prefixes/storage are separate from serial DB-reset E2E (`test-misskey`, Redis14).

| Check | Result | Scope / limit |
| --- | --- | --- |
| TS-first/native public HTTP | PASS, exit0 |71 identical inputs:51 successes,10 client400 responses (six validation/four missing-post),2 GET405 and8 stored-date errors; status/body/order/stable headers match, diagnostic UUID normalization only. |
| Actual failing comparison before repair | FAIL retained, then PASS | Unfixed candidate differs on6/71 inputs:4 legal nested-array200-to500 and2 malformed-file Date diagnostics. Immutable RED binary/source/responses remain private. |
| Published shared attachment regression | FAIL reproduced, then PASS | Same owned note/file, anonymous and owner: pure-TS RangeError vs published native generic500; corrected native matches both actual TS responses. |
| Rust | PASS | fmt, locked all-target clippy -Dwarnings,311 passed/11 ignored. Extra style lint introduces no changed-logic warning. |
| TS types/builds/API generation | PASS | Backend typecheck, API/production/E2E builds and changed-file ESLint; generated SDK source unchanged. Closest units5+2 pass, as counted by the actual two Vitest logs. |
| Related backend E2E | PASS after fixture cleanup | Initial three-file command244 passed/1 failed because new uploads contaminated an existing drive-usage assertion. Scoped public-API cleanup fixes it: endpoints98 pass; unchanged note/API-visibility147 pass on the same native binary. Initial failure is retained, not relabeled PASS. |
| SDK response validation | Coverage/parity PASS; types BASELINE | Fresh37/37 source-derived coverage;218 success pairs retained:162 conforming,56 identical inherited TS/native discrepancies, including13 Gallery null/nested-array cases.92 error pairs compared separately. |
| Product UI | Main parity PASS; overall BASELINE | Seven real Gallery page pairs preserve responses, body, image order/loading, links and likes. Deliberately malformed stored fixtures also cause matching auxiliary users/gallery/posts and anonymous local-timeline500s; this is not a blanket healthy-UI claim. Initial readiness-selector harness failure is preserved separately. |
| Lifecycle | PASS | Observed1-to0 live Rust children; Gallery and4 core APIs return503 RUST_BACKEND_UNAVAILABLE; host survives, no restart, final host exit0/no child. |
| Independent review | PASS | Fresh-context source, runtime, metrics, docs and federation evidence reviewed; all56 metric rows/eight gates independently recomputed, frozen federation source/artifact/harness hashes independently verified. |
| Paired measurement/gate | PASS, exit0 | Eight actual normal POST scenarios;48,000 measured +4,800 warm-up +6 cold responses all match. All eight median throughput/p95 gates pass on the first attempt. |
| Fresh Linux federation | PASS, exit0 | Fresh Linux build113.106s; nine files,92 passed/13 skipped; test84.611s. Frozen1319d080e3 HEAD,1,208 selected sources,195 host JS artifacts and harness remain unchanged. |
| Final pushed-SHA CI | PENDING | No previous/intermediate result substituted. |

No new compatibility exception is taken. SDK declaration mismatches and deliberately corrupted stored-fixture failures remain visible as BASELINE rather than being filtered out. The shared attachment Date fix restores ordinary published response parity; it is not the approved infrastructure-detail exception.

## Paired cloud measurement

Run `905cfc2e-ad95-47de-95aa-e5f51470505d` uses the unchanged common runner, three AB/BA/AB pairs, concurrency8,100 warm-ups and1,000 measured requests per scenario/side. The eight scenarios cover anonymous/owner/other-user application viewers for one-file and duplicate/missing/reordered-file posts, plus anonymous/owner empty-file posts. Median throughput improves21.31–74.11%; median p95 decreases17.52–48.25%. No performance exception or favorable rerun is taken. p99 changes range from49.38% lower to24.00% higher; combined sampled RSS is14.24–22.53% lower, while Rust sampled RSS is0.43–2.08% higher. The acceptance gate concerns throughput/p95, not improvement in every metric.

[Generated56-row metrics](gallery-posts-show-benchmark.csv) preserve three-run medians, min/max, CV and paired deltas for latency, throughput and sampled TS/Rust/combined VmRSS. These are bounded synthetic workloads, not production traffic or peak/unique physical memory. Raw per-request latency samples are not retained. Latency ends at complete response bytes; semantic parsing/validation is excluded from latency but included in throughput, identically for both sides.

The complete source/build/config/HEAD, actual response captures, bounded owned Gallery/file/like/role metadata and private storage hashes remain fixed throughout measurement. Candidate HEAD is `1679bec20d`, with the reviewed Gallery working-tree diff captured by source hash; native executable SHA256 is `084c29ebb3744ecf86ae0cc32dd875b113f508fb9a4fcde75ef69f38b1e4c2e3`. All eight server lifetimes (two readiness plus six measured sides) exit0; all16 host/native PIDs are collected, and the runner's port checks pass. An extra Python bind check without address reuse fails on TIME_WAIT; this supplemental check is retained separately, and actual Node listen/close subsequently succeeds without rerunning measurement.

## Fresh Linux federation

Project `cloud-gallery-posts-show-fed` builds slacc/backend-rust into fresh container-only outputs, reusing only existing Linux compilation caches. Dedicated two-instance DB/Redis volumes, proxies and explicit test settings isolate the suite. All setup, build, test, log collection and teardown commands exit0 on their first attempt. All source/artifact/harness pins remain unchanged through teardown. Declared13 skips remain unverified.

The capacity guard never fires: minimum overlay free space1.735GiB, tmp3.322GiB and effective available memory2.037GiB. First cleanup succeeds, preserves the original teardown evidence, records both native artifact hashes, resets only owned temporary outputs and leaves zero owned containers/volumes/networks. Shared compilation caches are retained. Free overlay space returns to5.61GiB and tmp3.88GiB. Exact commands, stage exits, logs and pins are in private `federation/execution-summary.json` and adjacent stage records.
