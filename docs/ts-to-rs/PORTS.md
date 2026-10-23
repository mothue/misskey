# TypeScript to Rust backend ports

## Authority and scope

The current approved unit extends the first port with `endpoints` (plural). The user authorizes implementation, validation, independent review, commits, normal pushes to `feat/ts-to-rs`, updates to its existing develop-targeted PR, and CI checks. Do not merge or close the PR, rewrite published history, or push directly to develop. `docs/superpowers` remains excluded from commits and must not be added to gitignore.

The final backend is Rust-only, but the public ingress moves last. Bun remains the build orchestrator. Port only the callback bodies of `ping`, `test`, `endpoint`, and now `endpoints`. Fastify/TypeScript retains public HTTP, authentication, permissions, rate limits, AJV/defaults, and response shaping. Do not change frontend product code, slacc, database schemas, or locale files. WebSocket, ActivityPub, uploads and all other handlers remain TypeScript. DB/Redis-dependent business ports come later. rocka and benchmark improvements/remeasurement are outside this unit.

`endpoints` reads the existing immutable Rust Catalog. Return every registry name in its original order without filtering, sorting, deduplication, or a fixed endpoint count. No new DB/Redis connection or data acquisition path is introduced. Rust failure now makes this fourth public API return the same fixed 503 error as the first three; no TS fallback, retry, or automatic child restart is allowed. See [the current unit's evidence](endpoints-validation.md); older evidence remains historical and is not a successful cloud rerun.

## Contract

Use an independent Cargo workspace at `packages/backend-rust`, with std-only `core` and `bridge` depending on Tokio 1, Axum 0.8, Serde 1, serde_json 1, thiserror 2, tracing 0.1 and tracing-subscriber 0.3. Pin Rust 1.99.0 and commit Cargo.lock when commits are eventually authorized. Business types/functions must not depend on the temporary RPC adapter.

The bridge listens on `127.0.0.1:0`. One HTTP POST `/rpc` performs one operation. All request and success response envelopes have `v: 1` and `op: 'ping' | 'test' | 'endpoint' | 'endpoints'`. Authenticate with `Authorization: Bearer <32-random-byte-token-as-hex>` received through stdin, never CLI arguments or logs. No retry, automatic restart, or TS fallback.

| Operation | Request fields | Success response fields |
| --- | --- | --- |
| ping | none | `pong: number` (safe signed integer Unix milliseconds, read in Rust) |
| test | `paramsJson: string` | `resultJson: string` |
| endpoint | `endpointUtf16: number[]` | `result: null | {params: {nameUtf16: number[], type: string}[]}` |
| endpoints | none | `endpointsUtf16: number[][]` |

TS validates every response from unknown, including version and operation. `test` serializes the complete AJV-validated params object with JSON.stringify. Rust treats the inner JSON text as opaque and echoes it. TS requires the returned text to equal the sent text and restores a validated object before returning it to the existing HTTP sender. Preserve additional properties, defaults/null distinctions, property order, number serialization and lone UTF-16 surrogates. Never expose the inner JSON as a public JSON string. Do not introduce unchecked generic result casts.

Bootstrap is one newline-terminated JSON frame `{v:1,token,endpoints}`. Endpoints are an ordered array of `{nameUtf16,properties:[{nameUtf16,type}]}`, where type is null for an absent type, otherwise the current schema's ASCII type. Extract it from the actual endpoint registry using Object.entries on top-level properties only. No allOf/anyOf expansion. Lookup is first exact UTF-16 match. Explicit types capitalize only their first ASCII character; absent type stays lowercase `string`. Unknown endpoint returns null and public HTTP remains 204 with no body. The list operation uses the same ordered Catalog and encodes each name as its exact UTF-16 code units, including empty names, duplicates, NUL, and lone surrogates. TS validates the version, operation, outer list, and every inner integer in 0..65535 before reconstructing strings; empty catalog returns an empty array. Definitions are immutable for one child lifetime. The complete bootstrap must be accepted before readiness.

Ready stdout frame: `{v:1,port}` followed by newline, maximum 4 KiB. Stdout is control-only; stderr is logs. Bootstrap and RPC request/response maximum: 16 MiB. Public 1 MiB body limit stays unchanged. Read sizes incrementally; do not allocate unbounded responses. All RPC/protocol/transport failures map to ApiError `RUST_BACKEND_UNAVAILABLE`, kind `server`, HTTP 503, with a fixed error ID and no internal details.

## Process and build lifecycle

One child per TS process that launches HTTP. RustBackendModule provides one shared RustBackendService; constructor/module initialization does not spawn. ServerService.launch awaits readiness before listening. Queue-only and metadata generation do not start Rust. Use operation-specific methods plus start/stop/isReady. Concurrent start/stop and double disposal are safe; a stopped instance does not restart.

Startup timeout 10 s; RPC timeout 5 s. Startup errors clean up and reach an explicit fatal nonzero boot path, including worker startup reporting. Runtime child exit makes selected APIs and healthz 503; unrelated TS APIs continue. A single call timeout fails that call, without declaring a live child dead. Recovery requires backend restart. Public HTTP and WebSocket shutdown is bounded to 5 s total, followed in finally by child stop; graceful child stop has 2 s, then kill and wait at most 1 s. Monitor stdin EOF so a killed parent cannot orphan the child. Ensure blocking stdin readers cannot prevent process exit. Connect normal signals, cluster workers, E2E reset and dev restarts.

Bun invokes Cargo `--locked --release`, target output `built/target`, distributed executable `built/bin/misskey-backend-rust` (platform suffix where applicable). Publish atomically on success; failed compilation must stop subsequent startup. Rust package script is build:native, not build, to avoid duplicate root workspace builds. Integrate backend build/unit/e2e/tsc/dev paths and direct benchmark builder. Watch Rust sources/manifests/lock, serialize build/restart and collect old processes. Find binaries relative to code root, not Config.rootDir/storage or cwd.

Docker builds for the target platform and copies the executable after runner source copy; ignore host Rust built artifacts. CI path filters cover Rust/integration files, with fmt/clippy/test on Rust changes. Include Cargo sources/locks and exclude compiled artifacts in source tarballs. Extend clean commands.

Federation: independent one-shot builder reusing the existing Rust-capable federation test Dockerfile; dedicated named volume over backend-rust/built, RW builder and RO apps. Both concrete app services depend on builder success. Do not add dependencies to the shared app template that setup extends. Preserve host artifacts. Tester itself does not need the new binary.

## Historical first-port plan

The tasks below describe the original three-handler implementation, not additional authorization for this unit. Their benchmark work is historical and is not rerun or improved in the current cycle. Current acceptance is recorded separately in [endpoints-validation.md](endpoints-validation.md).

### Task 1: Baseline, contracts and benchmark harness

Before editing production implementation, preserve baseline d875b7e3c737b7f9f75331fffb691f6a2b97f055 in an isolated copy. Prepare the same HTTP harness for both builds. Save baseline results. Behavior CSV columns: case ID, scope, source reference, TS type, runtime behavior, Rust contract, evidence type and status. Keep fixtures as JSON and runtime records machine-readable; never save credentials. Distinguish source/typecheck/baseline-runtime/differential evidence.

Cover existing input validation, defaults/null, extra properties, nesting, Unicode/lone surrogates, numeric boundaries, duplicate keys and near-limit bodies. Compare all registered endpoint descriptions, exact array order and type spelling, unknown/empty/case-mismatched names and 204. Include GET405, invalid auth/JSON and schema errors. Compare status, body and meaningful headers, explicitly normalize only documented volatile fields. Ping is clock-range checked plus fixed-clock unit tested.

Reuse the current benchmark framework and add these three APIs. Same environment/data/readiness; 100 warmup, 1000 measured requests, concurrency8, three baseline/candidate pairs with alternating order. Record p50/p95/p99, throughput, errors and TS/Rust memory. Build/startup duration separate. Run existing scenarios as infrastructure smoke. Performance improvement is not an acceptance gate; report overhead and spread honestly.

### Task 2: Rust core and bridge

Own Cargo workspace source, manifests/lock/toolchain and Rust tests only. Implement the exact contract and lifecycle above. Use test-first development; test fixed clocks, arbitrary opaque echo, ordered metadata and first-match lookup, malformed/unauthorized/oversize RPC, bootstrap failures, shutdown and stdin EOF. Coordinate build-script ownership with Task4.

### Task 3: TS integration and lifecycle

Own TS service/module/DTOs, the three endpoint callbacks, ServerService/HealthServerService, boot signal/startup integration, and associated TS tests. Preserve meta/paramDef/res. Reuse actual metadata rather than a hardcoded registry. Add bounded IO, response guards and all lifecycle/error cases. No retries/fallback. Source root lookup must survive production/test/benchmark bundle layouts and changed cwd.

### Task 4: Build, dev, Docker and CI

Own backend-rust package.json/build scripts, root workspace/lock, backend scripts/Rolldown, clean, Docker and federation compose/build wiring, CI and direct benchmark builder build hook. Avoid modifying benchmark scenario/harness owned by Task1. Test build tooling meaningfully. Retain slacc behavior.

### Task 5: Integration, independent review and acceptance

Run Cargo fmt/clippy/test, related TS checks and changed-file ESLint, repository unit tests, complete backend E2E and federation E2E. Full build, API generation (no unintended public type changes), tarball inspection, SPDX/shipping checks. Browser /api-console must generate argument fields and execute ping/test. Exercise real HTTP including errors and child termination; check process cleanup and no retries. Run paired benchmarks. Fresh-context agents review changes independently from implementers; additionally run local CodeRabbit if available. Fix findings and rerun affected checks. Record PASS/FAIL/BASELINE/SKIPPED, preserving existing failures with baseline evidence. Never label skipped verification as passing.

At most three worker agents plus leader in the current environment. Implementers do not commit, push or spawn reviewers; leader coordinates disjoint edits and independent review. Preserve review and measurement evidence because there are no commits this iteration.

## Implementation and evidence

The four selected handler bodies now run in the required Rust child. The TS service and Rust bridge implement the contract above, and normal/backend-test/development/Docker/federation/benchmark build paths include the native executable. Validation was performed on the implementation in `feat/ts-to-rs` before its commits.

Use [validation.md](validation.md) for actual PASS/FAIL/SKIPPED results and invalidated attempts. Detailed records are [behavior.csv](behavior.csv), [baseline evidence](baseline-evidence.md), [runtime fault and shutdown evidence](runtime-evidence.md), [federation evidence](federation-evidence.md), and [benchmark evidence](benchmark-evidence.md). A successful source review does not replace the runtime evidence or turn unfinished checks into passes.
