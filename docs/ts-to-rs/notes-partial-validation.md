# Native partial-note polling validation

`notes/show-partial-bulk` now has a complete native implementation sharing the existing note reaction and emoji packers. It remains outside `ACTIVE_READ_ENDPOINTS` pending its paired measurement and activation checks. The public listener and API contract stay unchanged; no TS callback is used by the candidate native handler.

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
| Native partial-bulk activation E2E | PENDING | Must be rerun with the new handler selected. |
| Paired measurement / final CI | PENDING | No activation or CI acceptance claimed yet. |
