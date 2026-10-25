# Complete native clips/show

`clips/show` reuses the existing native Clip and UserLite formatters, with one metadata-derived clip SELECT and the original sequential count/existence reads. The handler is selected after all six paired measurement conditions pass. `notes/clips` retains its separate performance hold. The [generated registry](native-status.md) records selection; final CI acceptance remains a separate check.

Authentication, supplied-token scopes, input validation and public HTTP handling remain in the common Rust application. A private clip is rejected before date, author or count interpretation; root does not bypass ownership. The direct Clip packer uses anonymous UserLite context even for an owner or moderator. Anonymous responses omit `isFavorited` and `notesCount`; authenticated nonowners receive `isFavorited`, and only owners receive `notesCount`, including zero. Nullable dates/descriptions and original object order are retained. Counts remain JavaScript Numbers without a new safe-integer restriction. No new formatter, crate, helper RPC, schema, frontend, slacc or queue change is introduced.

New SELECT identifiers come from the actual Clip entity metadata and existing quoting helpers. Bootstrap checks the six consumed properties and hydration kinds before readiness. New query/access failures retain typed stages and query causes with safe boundary logging. The reused count/UserLite queries retain their existing SQL; this slice does not claim to clear all historical query/error debt.

## Observed compatibility corrections

The TS-first capture includes finite PostgreSQL dates outside JavaScript's range and timestamp infinity. The former returns `RangeError: Invalid Date`; the latter reaches a primitive `.toISOString()` call and returns a distinct TypeError. Five actual HTTP error-shape mismatches were retained before correction, then corrected using centralized captured definitions. The TypeError text includes the pinned production minifier's local variable name; the differential test guards this build-specific detail, not universal engine equivalence.

The same comparison exposed an existing shared UserLite regression: validating a decoration ID before excluding unmatched metadata caused Rust to return HTTP 500 where TS returned HTTP 200. A fresh public `notes/show` comparison on the same owned note reproduced this in published `7f4fa756b9` versus pure-TS `d875b7e`, for anonymous, owner and author requests. `users/show` controls matched HTTP 200. Commit `48165e43bb` fixes that ordering separately from this feature. Empty decoration inventory performs no member property access; null/undefined access with nonempty inventory retains observed TypeErrors; unknown IDs are filtered before strict selected-entry decoding. No process crash was observed. Six property cases had five pre-fix mismatches and all match after correction; the fresh note/control comparisons also match after correction. This is a corrected migration regression, not a TS baseline defect or a new compatibility exception.

## Fresh cloud evidence

Private evidence: `/workspace/native-queue/clips-show`, including command/cwd/exit-code ledger `results.jsonl`, sanitized logs, before/after HTTP, packing/date fixtures, source/build hashes and independent reviews. The clip business baseline is isolated hybrid `7f4fa756b95890ef7e5914045fbce3fd888829e5`, not pure-TS develop. Dedicated `notes-clips-runtime` and Redis DBs 4/3 were used without a schema change or reset. DB-reset E2E uses separate `test-misskey`/Redis DB14 settings and runs serially.

| Check | Result | Scope / limit |
| --- | --- | --- |
| TS-first public HTTP | PASS, exit 0 | 50 cases: 21 successes, 20 HTTP 400, five HTTP 500, two scope403 and two GET405 responses. Ordinary error results are expected contract observations, not test failures. |
| Before-fix native HTTP | FAIL, exit 1 | Six mismatches: one TS200 / native500 decoration case and five ordinary error-detail differences. Retained separately. |
| Final native HTTP | PASS, exit 0 | Same 50 inputs, bodies, order and stable headers. Only diagnostic `error.info.e.id` UUIDs normalized. Includes private/missing IDs, root, anonymous, account/app scopes, malformed inputs, null/date/count/omission boundaries. |
| Shared decoration regression | PASS after actual RED | PureTS/published-native same-note RED; six property cases RED→GREEN; fresh pureTS/current note/control requests all match. Fixtures restored and temporary metadata removed. |
| Rust checks | PASS | fmt, locked all-target clippy `-D warnings`, workspace 301 passed/11 ignored. Additional style lint set adds no warnings in the new handler/filter logic. Initial test ownership compile errors are retained and do not count as behavioral RED. |
| Backend types / API generation / builds | PASS, exit 0 | Explicit private configurations; generated SDK source unchanged. |
| Related backend E2E | PASS, exit 0 | `note.ts`, `clips.ts`, `api-visibility.ts`: 247 passed, 2 TODO. Private-read assertion now awaited; new viewer omission/count and app-scope checks. |
| SDK response validation | Coverage/parity PASS; SDK BASELINE | Fresh source-derived 35/35 coverage; 100 conforming samples and 22 identical inherited TS/native type discrepancies. All 122 success pairs retained; 63 error pairs compared separately. Global baseline is pure-TS d875; prior local baseline hybrid 8d907; clip baseline hybrid 7f4. |
| Product UI | PASS | Five actual TS/native page pairs: owner/other/root public, empty owner and private owner. Logged-in SPA navigation observes real `clips/show` calls; no page errors or clip mutation actions. Initial clips-list bootstrap failed on deliberately invalid stored dates; switching the harness bootstrap to API console resolved that fixture issue. |
| Child lifecycle | PASS, exit 0 | One Rust child; after termination new handler and four core APIs return fixed 503; host survives, measured live Rust children 0, no restart. |
| Measurement / activation gate | PASS, exit 0 | Six throughput/p95 conditions; all measured responses match captured TS responses. |
| Independent source/metrics review | PASS | Source, all 50 HTTP pairs, date fixture tuples and five UI pairs checked; all 42 metric rows, 1,008 numeric summary/CSV fields and six gates independently recomputed without discrepancies. Request-level latency samples are not retained. |
| Fresh federation / final pushed-SHA CI | PENDING at this evidence stage | Earlier successes do not replace checks for this batch. |

## Paired cloud measurement

Run `c4be0003-9485-46d7-873e-b442c8287abc` uses the unchanged common runner, three AB/BA/AB pairs, concurrency 8, 100 warm-up and 1,000 measured requests per scenario/side: 36,000 measured plus 3,600 warm-ups. Cases cover populated public anonymous/owner/scoped-nonowner reads, empty public anonymous/owner reads and private owner reads. All owned clip rows and mappings/favorites, relevant roles/meta, source, build, configuration and captured responses remain frozen. This bounded dedicated workload is not production traffic.

All six gates pass: median throughput improves 12.40–67.91% and median p95 decreases 20.75–43.70%. [Generated 42-row metrics](clips-show-benchmark.csv) retain variance and sampled TS/Rust VmRSS; memory is not peak RSS or unique physical memory. No favorable-run retry or performance/compatibility exception was taken.
