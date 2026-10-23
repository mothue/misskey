# Rust port runtime acceptance

Historical three-handler observations: `endpoints` was still TypeScript here. Its later Rust port intentionally changes the child-failure response from 200 to 503; see [endpoints-validation.md](endpoints-validation.md) for new cloud evidence.

Observed on 2026-10-23 using the actual production Bun bundle, actual Rust executable and dedicated benchmark PostgreSQL/Redis services. These observations are live HTTP/process evidence, separate from source review and controlled fault fixtures. No bridge token or user credential is saved in records.

## Actual HTTP and child termination

The existing server was verified as Bun parent PID321, Rust direct child PID330. It listened on localhost61912. Requests used public HTTP, with additional nested/lone-surrogate data in `test`.

| Request | Healthy | After verified Rust child SIGKILL |
| --- | --- | --- |
| POST /api/ping | 200 | 503 |
| POST /api/test with required=true and extra data | 200 | 503 |
| POST /api/endpoint for ping | 200 | 503 |
| GET /healthz/ | 200 | 503 |
| POST /api/endpoints (TypeScript handler) | 200 | 200 |
| POST /api/test without required field | 400 | 400 |

All three selected API failures returned `RUST_BACKEND_UNAVAILABLE`, kind `server`, fixed ID `91600ba8-92ad-48fa-9afd-d6c615f06946`, and no internal details. The health failure had no body. No replacement child appeared during the two-second observation interval. Successful TypeScript handling alongside unavailable selected APIs demonstrates no TS fallback for these callback bodies. Parent321 was then stopped normally and both original PIDs were gone.

Ignored machine-readable evidence: `built/ts-to-rs/runtime-http.json`. The probe also records meaningful Content-Type/Cache-Control response headers and exact response bodies.

## Actual parent shutdown and cluster cleanup

Each scenario started a fresh production bundle with no daemons, server-only mode and the same dedicated configuration; readiness was checked through real `/healthz/` before signaling. Process identities and direct/indirect parent relationships were captured before signaling. Child liveness excludes reaped/dead zombie states.

| Scenario | Verified processes | Result | Observed signal-to-cleanup time |
| --- | --- | --- | --- |
| Non-cluster SIGTERM | Bun2792 → Rust2794 | parent exit0; child gone | 40ms |
| Non-cluster parent SIGKILL | Bun2803 → Rust2804 | parent exit-9; child exited on stdin EOF | 23ms |
| Cluster master SIGTERM | master2811 → worker2812 → Rust2814 | master exit0; worker and Rust gone | 120ms |

These are observations from one run per scenario, not performance guarantees. Ignored records: `built/ts-to-rs/runtime-lifecycle.json`; per-scenario startup/process logs are `runtime-normal-sigterm.log`, `runtime-parent-sigkill.log`, and `runtime-cluster-sigterm.log` in the same directory. At completion, localhost61912 had no listener and the scenario processes were gone.

## Controlled fault coverage and limits

Associated focused tests use real controlled subprocesses for missing executable, silent-child10s startup timeout with child PID cleanup, malformed RPC JSON, streamed response exceeding16MiB, and request-count assertions proving no retry after failure. Existing tests cover readiness envelopes, UTF16, exact opaque JSON restoration, runtime child exit,5s call timeout preserving readiness, concurrent start/stop, terminal stop, Nest context disposal, and real Fastify/WebSocket bounded shutdown.

Missing-executable startup is exercised at the transport boundary rather than by removing the shared production binary. Actual missing-binary boot/cluster failure reporting was not probed in this run. Full HTTP differential, browser checks, suite/build/type generation and paired benchmarks are recorded by their owning acceptance tasks.

Final scoped verification: PASS17 tests across2 files, including exact16MiB acceptance and16MiB+1 rejection (`built/ts-to-rs/task3-final-fault-tests.log`); PASS changed-test ESLint (`task3-final-fault-eslint.log`). A final isolated startup timeout rerun additionally checks elapsed time remains within9.5–15s and the silent child's PID has exited.
