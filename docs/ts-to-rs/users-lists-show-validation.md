# Native users/lists/show candidate

The complete detail read reuses the existing UserList projection and bounded membership reader. It adds no schema, connection route, crate, TS callback or helper RPC. Shared authentication, scope checks and input validation remain authoritative.

Authenticated requests with omitted/default-false `forPublic` select only the caller's list. Anonymous requests and `forPublic: true` select only public lists. Missing and inaccessible lists retain the same `NO_SUCH_LIST` 400 result and original ID. No owner-host restriction is introduced.

The original operation order is retained: eligible list lookup, then (only for explicit `forPublic: true`) favorite COUNT followed by the viewer's EXISTS query or anonymous false, then membership lookup and date/name packing. Favorite fields are appended only in that explicit public mode, including zero and false; all other requests omit both fields. Counts are validated finite nonnegative integral binary64 numbers, preserving PostgreSQL int8/TS Number rounding. There is no bigint/string conversion or safe-integer cap. Base dates, UTF-16 names, membership order and required fields use the existing formatter.

This is an optional-credential endpoint. The existing shared metadata allows valid suspended/moved callers to read public data here; the TS business baseline returns 200 for those cases. This differs from credential-required endpoints' suspension gate, and is retained without changing authorization policy.

## Activation decision

**PASS — measured activation gate.** All six scenarios improve median throughput (+10.30% to +59.27%) and p95 (-14.44% to -41.25%). The complete operation is selected for native execution; acceptance remains pending fresh federation and final pushed-commit CI.

The unchanged runner used three AB/BA/AB pairs, concurrency 8, 100 warm-ups plus 1,000 measured requests per scenario per side. All 36,000 measured requests and 3,600 warm-ups matched their TS business responses. Source, artifacts, dedicated fixtures and config remained fixed. Baseline: TS business at `02c0f8a4b46c956e73611052bb9d3bccd150cb18`, with the same hybrid common runtime. [Generated metrics and variation](users-lists-show-benchmark.csv) retain all scenarios.

## Fresh cloud checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Original TS HTTP baseline | PASS, 57 cases | Captured before integration at `02c0f8a4b46c956e73611052bb9d3bccd150cb18`: ownership/public modes, field omission, zero/false/liked states, missing/inaccessible lists, scoped and optional-auth behavior, validation. |
| Native HTTP differential | PASS, 57 cases | Same inputs/status/body/stable headers; only per-request diagnostic UUIDs normalized. |
| All native SDK response validation / parity | PASS coverage/parity; SDK BASELINE | 58 paired cases cover all 31 locally selected endpoints: 51 SDK PASS, seven identical inherited violations in issue #55, no new failures. |
| Related backend E2E | PASS, exit 0 | `users.ts` and `api-visibility.ts`: 261 passed, one skipped, ten TODO. Existing list checks also pass after the shared typed Vec-return refactor. |
| Rust workspace | PASS, exit 0 | 268 passed, 11 ignored. Ten inline UserList tests cover both operations, including favorite field presence and int8 rounding. |
| Rust fmt / clippy | PASS, exit 0 | Workspace fmt and all-targets clippy with `-D warnings`. |
| Backend types / API type generation | PASS, exit 0 | Source, E2E and federation types. Explicit test config; no generated SDK source changes. |
| Independent manual review | PASS | Fresh-context review of original TS, query order, eligibility, types, old list behavior and documentation; no findings. |
| Rust child lifecycle | PASS, exit 0 | A valid detail request and native core requests return 503 after child termination; host survives, no restart, owned processes collected. |
| Local product browser | PASS, exits 0 | Actual settings/public detail pages on TS and native versions: omitted, liked, anonymous and zero-like fields, matching count/icon and response, zero page errors. Eight screenshots retained; external HTTP resources blocked. |
| Measured activation gate | PASS, exit 0 | All six paired scenarios improve throughput and p95; zero response failures. |
| Local federation | PENDING | Fresh Linux container build/two-instance suite required if the gate passes. |
| Final pushed-commit CI | PENDING | Check the actual final pushed SHA. |

Commands, exit codes, sanitized logs, captures, reviews and provenance: `/workspace/native-queue/users-lists-show/`. The dedicated UserLists DB `users-lists-list-runtime` and Redis DB10/reactions DB15 are reused serially with explicit settings and four favorite fixtures; no reset runs concurrently. Backend E2E uses the separate `test-misskey` fixture. No shared or production service is used.

The first benchmark preparation attempt failed a fixture precondition that assumed only the six seeded users. The dedicated DB also contained a normal `system.proxy` account. The guard now permits pre-existing system accounts and freezes the complete actual fixture snapshot, still rejecting any changes during measurement. The failed preparation made no benchmark requests and remains recorded with exit 1; no data was reset.
