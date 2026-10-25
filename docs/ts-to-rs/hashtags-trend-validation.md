# Complete native hashtags/trend

`hashtags/trend` executes its whole business operation in Rust, reusing the main Redis read lane, existing public Value serializer and server-local calendar. The handler is selected after all nine paired conditions pass. The public listener remains TS; the common Rust application continues to own authentication, validation, headers and response handling. No TS business callback, new connection, helper RPC, cache, crate, schema, frontend or slacc change is added.

Current and previous hourly rankings are read in that order by one nontransactional pipeline, with inclusive indices 0..10. First decoded tag occurrence determines output order; merging never sorts by averaged score or truncates the union to ten. Empty rankings skip the chart clock and I/O. Nonempty rankings sample a second clock and read twenty time-major chart slots, including repeated keys. Existing local `setMinutes` overflow and compatible DST resolution are preserved. The cumulative interval issue is inherited TS behavior, reported separately as [issue #61](https://github.com/mothue/misskey/issues/61), not corrected during migration.

Ranking command errors leave successful sibling results intact. Chart command errors retain undefined until Math.max gives NaN and JSON projects the affected array slot and maximum to null. This ordinary HTTP behavior is reproduced, rather than treated as an infrastructure-detail exception. New connection/timeout/command failures retain concrete internal causes and log only typed operations/kinds; embedded command errors retain their RedisError information without logging keys or messages. Existing callers alone keep their prior stage projection.

Redis members use UTF-8 replacement before deduplication and chart-key construction, without normalization. Numeric/prototype tags keep ranking order. Cardinalities follow actual redis-parser 3.0.0 digit-by-digit binary64 parsing: a single i64-to-f64 cast differs for large legal counts. Expected bits and JSON are generated from the installed JS parser; actual cached-HLL public HTTP confirms large-count behavior. Public null, omission, Number and object/array order remain meaningful.

## Fresh cloud evidence

Private root: `/workspace/native-queue/hashtags-trend`. `results.jsonl` records root command/cwd/exit/time/log; `candidate-verification.json` records delegated HTTP/browser/SDK commands, source/artifact hashes and lifecycle observations. The business baseline is isolated hybrid `abacb012d981123365abf0d9766662e74c79dca8`, where this body is TS, rather than pure-TS develop. Dedicated `notes-clips-runtime`, Redis DBs 4/3 and unique prefixes are used without a reset. DB-reset E2E uses separate `test-misskey`/Redis 14 and runs serially.

| Check | Result | Scope / limit |
| --- | --- | --- |
| TS-first / native public HTTP | PASS, exit 0 | Same 54 inputs: empty, ten tags, overlapping rankings,22-tag union, prototype/numeric/Unicode tags, supplied/invalid auth, GET/POST/open extras, cache headers, per-command WRONGTYPE and three large cached cardinalities. Full bodies/order/stable headers match; diagnostic UUID normalization only. |
| Binary-member public HTTP | PASS, exit 0 | Two actual TS/native pairs preserve BOM, NUL, supplementary characters and replacement/deduplication of selected malformed byte members. Finite corpus, not exhaustive malformed-UTF8 proof. |
| Rust | PASS | fmt, locked all-target clippy `-D warnings`, workspace 306 passed / 11 ignored. Extra style lint adds no warnings in the changed logic. Initial compile/style failures are retained as FAIL, not behavioral RED. |
| TS types / builds / API generation | PASS, exit 0 | Full backend typecheck, production/API generation and E2E builds; generated SDK source unchanged. Changed-file ESLint, SPDX and locale checks pass. |
| Related backend E2E | PASS, exit 0 | `endpoints.ts` and `api-visibility.ts`: 173 passed. Three new cases verify empty/cache/open fields, inclusive union/dedup/order and real HLL chart/max. Ranking dumps/TTLs restored; only owned chart keys removed. |
| SDK response validation | Coverage/parity PASS; type BASELINE | Fresh 36/36 source-derived coverage; 167 success pairs retained: 124 conforming and 43 identical inherited TS/native type discrepancies. Includes undeclared GET operations and wrong-HLL null values; these are not new successful type-conformance claims. |
| Product UI | PASS | Three real WidgetTrends page pairs, empty/ten/22tags; actual GET responses, DOM order/links and SVG charts match, no page errors. |
| Lifecycle | PASS | One observed live child, then zero after termination; trend and four core routes return 503 RUST_BACKEND_UNAVAILABLE. Host remains alive, no restart, host exit 0 and no live children afterwards. |
| Independent source/metrics review | PASS | Fresh-context source review plus independent recomputation of all 63 metric rows and nine gates, with zero discrepancies. Reviewed source pins and recorded source/build hashes match. Raw per-request latency samples were not retained. |
| Paired measurement / gate | PASS, exit 0 | All nine throughput/p95 conditions pass; 54,000 measured + 5,400 warm-up responses match actual TS captures, no errors or partial runs. Three immutable first attempts; no favorable rerun. |
| Fresh Linux federation | PENDING | No previous-slice result is substituted. |
| Final pushed-SHA CI | PENDING | No earlier/intermediate commit result is substituted. |

Source-only null-exec controls do not establish a real Redis transport case or bundled public-error contract. No new infrastructure-fault matrix or wire-validation unit is added. Security rules and ordinary response contracts retain strict parity; no new compatibility exception is taken.

## Paired cloud measurement

Three independent prefixes cover empty, ten-tag and 22-tag rankings, each with anonymous GET/POST and scoped authenticated POST. The unchanged runner uses three AB/BA/AB pairs, concurrency 8, 100 warm-ups and 1,000 measured requests per scenario/side. Live business key sets/types/no-expiry/DUMP hashes, role/meta state, source/build/config/HEAD and expected responses remain fixed. HLL cached cardinalities are primed symmetrically before freezing. A single UTC ranking hour and real wall/monotonic clocks are guarded; constant 32-hour chart coverage allows ten-minute transitions without changing responses. No clock injection, business cache or in-pair reseeding is used.

Actual totals are 54,000 measured + 5,400 warm-ups + 18 cold readiness samples; all succeed. All nine gates pass: median throughput improves 5.46–53.50% and median p95 decreases 15.30–39.97%. [Generated 63-row metrics](hashtags-trend-benchmark.csv) retain variation and sampled TS/Rust/combined VmRSS, not peak or unique physical memory. Raw request-level latency samples are not retained. These are dedicated bounded workloads, not production traffic. Run IDs are `3271e7af-6b1f-4285-ad72-952d3d5890a6`, `b4a612b4-b934-4818-b246-9f79dc05a8ac` and `ac290275-35d6-4bfb-81a2-f962b94f99a7`. All 18 server sides and 36 host/child PIDs are collected; port 61912 is released.
