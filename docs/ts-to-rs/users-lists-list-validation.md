# Native users/lists/list candidate

The complete operation uses the existing PostgreSQL read lane and shared API authentication, scope authorization, validation and response handling. It adds no connection route, schema, crate, TS business callback or helper RPC. One internal UserList module owns the base projection for reuse by a later complete list operation.

Omitted `userId` with credentials selects the caller's public and private lists. Explicit `userId`, including the caller's own ID, selects public lists only. Explicit missing users retain `NO_SUCH_USER`; any non-null host retains `REMOTE_USER_NOT_ALLOWED`; anonymous omission retains the endpoint's `INVALID_PARAM` with its original ID. Eligibility is applied before list content and membership decoding. No user-profile packing or unrelated note content is decoded.

Membership reads run in bounded batches of up to 64 eligible lists before constructing their DTOs. Each correlated per-ID scan retains the original predicate and full membership projection, with no member sorting or deduplication. Only per-list completed arrays are ordered by their input ordinal, preserving Promise.all response order. IDs remain bound parameters; row count, ordinal, ID and array shape are checked before assigning memberships to lists. This is an internal statement-size bound, not a public count limit. The selected list and membership columns match the original entities to avoid introducing an index-only ordering change. Membership order follows each original per-ID scan. Dates use the existing configured ID parser and UTC ISO formatter. Names retain UTF-16 content, including empty strings from stored rows; false and empty arrays remain present. The base result omits show-only liked fields.

## Activation decision

The original per-list-query candidate failed: large scoped-app throughput -20.20%, p95 +5.22%, while the five other scenarios passed. That attempt is retained under `/workspace/native-queue/users-lists-list/attempt-1-per-list-queries/`. The second paired measurement of the reviewed bounded-batch change passed all six scenarios: median throughput +18.62% to +85.61%, p95 -21.99% to -48.09%. All 36,000 measured requests and 3,600 warm-ups matched their TS responses. The same dedicated fixture, source, artifacts and configuration stayed fixed through three AB/BA/AB pairs; concurrency 8, 100 warm-ups plus 1,000 measured requests per scenario per side. Baseline: the TS business body at `fb102ecbcc23681020c6e895e94cdc6377048788`, with the same hybrid common runtime. [Full metrics and variation](users-lists-list-benchmark.csv) retain every scenario.

**PASS — measured activation gate.** The complete operation is selected for native execution. Fresh federation and final pushed-commit CI at `02c0f8a4b46c956e73611052bb9d3bccd150cb18` passed. Local product browser checks also passed. Source review and HTTP parity alone were not counted as completion.

## Fresh cloud checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Original TS HTTP baseline | PASS, 49 cases | Captured before integration. Own/explicit-self/public eligibility, empty and larger lists, membership order, empty/Unicode names, anonymous/account/scoped tokens, target and validation errors. |
| Native HTTP differential | PASS, 49 cases | Same inputs/status/body/stable headers; only per-request diagnostic UUIDs normalized. |
| All native SDK response validation / parity | PASS coverage and parity; SDK BASELINE | 58 paired captures cover all 30 locally selected endpoints: 51 SDK PASS, seven identical inherited violations in issue #55, no new failures. |
| Related backend E2E | PASS, exit 0 | `users.ts` and `api-visibility.ts`: 259 passed, one skipped, ten TODO. Dedicated DB/Redis with serial resets. New tests retain private-list eligibility, exact DTO/member order and the three distinct endpoint errors. |
| Rust workspace | PASS, exit 0 | 265 passed, 11 ignored. Seven inline tests cover selection, errors, UTC dates, UTF-16, member order/duplicates and required fields. |
| Rust fmt / clippy | PASS, exit 0 | Workspace fmt and all-targets clippy with `-D warnings`. |
| Backend type checks / API type generation | PASS, exit 0 | Source, E2E and federation types. Explicit dedicated generation config; no generated SDK source changes. |
| Independent manual review | PASS | Fresh-context review of original TS, schemas, selection, shared gates and projection; no findings. |
| Rust child lifecycle | PASS, exit 0 | New list/core APIs return 503 after Rust termination, host survives, no automatic restart, owned processes collected. |
| Measured activation gate | PASS, exit 0 | All six scenarios improve throughput and p95 after bounded membership batching. The first attempt remains FAIL. |
| Local federation | PASS, exit 0 | Fresh Linux container build and dedicated two-instance suite at `2de2e66b27cb0bb12eb557d746059b2c556c7657`: 92 passed, 13 skipped. All 1,283 product/test source files and HEAD stayed unchanged; owned containers/volumes removed. |
| Pushed slice CI | PASS at `02c0f8a4b46c956e73611052bb9d3bccd150cb18` | All 18 executed workflows succeeded; Storybook was skipped. Later revisions require their own final-SHA check. |
| Local product browser | PASS, exits 0 | Existing frontend build and system Chromium via Playwright: actual `/my/lists` and anonymous public-list page on TS and native versions. Same rendered IDs/order (three own / two public), zero page errors, four screenshots retained. External HTTP resources blocked. |

Commands, exit codes, sanitized logs, source/binary provenance, capture pairs and reviews: `/workspace/native-queue/users-lists-list/`. Dedicated runtime DB: `users-lists-list-runtime`; main Redis DB10 and reactions DB15 with separate prefixes. Backend E2E uses the separate `test-misskey` fixture. No shared or production service is used.

The earlier browser availability assessment checked the wrong frontend build path; existing root `built/_frontend_*` assets and system Chromium were available. Initial screenshot checks were limited by expected onboarding, and two harness attempts failed waiting for the unhandled Later confirmation. After handling that normal UI dialog and waiting for rendering to settle, both TS and native checks passed. These setup failures are retained, not treated as product failures or silent passes.
