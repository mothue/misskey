# Native users/relation candidate — held

The complete native operation is implemented as an internal module, with a typed twelve-field Following projection and request-local relationship sets. It adds no crate, schema, cache, RPC helper, TS business callback or connection route. Shared authentication, suspension/scope checks and validation run before the business body.

**FAIL — measured activation gate; Held (TS).** Five of six scenarios fail the throughput/p95 gate. Only the account-token scalar request passes both metrics. The scoped-token 20-ID request is 30.46% slower in median throughput with p95 29.12% higher. No dependency exception is taken. The activation list and TS callback guard exclude this operation; the ordinary TS business body remains selected. The generated registry records the final selection.

## Preserved contract

Both scalar and array input return arrays. Empty arrays still initiate the eight independent read-lane queries. Duplicate IDs collapse in first-appearance order. IDs remain case-sensitive opaque strings, without a user lookup, date parsing or length cap beyond the existing input validator. Missing/self/remote/suspended target IDs do not introduce a new error.

Following includes all twelve persisted scalar fields, with explicit nulls; it never adds joined users. Nullable strings preserve null, empty and nonempty values. The SQL varchar `notify` is retained even when outside its narrower TS annotation. The other eight flags are booleans; an absent following is explicit null. A still-present expired mute row remains true, matching TS. There are no counts, bigint or timestamp conversions in this response. UTF-16 values are retained at the value boundary.

Scalar reads use a full outgoing Following query plus seven EXISTS queries. Array reads use the original viewer-wide Following projection plus seven ID reads. All eight statements run concurrently on the original read lane. Their independent snapshots are retained; no combined statement or hidden cache is introduced.

## Fresh cloud evidence

| Check | Result | Evidence / limit |
| --- | --- | --- |
| TS-first public HTTP | PASS, 60 cases, exit 0 | Before integration at `b06da9fb69a63be3e575b7ca4025dd4e44fb8292`: 43 success, two 401, three 403 and twelve 400 responses. Full following fields, null/empty/unknown notify, directions, duplicate order, unknown/long/case IDs, expired mute, scope, moved/suspended callers and malformed input. |
| Native public HTTP differential | PASS, 60 cases, exit 0 | Same status, body and stable headers; only per-request diagnostic UUIDs normalized. Scope denial precedes malformed-input validation as in TS. |
| SDK validation / parity | PASS coverage/parity; SDK BASELINE | Candidate capture: 78 paired cases cover all 32 provisionally selected native operations; 71 SDK PASS, seven inherited identical violations from issue #55, zero new failures. Final held inventory has one fewer native selection. Raw following extras are also compared semantically, not trusted through the SDK type. |
| Related backend E2E | PASS, exit 0 | `users.ts` plus `api-visibility.ts`: 264 passed, one skipped and ten TODO. Three new tests use isolated users and finally-cleaned typed relation repositories, including all flags and expired mute. |
| Rust workspace | PASS, exit 0 | 272 passed, 11 ignored. Four new inline tests cover complete/null/missing/invalid fields, open notify strings, flags, ordering, duplicates and scalar-array behavior. |
| fmt / clippy / backend types | PASS, exits 0 | Workspace fmt, all-targets clippy with `-D warnings`, source/E2E/federation typechecks. |
| Extra manual test-file lint | BASELINE, exits 1 | Existing `response.avatarUrl && assert.match(...)` expression is rejected on both HEAD and the changed test file. Evidence retained separately; configured shipping checks pass. |
| API type generation / build | PASS, exit 0 | Explicit dedicated configuration; no generated SDK source differences. |
| Independent code review | PASS | Fresh-context manual review found no defects; final held selection, inventory and evidence verified. All 42 CSV rows independently matched raw phase results. |
| Native child lifecycle | PASS, exit 0 | Valid relation and core requests return 503 after child termination; parent survives, no restart, owned processes collected. |
| Paired measurement | PASS execution, FAIL activation gate | All 36,000 measured requests and 3,600 warm-ups matched their exact TS responses; zero HTTP, timeout, transport or semantic errors. |
| Product browser | PASS TS baseline / final held selection; native UI SKIPPED | Four actual API-console scenarios (scalar, duplicate, empty and invalid input), matching displayed responses, eight screenshots, zero page errors. Both business paths are TS. Three earlier failed attempts used an incorrect Send-button selector; all logs retained. Native UI was not run after the failed activation gate. |
| Final held-selection build / HTTP | PASS, exits 0 | Rebuilt native and TS artifacts with relation excluded from both activation and callback guard; all 60 final public HTTP cases match the TS baseline. |
| Local federation | SKIPPED | Candidate is held; no business handler is activated. Related backend E2E was run against the native candidate. |
| Final pushed-commit CI | PASS | `b0bcddc669c743a344e6119a872949bbe6981adf`: 18 successful workflows and one Storybook skip. |

The unchanged common benchmark runner used three AB/BA/AB pairs, concurrency 8, 100 warm-ups and 1,000 measured requests per scenario per side. Six cases use the same owner graph and identical scalar, 20-ID and empty-array inputs for account and read:account app tokens. Fixtures include 45 targets, 39 following rows and independently directed request/block/mute rows. Source/artifacts/config/fixtures stayed fixed during the 80.65-second run `d973a8e0-2cef-4f93-b95a-af239c8e696c`. This is a bounded dedicated workload, not production traffic. [Full metrics and variation](users-relation-benchmark.csv).

The original private runner's human-readable scope label mistakenly retained list-detail wording. Its actual scenarios, requests, checks and numeric results are relation-specific. Raw results and the original harness remain unchanged; a separate metadata-correction record and corrected summary describe the six real cases. All 42 numeric CSV rows remain identical.

Evidence is retained in `/workspace/native-queue/users-relation/`: command/cwd/exit-code ledger `results.jsonl`, sanitized `logs/`, captures, reviews, benchmark provenance/raw results/summary/correction and final CI record. The dedicated `users-lists-list-runtime` DB and Redis DB10/reactions DB15 are reused serially without resetting previous fixtures. Backend E2E uses the separate `test-misskey` DB. No shared or production service is contacted.
