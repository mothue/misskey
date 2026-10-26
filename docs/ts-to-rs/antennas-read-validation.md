# Complete native antenna reads

`antennas/list` and `antennas/show` use complete native business handlers after their ten-scenario paired measurement passed. Shared Rust admission owns authentication, `read:account`, suspension checks and validation. Neither handler calls a TS business callback. The [generated registry](native-status.md) records selection; this document does not replace final CI acceptance.

List preserves the original owner-filtered full entity projection and database-returned order, without sorting, pagination, caps, caching or unread queries. Show filters both ID and owner in parameterized SQL before decoding the payload. Missing and foreign-owned antennas retain the same `NO_SUCH_ANTENNA` status/message/code/id. Bootstrap supplies the actual entity metadata; query construction checks every selected property and hydration kind and reuses existing identifier/literal quoting.

The formatter retains explicit nulls, opaque keywords/excludeKeywords JSON and nullable array members exactly as the TS packer does. It adds no read-time normalization. Existing ID parsing supplies `createdAt`, including irregular stored IDs and the observed `RangeError: Invalid Date`. Permission inputs remain strictly typed. There are no schema, frontend, slacc, queue or notification changes.

## Fresh cloud evidence

Private evidence: `/workspace/native-queue/antennas-read`, including `results.jsonl`, sanitized logs, captures, source/build hashes, reviews and raw measurements. The TS business baseline is isolated hybrid commit `8d907ab08a6ee02af0bcfa9a672771379ddf4859`, not pure-TS develop. Dedicated runtime DB `users-lists-list-runtime` and Redis DBs 10/15 were used serially; DB-reset E2E uses separate `test-misskey`/Redis14 settings.

| Check | Result | Scope / limit |
| --- | --- | --- |
| Actual TS packing oracle | PASS | 31 source-executed cases: 19 successes and 12 expected RangeErrors, all six ID methods; zero repository/network access. |
| Public HTTP differential | PASS, exit 0 | 46 same-input cases, including account/app scopes, anonymous/suspended/moved callers, owner/foreign/missing IDs, input boundaries, order and malformed stored JSON. Only diagnostic correlation UUIDs normalized. |
| SDK validation and parity | Coverage/parity PASS; SDK BASELINE | All 34 selected native endpoints covered; 79 conforming samples, 22 identical TS/native declaration discrepancies: seven inherited issue #55 samples, twelve deliberate achievement-storage probes and three deliberate malformed antenna-storage probes. No native-only mismatch. Global samples use pure-TS d875b7e; local samples use hybrid 8d907ab. |
| Rust fmt / clippy / workspace tests | PASS | Locked workspace/all-target clippy with `-D warnings`; 292 tests passed, 11 ignored. Extra style lints add no warnings in antenna logic; existing warnings remain. |
| Backend types / API generation / production and E2E builds | PASS, exit 0 | Explicit dedicated configuration; generated SDK source unchanged. |
| Related backend E2E | PASS, exit 0 | `antennas.ts` and `api-visibility.ts`: 167 passed, one skipped, three TODO. Added foreign-owner rejection and account/scoped-token equivalence/denial tests. |
| Product UI | PASS | Five actual TS/native page pairs: empty/one/twelve-row lists and two edit pages; no page errors. No save/delete actions or mocked API responses. |
| Child lifecycle | PASS, exit 0 | Exactly one Rust child; after termination both antenna APIs and four core APIs return 503, host survives without restart; owned processes collected. |
| Paired measurement / activation gate | PASS, exit 0 | All ten throughput/p95 conditions pass; all measured responses match captured TS responses. |
| Independent source and measurement review | PASS | Fresh-context source review found no actionable issue; all 70 CSV metric rows and ten activation gates independently recomputed from saved run summaries, with zero discrepancies. Request-level latency samples are not retained. |
| Fresh local federation | PASS, exit 0 | Native container builds; nine files, 92 passed, 13 skipped. Product/test source (1,160 files) and e44fdafc69 HEAD remained unchanged. Owned containers and volumes removed. |
| Final pushed-SHA CI | PENDING at this evidence commit | Check the PR checks for the final pushed SHA; local verification is not a CI success claim. |

The related readiness change initially left two neighboring unit-test mocks stale at `8d907ab`; this was an introduced test-fixture omission, not an established product or baseline failure. Commit `4c7bfa0251` updates only those mocks. Fresh closest tests pass 34; full backend unit tests pass 951 with two skipped across 80 files. The first full local run failed because Meilisearch was absent; after provisioning the CI-pinned Meilisearch v1.49.0, the full rerun passed. Logs remain under `/workspace/native-queue/user-birthdays-resume/logs/ci-*`; the owned Meilisearch container was removed afterward.

Federation evidence is in the phase's `federation/` directory. The first container build failed before compilation because the new dedicated bind-volume directories did not exist. After preparing those directories, the Linux-native build passed in 194.398 seconds and the test command passed in 85.722 seconds. Both attempts are retained separately. This was an environment preparation failure, not a product regression. Teardown reclaimed approximately 3.4 GiB of overlay storage; no shared runtime database was reset.

## Paired measurement

Run `6a108dec-8270-48fe-b57d-3f1973a39289` uses the unchanged common runner, three AB/BA/AB pairs, concurrency 8, 100 warm-up and 1,000 measured requests per case/side: 60,000 measured plus 6,000 warm-up. List covers zero/one/twelve owned rows under account and scoped app tokens; show covers minimal/populated payloads under both token types. Source, builds, configuration, expected responses, owned antenna rows and relevant role/meta state remain frozen throughout. This bounded dedicated workload is not production traffic.

Median throughput improves 11.82–74.51%, and median p95 decreases 14.32–47.49%, across all ten scenarios. [The generated 70-row CSV](antennas-read-benchmark.csv) includes variation and sampled Linux VmRSS; RSS is neither peak memory nor unique physical memory. No performance exception or favorable-run retry was taken.
