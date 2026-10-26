# Historical native notes/state candidate — activation held

This is the original held revision. The subsequent [typed-row revision](notes-state-typed-read-validation.md) passes all six fresh paired performance conditions; the original failure and metrics below remain historical evidence.

The candidate implements the entire `notes/state` business operation in Rust: the existing note lookup, per-viewer favorite count, and per-viewer thread-muting count. It reuses the existing native DB adapter/read lane and introduces no new connections, TS business callback or helper RPC. Endpoint credentials, scope and input checks remain in the shared API runtime.

Only `id` and nullable `threadId` are projected. The null-only fallback preserves an empty or Unicode thread ID. Unrelated note content is not decoded. The existing `MiNote`/criteria `EntityNotFoundError` 500 result is retained, rather than changed to a new error contract. The same result was reproduced on pure-TS develop `d875b7e` and is tracked separately in [mothue/misskey#56](https://github.com/mothue/misskey/issues/56).

## Decision

**FAIL — measured activation gate. Keep TS execution.** Five native-account scenarios improve, but the scoped application-token reply scenario regresses: median throughput -37.6%, p95 +47.1%. Token type and note shape differ between scenarios; the cause has not been established. No infrastructure or dependency exception was taken. The handler is compiled and tested but absent from the activation allowlist; the TS callback guard likewise does not reject it. This is selection of the existing TS body, not an error fallback.

The existing unmodified runner used three AB/BA/AB pairs, concurrency 8, 100 warm-up plus 1,000 measured requests per scenario per side. All 36,000 measured requests and 3,600 warm-ups matched their TS baseline responses. Successful completion of a measurement does not mean the performance gate passed. Baseline: TS business body at `f597c92ea699109f82ffc9b9cbc116cc0ad35536`, with the existing hybrid common runtime. The dedicated fixture, config, product source and artifacts were frozen during measurement.

[Generated metrics and variation](notes-state-benchmark.csv) preserve the full six-scenario result, including the regression.

## Fresh cloud checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Original TS HTTP baseline | PASS, 44 cases | Before native selection; success flags, viewer/scope, null/empty/Unicode thread IDs, ignored payload, validation and missing-note error. |
| Native candidate HTTP comparison | PASS, 44 cases | Same inputs, status, body and stable headers; only per-request diagnostic UUIDs normalized. |
| SDK conformance / semantic comparison | PASS coverage and parity; SDK BASELINE | Fresh capture of all selected native endpoints during the experiment: 31 cases, 24 SDK PASS, seven identical inherited SDK violations, zero new failures. |
| Rust workspace | PASS | 250 tests passed; eight ignored are not successes. |
| Rust clippy | PASS, exit 0 | Workspace/all-targets, `-D warnings`. |
| Rust fmt | PASS after correction | Initial import ordering check failed; rustfmt applied before final candidate build. |
| Backend type checks / API type generation | PASS, exit 0 | Source, E2E and federation test types; no SDK generated source diff. |
| Related E2E | PASS, 143 tests | Initial malformed-fixture/notification race failed; direct SQL fixtures with finally cleanup passed. Both the new state fixture and earlier malformed partial-note fixtures are isolated. |
| Independent code review | PASS, no findings | Source and fixture revision reviewed independently. |
| Activation performance gate | FAIL | Scoped application-token reply case regresses; all six must pass. |
| Local federation for this candidate | SKIPPED | Candidate is not activated after the failed performance gate. Earlier slice results are not reused as this candidate's success. |
| Pushed candidate CI | PASS at `0719dfae0eff3c183f01329eb49d7c3e5409ca5a` | All 18 executed workflows succeeded; Storybook was skipped. Later source revisions require their own final-SHA check. |

The earlier validation-tools commit `5ba2bcc` had one backend E2E matrix failure with the same TS `No default value` notification stack. The fixture correction is a test repair, not a product behavior change. Both backend E2E matrices and federation subsequently passed at `0719dfae`, which contains the fixture correction.

Commands, exit codes, logs, source/binary provenance and reviews: `/workspace/native-queue/notes-state/`. Dedicated DB: `notes-state-runtime`; main Redis DB 6, reactions DB 5. No shared or production service was used.
