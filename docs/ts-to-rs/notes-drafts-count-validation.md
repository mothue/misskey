# Native notes/drafts/count candidate — activation held

The candidate implements the complete owner-scoped count through the existing PostgreSQL read lane. It uses the authenticated user ID as the only predicate. It does not inspect draft content, visibility, scheduling or caller-supplied filters, and adds no service, schema, connection route, helper RPC or formatter. Shared authentication, suspension/moved checks, scope authorization and validation still precede business execution.

The response remains a JSON number, including zero. PostgreSQL int8 counts retain the existing binary64/JS Number rounding beyond the safe-integer boundary; they are not strings or bigint. Missing, multiple, null, negative or non-integral decoded count results do not silently become zero.

## Decision

**FAIL — measured activation gate. Keep the complete TS body.** All six scenarios improved median throughput, but the nine-draft account case increased median p95 by 3.71% while throughput improved by 19.52%. The five other scenarios improved both measures. No performance tolerance or dependency exception was taken. This measurement does not establish a product correctness bug or its performance cause.

The handler is compiled and tested, but excluded from the activation allowlist and TS callback rejection guard. This selects the existing TS operation; no runtime error fallback, retry or automatic restart was added. [Generated metrics and variation](notes-drafts-count-benchmark.csv) retain all six scenarios.

The unmodified benchmark runner used three AB/BA/AB pairs, concurrency 8, 100 warm-ups plus 1,000 measured requests per scenario per side. All 36,000 measured requests and 3,600 warm-ups matched their baseline status, response and stable headers. Source, artifacts, dedicated fixtures and configuration stayed fixed. Baseline: TS business at `8fd8819dae8dd6ef08db569b5137aef2a57fd948`, including the same shared native execution infrastructure and compatible-session reuse change. This is a hybrid comparison; the separate pure-TS develop measurement is recorded in [queue-validation.md](queue-validation.md).

## Fresh cloud checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Original TS HTTP baseline | PASS, 27 cases | Captured before integration: zero/single/multiple counts, foreign owners, extra caller filters, normal/scoped tokens, moved/suspended users and invalid input. |
| Native HTTP differential | PASS, 27 cases | Same inputs/status/body/stable headers; only per-request diagnostic UUIDs normalized. |
| Global SDK response validation and parity | PASS coverage/parity; SDK BASELINE | 51 actual paired cases covered all 30 temporarily selected endpoints: 44 SDK PASS, seven identical inherited violations tracked in issue #55, no new failures. |
| Related backend E2E | PASS, exit 0 | `note.ts` and `api-visibility.ts`: 147 passed. The count test isolates owner rows and removes its fixtures in finally. |
| Rust workspace | PASS, exit 0 | 258 passed, 11 ignored. Three inline count tests include int8/Number rounding. Ignored tests are not counted as successes. |
| Rust fmt / clippy | PASS, exit 0 | Workspace fmt and all-targets clippy with `-D warnings`. |
| Backend type checks | PASS, exit 0 | Source, E2E and federation test types. |
| API type generation/build | PASS, exit 0 | Dedicated explicit config; no generated SDK source changes. |
| Independent manual review | PASS | Fresh-context reviewer compared the original TS operation, shared gates and count conversion. |
| Rust child lifecycle | PASS, exit 0 | The new count/core APIs return 503 after child termination, host survives, no restart, owned processes collected. |
| Performance activation gate | FAIL, exit 1 | Nine-draft account p95 +3.71%; successful measurement completion is not gate success. |
| Local federation for this candidate | SKIPPED | No activation after the failed performance gate. Previous shared-pool federation results are not reused as this candidate's success. |
| Final pushed-commit CI | PENDING | Check the actual pushed SHA; no final CI success claimed here. |

The first private benchmark preparation attempt failed its copied fixture-name guard before any benchmark requests. Correcting the dedicated fixture guards and draft-row digest allowed preparation, readiness and the paired measurement to complete. This setup failure is retained with exit 1 and is not a product regression or a successful measurement.

Commands, exit codes, sanitized logs, captures, reviews and source/binary provenance: `/workspace/native-queue/notes-drafts-count/`. Runtime DB: `notes-drafts-count-runtime`, Redis DB2/reactions DB1 with separate prefixes. Backend E2E uses the separate `test-misskey` fixture, with serial DB resets. No shared or production services were used. This backend-only change does not change UI behavior; product browser checks are SKIPPED.
