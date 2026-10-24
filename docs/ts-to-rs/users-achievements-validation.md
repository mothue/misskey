# Native users/achievements

The whole profile-achievement read is implemented in an internal Rust module and selected for native execution after its six-case measured gate passed. It performs one read-lane query and adds no schema, crate, cache, helper RPC or TS business callback. Fresh federation passed; final acceptance remains pending final pushed-commit CI.

The existing optional-auth metadata is unchanged: anonymous, account and valid unrelated-scope app tokens can read the same target. No new suspended/moved-viewer, target visibility, owner or permission gate is introduced. Shared authentication and input validation run before the body.

## Response and type preservation

The SQL wrapper distinguishes an absent profile, an absent selected field and a present JSON null. The actual TS handler returns the JSONB field unchanged; it does not validate the stored value as an enum array. Rust therefore gives this field an explicitly opaque JSON boundary rather than inventing a narrower storage contract. It preserves array order, duplicates, unknown names, extra/missing fields, number/string/null distinctions and nonarray roots. Counts and dates are not parsed or derived. Number parsing retains JS binary64 rounding, including values above the safe-integer range; there is no integer, bigint or timestamp conversion. Unicode uses the shared UTF-16 value representation.

The common response helper preserves root JSON null as an empty 204 response and root strings as quoted JSON strings with `text/plain; charset=utf-8`. Normal arrays use the existing JSON response. The generated SDK still expects Achievement[]; intentionally malformed storage values remain explicit identical-baseline type violations, not new native response failures.

A missing profile retains the ordinary `INTERNAL_ERROR` 500 and exact `EntityNotFoundError` message/criteria through the existing legacy-error helper. This is not an infrastructure-fault exception. It was reproduced through public HTTP in pure TS at `d875b7e3c737b7f9f75331fffb691f6a2b97f055`, with zero Rust children, and reported in [mothue/misskey#57](https://github.com/mothue/misskey/issues/57). This migration does not fix it.

## Fresh cloud checks

| Check | Result | Evidence / limit |
| --- | --- | --- |
| TS-first public HTTP | PASS, 54 cases, exit 0 | Before integration at `b0bcddc669c743a344e6119a872949bbe6981adf`: 34 HTTP 200, three 204, two 401, five 500 and ten 400 responses. Includes normal/empty/full/reversed/duplicate arrays, raw JSON types, scopes, moved/suspended viewers/targets, IDs and input boundaries. |
| Native HTTP differential | PASS, 54 cases, exit 0 | Exact status/body/stable-header parity. Only per-request diagnostic UUIDs normalized. |
| Pure-TS missing-profile reproduction | BASELINE, exit 0 | Public anonymous request at `d875b7e` returns the same 500 error class/message. Request-specific diagnostic ID is not a fixed contract value. |
| SDK response validation / parity | PASS coverage/parity; SDK BASELINE | 72 paired cases cover all 32 selected native endpoints: 53 SDK PASS, seven inherited violations from issue #55 and twelve deliberate malformed-achievement storage fixtures with identical TS violations; zero new failures. |
| Related backend E2E | PASS, exit 0 | `users.ts` and `api-visibility.ts`: 267 passed, one skipped, ten TODO. Three new tests preserve raw types/order/auth and ordinary missing-profile errors; modified fresh profiles are restored in finally. |
| Rust workspace | PASS, exit 0 | 275 passed, 11 ignored. Three inline tests cover opaque payload boundaries, ordering/duplicates, number types/rounding and missing-profile error formatting. |
| Rust fmt / clippy | PASS, exits 0 | Workspace fmt and all-targets clippy with `-D warnings`. |
| Backend types / API type generation | PASS, exits 0 | Source, E2E and federation types; explicit dedicated config; no generated SDK source differences. |
| Independent manual review | PASS | Fresh-context source and final evidence review found no defects. All 42 CSV rows, six gates, source hashes, SDK/UI/E2E evidence and documentation independently verified. |
| Child lifecycle | PASS, exit 0 | Valid achievement/core requests return 503 after child termination; host survives, no restart, owned processes collected. |
| Actual product UI | PASS, six paired scenarios, exits 0 | Existing achievement panel, anonymous/account, one/duplicate/empty payloads. Matching API bodies and rendered counts, twelve screenshots, zero page errors. The UI deduplicates in enum order; the server retains the original duplicate array. |
| Paired measurement | PASS execution and activation gate | All 36,000 measured requests and 3,600 warm-ups matched the captured TS bodies/status/headers. All six median throughput/p95 gates pass. |
| Shipping lint / SPDX / locale | PASS, exit 0 | Configured changed-file checks; unchanged out-of-scope historical test-file lint is not claimed fixed. |
| Fresh local federation | PASS, exit 0 | Fresh Linux-native build and two-instance suite at `c58f03c90760093fd6200331345279c334b62af3`: 92 passed, 13 skipped. HEAD and all 1,285 product/test files stayed unchanged; dedicated containers/volumes removed. |
| Final pushed-commit CI | PENDING | Check the final actual pushed SHA. |

Three AB/BA/AB pairs use the unchanged common runner, concurrency 8, 100 warm-ups and 1,000 measured requests per scenario per side. The six cases are anonymous/account/unrelated-scope app tokens against identical empty and all-current-enum target payloads. Enum names are loaded from the authoritative TS source, not a fixed count. Observed median throughput improves 27.53–121.70%; p95 falls 28.02–52.00%. Run `0d604319-3fb4-4d00-95da-8bc66b5a7efe` completed in 54.93 seconds. This is a bounded dedicated workload, not production traffic. [Full metrics and variation](users-achievements-benchmark.csv).

Evidence: `/workspace/native-queue/users-achievements/`, including command/cwd/exit-code ledger `results.jsonl`, sanitized `logs/`, full private captures, SDK diagnostics, review, twelve UI screenshots, source/artifact/config/fixture provenance and benchmark raw results/summary. Ten new fixture users/profiles are owned by this phase; previous profiles are untouched. Dedicated `users-lists-list-runtime` and Redis DB10/reactions DB15 are reused serially without resets; backend E2E uses separate `test-misskey`. No shared or production service is contacted.
