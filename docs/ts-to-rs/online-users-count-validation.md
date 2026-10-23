# Online-user count: cloud validation

Date: 2026-10-23. Starting branch `feat/ts-to-rs`, clean HEAD `2818f4614e8338f3d9959cf5af8f644f86fc727d`, also the remote/PR #48 head. No later work was reset. This unit ports `get-online-users-count` and a minimal read-only PostgreSQL adapter. Earlier local/cloud records remain historical.

## Behavior and configuration

The TS endpoint keeps its metadata, AJV, GET/POST behavior and cache headers. Rust owns the operation from one clock sample through SQL and result construction. The nullable `timestamptz` condition is strictly `lastActiveDate > now - 600000ms`, with no user visibility/suspension filters. COUNT follows existing pg int8-to-Number and TypeORM parseInt behavior, including rounding above 2^53. Core still uses only the standard library.

The adapter is lazy and shares one connection per configured read target. Replicated reads select a slave for each operation; no retry or failed-slave master fallback. Total deadline is four seconds, including slot acquisition/connect/query/cancellation waits. Read-only mode is set before the query. Shorter configured client deadlines and existing server timeouts remain effective. Query failure/cancellation drops the connection; a later request can establish a new one. Unfinished SQL also sends a bounded 500ms CancelRequest using the same TLS policy, because TCP close alone does not interrupt a PostgreSQL lock wait. Shutdown cancels connection establishment and queries and bounds adapter disposal to one second. A network failure can prevent delivery of cancellation; no implementation can promise server-side cancellation through a broken network.

The TS normalizer constructs the installed `pg.Client` without connecting and uses its resolved connection/startup/password settings. It preserves extra/URL/PG environment precedence, pgpass matching/permissions, read targets, options, application name, timeouts, keepalive and channel binding. TLS keeps resolved roots/explicit CA replacement, hostname/SNI, certificate/key/passphrase, verification, protocol/cipher settings and direct negotiation with PostgreSQL ALPN. Enabled TLS never falls back to plaintext. Credentials/PEM/bootstrap are private stdin data, not argv, stdout, diagnostics or public errors.

Supported extra keys are explicit in `postgres-config.ts`: base connection fields/connectionString, ssl/sslnegotiation, options, application/fallback name, statement/lock/idle-in-transaction/query/connect timeouts, UTF-8 client encoding, keepalive and channel binding. Pool max/min/idleTimeoutMillis/allowExitOnIdle/maxUses/maxLifetimeSeconds continue to govern the TS pool; Rust owns its independently bounded shared connection. TLS supports rejectUnauthorized, ca, cert, key, passphrase, servername, minVersion, maxVersion and ciphers. Arbitrary JS callbacks, custom streams/clients/type parsers/TLS contexts, non-UTF-8 encoding and unsupported effective startup/TLS options are unsupported: the new operation returns the fixed 503, while the original four remain available. This is a configuration limitation, not proof that every possible pg extension has been ported.

## Executed results

All commands below ran in this cloud on this unit. Paths are relative to the repository unless stated. Full argv/cwd/exit code/time/log records, including failed attempts, are in `/workspace/online-users-port/results.jsonl`; human-readable command index and an evidence bundle accompany the final report. Dedicated PostgreSQL 18 / Redis 7 fixtures were used. DB-reset suites never ran concurrently against the same DB.

