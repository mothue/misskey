# Native partial-note polling validation

`notes/show-partial-bulk` now has a complete native implementation sharing the existing note reaction and emoji packers. It is selected by `ACTIVE_READ_ENDPOINTS` after passing its paired measurement; the TS callback guard also rejects this endpoint. The public listener and API contract stay unchanged; no TS callback is used by the candidate native handler.

The implementation preserves database-returned order and duplicate elimination, the operation-specific visibility rules, direct following/viewer reads, and the existing reaction-buffer lookup using the original requested IDs. Recipient and mention arrays explicitly represent PostgreSQL NULL members as nonmatching IDs. No schema, frontend, slacc, or queue changes are included.

The existing note reader now applies its ordered endpoint access checks before decoding unrelated payload. Shared legacy reaction interpretation preserves the actual TypeScript/Bun JSON operations, including UTF-16 keys, property order, numeric versus string addition, plain-object property behavior, and runtime errors. Permission inputs remain strictly decoded.

## Current evidence

Fresh cloud evidence is stored privately under `/workspace/native-queue/partial-bulk/`; commands, exit codes and log paths are in `results.jsonl`. Private reproduction details are excluded from Git.

| Check | Result | Scope |
| --- | --- | --- |
| Test before correction | PASS (expected failures observed) | Paired public HTTP comparison failed before the corresponding corrections; these red runs are retained, not reported as successful product runs. |
| Final payload/access differential | PASS, exit 0 | 47 actual HTTP cases against TS note bodies at `57fdf2198bb66b1fe761ef23fa983e3d9d1e6865`; only per-request diagnostic correlation UUIDs normalized. |
| Nullable membership differential | PASS, exit 0 | Two cases previously failed before correction. |
| Independent source audit | PASS, static inspection | All 28 previously activated handlers inspected; the note-specific correction is included in this batch. This is not 28-endpoint HTTP coverage. |
| Independent batch review | PASS | Both review findings resolved and reviewed again. |
| Related backend E2E | PASS, 140 tests, exit 0 | Existing activated note reader and TS partial-bulk path while activation remains held. |
| Backend type checks | PASS, exit 0 | Source, backend tests and federation tests. Initial concurrent generated-dependency check failed; serialized retry passed. |
| API type generation | PASS, exit 0 | Explicit dedicated configuration; no SDK source diff. Initial empty local YAML attempt failed and is retained. |
| Native partial-bulk activation E2E | PASS, 140 tests, exit 0 | Rerun with the new handler selected and the TS callback guard enabled. |
| Native partial-bulk differential | PASS, 88 cases, exit 0 | Buffering on/off; 1/30/100 IDs, order, duplicates, missing IDs, validation and viewer variants. No body normalization. |
| Rust child lifecycle | PASS, exit 0 | Startup, shutdown and 503 for six native endpoints after child termination; host remains alive without fallback/restart. |
| Rust fmt / clippy / workspace tests | PASS | Clippy `-D warnings`; 246 tests passed, 8 ignored. Ignored tests are not successes. |
| Local federation E2E | PASS, exit 0 | Fresh container-native build, 9 files / 92 passed / 13 skipped; owned services and volumes removed. |
| Paired measurement | PASS, exit 0 | 36,000 measured + 3,600 warm-up requests; all six throughput/p95 gates passed. |
| Activation commit CI | PASS | `f597c92ea699109f82ffc9b9cbc116cc0ad35536`: 18 successful workflows; Storybook explicitly skipped. Backend and federation workflows succeeded. |

## Paired cloud measurement

The existing `phase`/`summary` runner is unchanged. Three pairs use AB/BA/AB order, concurrency 8, 100 warm-up and 1,000 measured requests per scenario per side. The baseline is the TS business body at `6125c29f1cab138665bae946b321dfdef5855794`, with the existing hybrid common runtime. Each response is compared with its captured TS status, body and stable headers. The fixture and product source/artifacts are frozen during the run. This is a small dedicated workload, not production traffic.

For 1, 30 and 100 notes, anonymous and authenticated, median throughput improved 46.7–85.9% and median p95 fell 31.8–42.0%. All scenarios meet throughput ≥ baseline and p95 ≤ baseline. Raw variation and RSS samples are in [the generated measurement CSV](notes-partial-benchmark.csv); RSS is sampled, not peak or unique physical memory.
