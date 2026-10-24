# Native notes/clips candidate — activation held

The complete business operation reuses the existing native UserLite formatter and PostgreSQL read lane. It checks note existence, selects public clips containing that note and preserves the database result order without adding sorting. It does not add a note-visibility restriction absent from the TS endpoint. Authentication, optional credentials, validation and public response handling remain in the shared API runtime.

Anonymous `isFavorited` and non-owner `notesCount` remain omitted. Nullable description and last-clipped time remain null; false and zero remain present. Epoch-zero dates remain ISO strings. Counts follow the existing JS Number conversion, rather than becoming JSON strings or bigint. Public clip authors are batch-packed with the existing formatter, and owner note counts/favorite counts use the original selection and per-clip await order. No schema, new connection route, TS business callback or helper RPC was introduced.

## Performance decision

**FAIL — measured activation gate. Keep TS execution.** The original candidate failed the measured activation gate for both scoped application-token scenarios. Median throughput regressed by 55.6% / 55.1%; p95 increased by 99.4% / 108.3%. This failed attempt is retained under `/workspace/native-queue/notes-clips/attempt-1-pre-pool-fix/` and is not counted as an activation success.

An observer outside measured request phases found roughly 2,000–2,600 new PostgreSQL sessions per 1,100 native app-token requests, versus 30–220 for their TS baseline. Both auth and business share the configured source budget; uncached app-token reads and last-use updates require opposite immutable session modes. At full capacity the pool immediately replaced healthy wrong-mode idle sessions even while compatible sessions were in flight.

The pool change grants the FIFO head one fixed maximum 2ms opportunity per checkout for a compatible lease to return. It preserves the source connection cap, original deadline, cancellation and shutdown, expiry/timer ownership and immutable read-only/write modes. A full pool with only an incompatible idle session still progresses immediately. The real-PostgreSQL regression failed before the change (the writable session PID changed) and passed afterward; two additional healthy-PG tests cover bounded fallback and max=1 mode isolation. Independent manual review found no issues. The reviewed pool change reduced scoped-token session creation to 31–141 per 1,100 native requests. A fresh three-pair measurement completed all 36,000 measured and 3,600 warm-up requests with matching responses, but the large scoped-token scenario still failed: throughput -16.53%, p95 +39.42%. Large-owner throughput also changed by -0.08% (p95 -14.88%); no tolerance exception was taken. The handler remains compiled and tested but excluded from native selection and the TS callback guard. No runtime failure fallback was added. [Full medians and variation](notes-clips-benchmark.csv) include every scenario.

## Fresh checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Original TS HTTP baseline | PASS, 36 cases | Empty/multiple clips, anonymous/account/scoped-app viewers, public/private selection, omission/null/false/zero, dates, errors and input validation. Captured before native selection. |
| Native HTTP comparison after pool change | PASS, 36 cases | Same inputs, status/body and stable headers. Only per-request diagnostic UUIDs normalized. |
| All native response coverage and parity | PASS | 62 paired cases covered all 30 temporarily selected endpoints before the performance hold; zero new differences. SDK conformance: 55 PASS, seven identical inherited BASELINE results tracked by issue #55. After restoring TS selection, the fresh 62-case comparison still matches and covers all 29 active native endpoints. |
| Rust fmt / clippy | PASS, exit 0 | Workspace format check and all-targets clippy with `-D warnings`. |
| Rust workspace | PASS, exit 0 | 255 passed, 11 ignored; the three new explicitly ignored dedicated-PG tests were additionally run and passed. |
| Related backend E2E | PASS, exit 0 | Three files: 242 passed, two TODO. Dedicated test DB/Redis; no concurrent reset. |
| Backend source/E2E/federation type checks | PASS, exit 0 | Run for the candidate; the subsequent pool-only Rust change does not alter TS declarations. |
| API type generation/build | PASS, exit 0 | Explicit dedicated config; generated SDK source is unchanged. Native executable rebuilt after the pool change. |
| Independent review | PASS | Separate fresh-context reviews of the endpoint and connection-pool dependency; no remaining findings. |
| Child lifecycle | PASS, exit 0 | New endpoint plus existing native note/core APIs return 503 after Rust termination; host survives without automatic restart. All owned processes collected. |
| Measured activation gate | FAIL | Large scoped-token case regresses; native activation is held. Both measurement attempts are retained. |
| Local federation | PASS for shared pool dependency, exit 0 | Fresh Linux container build and dedicated two-instance suite: 92 passed, 13 skipped. Source and HEAD stayed unchanged; owned containers/volumes removed. The clips handler itself remains held. |
| Pushed candidate CI | PASS at `8fd8819dae8dd6ef08db569b5137aef2a57fd948` | All 18 executed workflows succeeded; Storybook was skipped. Later revisions require their own final-SHA check. |

Commands, exit codes, sanitized logs, captures, reviews and binary/source provenance are retained in `/workspace/native-queue/notes-clips/` and `/workspace/native-queue/pool-mode-reuse/`. Runtime fixtures use dedicated `notes-clips-runtime`, Redis DB4/reactions DB3. E2E uses the separate `test-misskey` fixture. No shared or production service is used. This backend-only change has no UI behavior change; product browser checks are SKIPPED.