| Status | Verification | Command / evidence | Exit |
| --- | --- | --- | --- |
| PASS | Existing TS public baseline | `python3 /workspace/online-users-port/http-check.py baseline`; `baseline-http-final.log`, `baseline-http.json` | 0 |
| PASS | pg/TypeORM Number boundary baseline | Real `SELECT ...::bigint`, pg OID20 Number then parseInt: 0, 1, 2^53-1, 2^53, 2^53+1, i64::MAX; `baseline-number.log` | 0 |
| PASS | Rust fmt | `cd packages/backend-rust && cargo fmt --all -- --check`; `rust-fmt-final.log` | 0 |
| PASS | Rust clippy | `cargo clippy --locked --workspace --all-targets -- -D warnings`; `rust-clippy-final.log` | 0 |
| PASS | Rust workspace tests | `cargo test --locked --workspace`; 7 core + 10 child process + 6 adapter tests; dedicated DB test explicitly ignored in this command; `rust-test-final.log` | 0 |
| PASS | Dedicated real PostgreSQL test | `MISSKEY_POSTGRES_TEST_CONFIG=/workspace/online-users-port/adapter-config.json cargo test --locked -p misskey-backend-postgres -- --ignored`; `postgres-lifecycle-shutdown-retry.log` | 0 |
| PASS | Main/test/federation TS types | `cd packages/backend && bun run typecheck`; `ts-typecheck-final.log` | 0 |
| PASS | Transport, server shutdown, protocol and config units | Four selected files: `transport-unit.log`; final Rust transport/config files: 34 tests, `ts-rust-full-unit.log` | 0 |
| PASS | Backend/native build, API generation and unchanged public generated types | `NODE_ENV=test MISSKEY_CONFIG_JSON=... bun run generate:api-types`; `api-types-serial.log`; `git diff --exit-code -- packages/misskey-js/src/autogen packages/misskey-js/generator/api.json`, `generated-api-diff-serial.log` | 0 |
| PASS | E2E server build | `bun run --filter backend build:e2e`; `e2e-build-serial.log` | 0 |
| PASS | Production-bundle public HTTP differential and child failure | `python3 /workspace/online-users-port/http-check.py candidate`; `candidate-http-final.log`, `candidate-http.json` | 0 |
| PASS | Real TLS differential against existing pg | `bun --tsconfig-override packages/backend/tsconfig.json /workspace/online-users-port/tls-check.ts`; 8 cases, `tls-compatibility-ready.log` | 0 |
| PASS | Related existing backend E2E | api.ts, endpoints.ts, rust-endpoints.ts and move.ts: 177 passed in `related-e2e.log`; same run's new fixture failure is recorded below | 1 (combined run) |
| PASS | New online-count E2E after fixture correction | `NODE_ENV=test MISSKEY_CONFIG_JSON=... bun run --bun vitest run --config vitest.config.e2e.ts test/e2e/rust-online-users-count.ts`; 2 passed, `count-e2e-fixed.log` | 0 |
| PASS | Federation image / container native linkage | Updated federation Dockerfile includes pkg-config/libssl-dev; `federation-image.log`, `container-native-links.log` | 0 |
| PASS | Federation E2E | Container-built Rust/slacc output volumes, two dedicated PostgreSQL/Redis app environments; 92 passed, 13 intentionally skipped; `federation-tests-serial.log` | 0 |
| PASS | Final changed-file lint, SPDX and locale safety | `node scripts/checks/shipping.mjs --base 2818f4614e8338f3d9959cf5af8f644f86fc727d`; `shipping.log`; explicit new E2E lint and test typecheck also exit 0 | 0 |
| SKIPPED | CodeRabbit | CLI absent; installer host HTTP403, `coderabbit-availability.log`. Independent review performed separately. | 22 |

The HTTP matrix compared raw body, status, Content-Type and Cache-Control for POST/GET, extra/null properties, array/null/number body, empty body, malformed JSON and invalid auth. Normal responses are `{count:2}` for NULL/old/two future rows; GET retains `public, max-age=60`, POST retains private/no-cache behavior. After killing Rust, the new API and original four return the fixed `RUST_BACKEND_UNAVAILABLE` 503; no child restart. AJV errors still precede dispatch and unknown API still returns 404. Parent exits cleanly.

The real DB test covers empty table, NULL, exact cutoff, one millisecond before/after, one microsecond after, future values and non-UTC session zone; 16 concurrent requests share one connection; write attempts fail read-only; terminated connections are recreated only on a later operation. Locked queries time out, cancelled futures and shutdown leave no test SQL sessions. Unit tests also cover a stalled startup handshake, slot deadline and shutdown during connection establishment.

TLS cases compare the same configuration with existing pg and Rust: verified CA/client certificate, wrong hostname, untrusted CA, explicitly configured no-verify, required TLS against a plaintext server, legacy cipher selection with TLS1.3, encrypted client key/passphrase, and direct TLS. Failure cases return a generic 503, with ping still 200 and clean child exit.

## Independent review and failed attempts

