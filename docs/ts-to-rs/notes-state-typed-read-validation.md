# Native notes/state typed-read revision

This is a new implementation revision of the existing complete, performance-held candidate. The original [six-condition failure](notes-state-validation.md) and [metrics](notes-state-benchmark.csv) remain historical evidence, including the scoped application-token reply regression. Removing the JSON row boundary does not establish the cause of that earlier failure.

The three original independent reads remain: a note lookup, followed by concurrent favorite and thread-muting counts on the existing read lane. Typed PostgreSQL String, Option<String> and bool results replace JSON construction/parsing. No combined snapshot, cache, connection, schema, RPC or TS business callback is added. Actual TypeORM metadata for all three tables is exported at bootstrap. A local projection validator checks identifiers, duplicates and consumed kinds while deliberately permitting supported unconsumed fields; the existing shared exact-layout validator is unchanged.

The response retains isFavorited then isMutedThread as booleans. Only null triggers the threadId fallback; empty and Unicode strings remain distinct. Favorite criteria use the loaded note.id and actual viewer. Unrelated note contents are not hydrated or decoded. The source has no visibility gate here; none is added. The existing missing-note EntityNotFoundError500 remains byte-identical and is already tracked in mothue/misskey#56. Query/row errors retain concrete causes and bounded operation/kind reporting.

## Fresh cloud evidence

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Actual TS callback source oracle | PASS | 13 cases; 11 success and two missing errors. TypeORM count clears take/limit. Source-harness model-import diagnostic is retained separately; not claimed as HTTP. |
| Fresh current TS public HTTP baseline | PASS | 46 cases: 37 success, five400, one each401/403/405/500. Original44 cases and all six benchmark inputs retained unchanged. |
| Native candidate public HTTP | PASS | All46 actual responses and stable headers match; only diagnostic UUID normalized on comparison copies. Source/build/dataset pins and fresh independent review pass. |
| Rust workspace | PASS | 328 passed,11 ignored; initial fmt import ordering failure preserved, corrected check passes; locked clippy all-targets warnings denied passes. |
| TS typecheck, API generation and production build | PASS | Exit0; generated SDK source unchanged. |
| Closest TS units | PASS | Five plus two tests. |
| Related backend E2E | PASS | note69,api-visibility78 and final-selection endpoints103;250 first-run passes across three files. |
| Full backend unit suite | PASS | First fresh full command:80files951passed2skipped,exit0,157.793s. Dedicated Meili1.49 health/teardown pass; owned resources0. |
| Whole-native SDK comparison | PASS parity/coverage; SDK BASELINE | Fresh39 native coverage plus two TS controls;285 success pairs229SDK PASS56 identical inherited declaration gaps;115 error pairs match. |
| Product UI | SKIPPED partial | Actual TS detail and state200 captured; first note-menu label wait timed out before a native visual pair. Failure screenshot/log retained. Automatic achievement and system.proxy setup writes occurred; no favorite/mute/note action or SQL repair. Exact original-three-user reconstruction and pinned auxiliary-row proof preserve the measured domain. |
| Rust child-loss lifecycle | PASS | Ready39, child1→0, state and four original APIs503; no automatic restart, hostexit0, children0/portreleased. First precheck stopped before startup on auxiliary fixture addition and remains archived. |
| All-six paired performance gate | PASS | First run22959f7c-b7b5-4aee-8798-7cb256cf73bf;36k measured,3.6k warm-ups,six descriptive first requests. All six strict median RPS/p95 gates pass. |
| Independent batch review | PASS source/runtime/metrics | Source, held core, actual HTTP/SDK, bounded UI fixture growth, honest visual skip and lifecycle independently reviewed; all42 metric rows and six strict gates recomputed. |
| Fresh Linux federation | PENDING | Final committed selection required before freeze/build/run. |
| Pushed final-SHA CI | PENDING | Earlier published b4a1082 CI is not reused. |

Private command/exit/log/provenance record: /workspace/native-queue/notes-state-typed-read/. Dedicated runtime DB notes-state-runtime, Redis6/reactions5; reset tests use separate test-misskey and Redis14, serialized. The working selection passed the first measured gate; fresh federation and final pushed-SHA CI remain pending at this documentation checkpoint.

## First paired performance gate

The unchanged six original inputs, including the previously failing scoped application-token reply, are measured against the current TS business baseline at b4a1082590faea9c00af3afb3df7b9aa41cb016b. This hybrid baseline uses the existing Rust common API runtime; it is not pure-TS develop. The substantive typed-row candidate is HEAD e1622a9c2efc389ca30f0a4ec407e88b2c5d6bd8 plus its reviewed local selection, with native executable SHA256 f345f66d41f4d7e3a83502ffd550c8463618443613672f24044f4580ec265bde.

First prepare and exact dry-run pass. The dry-run has24 warm-ups,two descriptive first requests and zero measured requests. Run22959f7c-b7b5-4aee-8798-7cb256cf73bf completes in77.58s, using C8,100 warm-ups and1,000 measured requests per condition/side in three AB/BA/AB pairs. All36,000 measured requests and3,600 warm-ups retain exact status, two-boolean body and stable headers. Six first requests after readiness are descriptive and do not establish system-wide cold caches. The persistent exclusive measurement lock and all first attempts remain preserved. No exception, favorable rerun or omitted condition is taken.

All six median throughput values improve7.43–82.20%; p95 decreases19.89–51.86%. The originally held scoped reply case now improves7.43% throughput with19.89% lower p95 against this current paired baseline. These are bounded synthetic measurements. They do not isolate the cause of the older failure or attribute all improvement to JSON removal across different historical source versions. The original six-condition failure and CSV remain unchanged.

[Generated42-row metrics](notes-state-typed-read-benchmark.csv) retain p50/p95/p99, throughput and sampled TS/native/combined VmRSS medians, ranges, population SD/CV and paired deltas. VmRSS is sampled rather than peak or unique physical memory. Raw latency samples are not exported. Latency ends at complete response bytes; semantic parsing is outside latency and inside throughput equally on both sides.

The source, build, canonical SDK inventories, HEAD, old historical artifacts, configuration, inputs and dedicated data are fixed through the single attempt. One legitimate baseline UI bootstrap created the known system.proxy account, and frontend setup appended an achievement to user_profile. No SQL repair or deletion is performed. The original three-user rows reconstruct their exact pre-UI raw hash except the predeclared owner activity field; the exact additional account is separately pinned. The complete four-user raw state freezes before prepare and remains unchanged across dry-run and every measured side. All original notes/favorites/thread-mutes/roles/scopes/schema remain exact. This is dedicated fixture preparation, not a public API compatibility exception.

All two dry-run and six measured server lifetimes exit0, collect their native child and release61912. The UI first baseline menu timeout and initial lifecycle guard stop are preserved separately; no native visual pair was completed. Cause of the menu presentation failure remains unconfirmed, and no frontend product fix or TS product-bug claim is made.
