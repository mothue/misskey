# Complete native flash/show

`flash/show` executes the whole business operation through the existing native PostgreSQL lane and actual-viewer UserLite formatter. The public listener remains TS; no TS business callback, new connection, internal crate, cache, schema, frontend or slacc change is added. All eight first-attempt paired throughput/p95 gates pass. Fresh Linux federation and final pushed-SHA CI remain pending below.

The query is compiled from actual Flash/FlashLike entity layouts, with quoted identifiers and bound values. Missing Flash retains NO_SUCH_FLASH400 and its existing error ID. This public endpoint has no required kind. Stored `private` visibility controls profile listing, rather than introducing an owner-only detail gate: anonymous and unrelated viewers retain TS success. UserLite receives the actual viewer. Anonymous isLiked is omitted; authenticated false/true, signed int4 likedCount, raw visibility and exact string contents remain unchanged. Permissions metadata is checked, but permissions values are neither projected nor decoded because TS does not consume them.

Lookup completes before author packing, authenticated like lookup, then ID/updatedAt conversion. This preserves source failure precedence. Shared stored-value/entity-layout helpers are extracted in a separate behavior-free commit; Gallery behavior is unchanged. No independent ID generation or approximation is introduced.

## Fresh cloud evidence

Private root: `/workspace/native-queue/flash-show`. Root commands, cwd, exit codes, durations and sanitized logs are in `results.jsonl`; delegated captures and provenance remain adjacent. Baseline is immutable hybrid `a13cc4ed36535d01ca729209e0e3eb83bf26d3ed`, where Flash business logic remains TS; it is not pure-TS develop. Dedicated `notes-clips-runtime`, Redis4/3 and owned Flash prefixes are separate from serial DB-reset E2E (`test-misskey`, Redis14). Six owned Flash rows and two likes are the only new runtime DB resources; temporary stored dates are restored exactly.

| Check | Result | Scope / limit |
| --- | --- | --- |
| Source oracle | PASS |17 actual TS service cases,11 successes/six rejections; network/process/repository/ID-generation isolation guards remain zero. Deliberately DB-unreachable source controls are not claimed public HTTP. |
| TS-first/native public HTTP | PASS, exit0 |44 identical inputs:22 success,13 client400,one401,two GET405,six stored-Date500. Full bodies/order/stable headers match, diagnostic UUID normalization only. All temporary dates restored. |
| Rust | PASS, exit0 |fmt, locked all-target clippy -Dwarnings,323 passed/11 ignored; extra style lint has no changed-logic warnings. |
| TS types/build/API generation | PASS, exit0 |Backend typecheck, production/API/E2E builds, changed-file ESLint; generated SDK source unchanged. Closest units5+2 pass. |
| Related backend E2E | PASS, exit0 |Three files248 passed on the first attempt: endpoints101, note69 and API visibility78. Two new Flash tests cover viewer-specific badges/likes, profile-private detail access, empty/Unicode values, missing400, invalid input400 and GET405. Owned resources are cleaned through public APIs. |
| SDK response validation | Coverage/parity PASS; types BASELINE |One fresh session covers38/38 candidate native endpoints and retains three inherited TS controls.248 success pairs:192 conforming,56 identical inherited declaration discrepancies.106 error pairs compared separately. |
| Product UI | Main parity PASS; overall BASELINE |Five real Play detail pairs/ten final page commands exit0 preserve actual responses, DOM, pure-text Play renderer, edit/like/author/avatar behavior. Expanded source presentation matches but CDN-loaded Shiki languages/themes are blocked by the explicit off-origin guard; source highlighting is not verified. The limited app pair also matches notifications403, registry/set403 and a frontend pageerror. |
| Lifecycle | PASS, exit0 |Ready inventory includes38 native endpoints/Flash and excludes held Gallery featured. Observed one-to-zero Rust children; Flash and four core APIs return503 RUST_BACKEND_UNAVAILABLE, with no automatic restart; host exit0, no child, port released. |
| Independent review | PASS |Fresh-context source/runtime/metrics/docs review finds no actionable issue; all56 metric rows and eight gates independently recomputed. Fresh Linux federation review remains a separate pending gate. |
| Paired measurement/gate | PASS, exit0 |Eight normal scenarios, all throughput/p95 gates pass on the first attempt.48,000 measured +4,800 warm-up +six descriptive cold-first responses all match; six measured server lifetimes exit0 and collect children/ports. |
| Fresh Linux federation | NOT RUN |Dedicated fresh Linux outputs, two-instance DB/Redis, no host native artifacts. |
| Final pushed-SHA CI | NOT RUN |Final commit has not been pushed yet. |

The missing-copied-file candidate preflight stops before HTTP/data mutation. The initial SDK orchestration assertion incorrectly requires sampled controls to equal the native set; the real checker subsequently uses the same immutable twelve fresh side captures, retains all controls and verifies the exact required set. Both private harness failures remain preserved; neither is labeled a product regression or a fresh successful recapture.

No new compatibility exception is taken. The existing infrastructure-detail exception does not relax ordinary Date failures. SDK declaration differences remain BASELINE rather than being filtered out.

UI harness corrections retain the original emoji-image/textContent, production CSS class, async source-loading and transition-timing failures. Settled real DOM is compared without normalization or mocks. Only invalidated pairs are recaptured; the first two valid pairs are retained. Generated UI reports remove credential fields, with explicit sanitization revision provenance. These private harness failures are separate from product response parity.

## Paired cloud measurement

Run `9966eef8-61b3-418f-8797-13c4250d6065` uses the unchanged common runner: concurrency8,100 warm-ups,1,000 measured requests per scenario/side and three AB/BA/AB pairs. The eight scenarios are public Unicode Flash and profile-private Flash under anonymous, account and other-reader application viewers, plus empty-string Flash under anonymous/account viewers. Fixture aliases are preserved: the `owner` alias is not the author of the private-other Flash. No visibility or permission semantics are changed to create a benchmark.

All eight median throughput values improve19.06–80.87%; p95 decreases21.77–45.86%. p99 decreases17.50–50.08%; combined sampled RSS decreases12.45–18.52%. Native sampled RSS ranges from0.40% lower to2.41% higher. Acceptance concerns throughput/p95, rather than improvement in every metric. No exception, retry or favorable run selection is taken.

[Generated56-row metrics](flash-show-benchmark.csv) retain three-run medians, min/max, CV and paired deltas for latency, throughput and sampled TS/native/combined VmRSS. These bounded synthetic workloads are not production traffic, peak RSS or unique physical memory. Raw per-request latency samples are not retained. Latency ends at complete response bytes; semantic parsing/validation is outside latency and inside throughput, identically for both sides. The six first public requests after readiness are descriptive and do not establish system-wide cold caches.

Canonical full product source/build inventories match the fresh SDK capture. Actual HEAD `3e023eab324ccf799324299c21c64de73def4c58`, reviewed working-tree product source, configuration, capture/input hashes and mtimes, owned six Flash rows/two likes, bounded author/viewer/relationship/role/schema/policy digests and private storage remain fixed through the single measured attempt. Native executable SHA256 is `c75f9c7081127d9edf3008ce4a085f1b39acc090096400a41e6b7fa9f10f4010`. The persistent exclusive measurement lock is retained. The two readiness and six measured server lifetimes all exit0, collect their native children and release61912. Prepare/dry/measure/summary/gate commands all exit0 on their first attempts; readiness contributes32 warm-ups and two cold-first requests separately from the measured totals.