A reviewer agent started without implementation conversation context and applied AGENTS/backend/API-reviewer guidance. It found and verified fixes for legacy-only cipher configuration dropping TLS1.3 defaults, direct TLS missing PostgreSQL ALPN, and unsupported effective replication startup options being ignored. It rechecked CancelRequest, shutdown-during-connect, and the corrected E2E fixture. No unresolved code findings. `/workspace/online-users-port/independent-review.md` records reviewed file hashes. Documentation is the leader's responsibility.

FAIL attempts are retained, not relabeled as successful runs:

- Initial DB migration targeted a not-yet-created dedicated database; explicitly creating it fixed setup.
- Initial HTTP fixture was inserted before NODE_ENV=test server schema initialization and got reset; seed after readiness. A production-mode attempt used cluster workers and the direct-child assertion was inapplicable. Final baseline and candidate use the same explicit test setup and production bundle.
- Initial PostgreSQL lifetime tests showed a real implementation issue: aborting TCP did not cancel server-side lock waits. Snapshot refresh alone did not fix it. Bounded CancelRequest fixed it; final tests passed. A further unit caught/covered shutdown during an unregistered startup connection.
- The combined backend E2E run exited 1 (178 passed, 1 failed): parallel initial fixture signups produced missing user IDs and TypeORM rejected empty update criteria. Fixture setup is now sequential with an ID assertion; both new tests passed on rerun. No product code change was required for this test issue.
- TLS fixture creation was once attempted before pg_isready; rerun after readiness passed.
- Federation's automatically allocated default network overlapped its explicit internal subnet; a dedicated temporary default subnet fixed that setup. Repeated build/federation attempts hit ENOSPC under Docker's vfs driver. Unused build caches and completed native debug artifacts were cleaned; fixtures were run in stages. API generation's interrupted file copy was restored from the clean starting HEAD, regenerated successfully, and final public-type diff was zero. These are environment/setup failures, not a backend regression.
- Early commands lacking the installed Bun directory in PATH exited 127 and were rerun with explicit PATH.

The earlier macOS outgoing-response disposal crash is not claimed reproduced here: move.ts passed 62 tests in this unit. Historical SystemWebhook/DNS failures are described in older cloud investigation evidence; this unit has not reclassified them as product defects. Final-head CI results are reported separately after push.

## Federation, cleanup and acceptance

Federation ran with fresh dedicated DB/Redis volumes and the repository's private-network topology. Temporary harness changes supplied proxy/CA trust, readable container certificate copies, explicit test configs, and a non-overlapping default subnet. The builder used the updated federation Dockerfile; app/tester runtimes used oven/bun 1.4.4 with separately container-built native output volumes (no host native artifacts). `ldd` in the app confirmed OpenSSL runtime linkage. The initial lightweight tester invocation tried to rebuild slacc without Cargo (FAIL/ENOENT). The accepted invocation ran compile-config and the exact federation Vitest configuration directly, after the one-shot native builder had already succeeded. Test skips were explicit existing skip cases; they are not passes. No benchmark ran.

Dedicated services were stopped after validation. Logs/review/baseline/candidate response records are under `/workspace/online-users-port/`; private test configs, keys, and local volumes are excluded from evidence bundles. Files are workspace artifacts, not proof of remote durability until downloaded or committed. Ignored builds, Docker containers, image caches and volumes are disposable and must not be assumed present in the next cloud environment. No shared/production service, schema, frontend product code or slacc source was changed.

CodeRabbit was SKIPPED because its installer was denied by the environment's network policy. Browser UI and a local complete frontend build were SKIPPED: this unit changes backend operation ownership and verifies the existing public HTTP contract; prior whole-workspace dependency acquisition was blocked on frontend GitHub source packages. Final-head hosted CI is checked separately, without manually launching external jobs.

The user's latest instruction prohibits editing the PR title/body. This agent does not edit either field; normal push advances the existing draft PR's code head. No merge/close is authorized.

The first pushed commit's Backend Rust CI failed only at the real-DB test fixture read (ENOENT): Cargo starts that test in its crate directory, while the workflow passed a workspace-relative path. The follow-up uses an absolute `github.workspace` fixture path and an explicit PostgreSQL health-check role/database. CI fmt, clippy and all ordinary Rust tests had already passed. See `ci-rust-fixture-failure.log`; verify the final pushed commit separately rather than treating the earlier failure as a pass.
