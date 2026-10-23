# Ordered endpoint catalog port — cloud validation

This unit extends `c33eae98b9243a922ab13ae641e48a864d486a94` with the `/api/endpoints` callback. The user approved implementation, independent review, commit/push and maintenance of develop-targeted [PR #48](https://github.com/mothue/misskey/pull/48). PR adoption still requires a human merge; no merge or history rewrite is authorized.

## Contract and scope

The existing `RustBackendService.start()` bootstrap supplies the real endpoint registry to the immutable core `Catalog`. `Catalog::names()` returns its UTF-16 names in order; the bridge adds request `{v:1,op:"endpoints"}` and response `{v:1,op:"endpoints",endpointsUtf16:number[][]}`. TS validates the version, operation, both array levels and every integer code unit in `0..65535` from `unknown`, then reconstructs `string[]`. Empty catalogs/names, duplicates, NUL, supplementary characters and lone surrogates are preserved. No sorting, filtering, deduplication or fixed registry size is introduced.

Only callback/DI and the internal operation change. Public `meta`, `paramDef`, response schema, auth, permissions, AJV, defaults, Fastify and HTTP response handling remain TS. No DB/Redis path, dependency, frontend product code, slacc source or database schema changes. The existing bounded transport, timeouts, no-retry/no-restart/no-fallback policy and fixed unavailable error apply. The approved failure behavior changes `endpoints` from 200 to 503 after Rust exits; the original three selected APIs continue to return 503.

The public result contains only strings: no new public numeric/bigint/date conversion is added. Missing/null/wrongly typed internal list fields are invalid rather than an empty catalog; valid empty arrays remain valid. Existing JSON echo/default/null behavior and singular `endpoint` null-to-204 behavior are untouched.

## Cloud execution and results

Executed on Debian 13.6 Linux x86_64, Bun 1.4.4, Rust/Cargo 1.99.0. These are new executions for this unit, not the earlier macOS validation or the earlier read-only cloud investigation. Logs and machine-readable results live at `/workspace/endpoints-port/`; Rust implementer logs are at `/workspace/endpoints-port-rust/logs/`. `results.jsonl` records cwd, argv, exit code, elapsed seconds and log path. Artifact disks/caches are not assumed to persist across environment recreation.

| Status | Check | Command / evidence | Exit |
| --- | --- | --- | --- |
| PASS | Rust fmt | `cargo fmt --all -- --check`; `endpoints-port-rust/logs/fmt.log` | 0 |
| PASS | Rust Clippy | `cargo clippy --locked --workspace --all-targets -- -D warnings`; `clippy.log` | 0 |
| PASS | Rust unit/process | `cargo test --locked --workspace`; 5 core + 9 bridge/process tests; `test.log` | 0 |
| PASS | Native release + production/E2E bundles | `bun run --filter backend-rust build:native`, backend `build` / `build:e2e`; `logs/build-rust.log`, `backend-build.log`, `e2e-build.log` | each 0 |
| PASS | Backend/test/federation TS types | `bun run --filter backend typecheck`; `logs/typecheck.log` | 0 |
| PASS | Real-child TS transport/service | `rust-backend.test.ts`, 19 tests; `agent-ts/endpoints-ts-full-tests.log` | 0 |
| PASS | API type generation | `bun run generate:api-types`; `logs/api-types-retry.log`; no generated `misskey-js` or generator API JSON diff | 0 |
| PASS | Public HTTP differential | `http-check.py baseline` before rebuilding, then `candidate`; `baseline-http.json`, `candidate-http.json`, `logs/baseline-http.log`, `candidate-http.log` | each 0 |
| PASS | Expanded HTTP fixtures | Existing capture harness, 482 cases including all 438 singular descriptions and the new list fixtures; `logs/expanded-http-retry.log`, `candidate-expanded-http.json` | 0 |
| PASS | Related backend E2E | `vitest run --config vitest.config.e2e.ts test/e2e/api.ts test/e2e/endpoints.ts test/e2e/rust-endpoints.ts`; 3 files / 115 tests; `logs/related-e2e-retry.log` | 0 |
| PASS | Federation E2E | Compose tester runs the unmodified `bun run --filter backend test:fed`; 9 files, 92 passed / 13 skipped; `logs/federation-tests-retry.log` | 0 |
| PASS | Scoped lint / SPDX / locale safety | `node scripts/checks/shipping.mjs --base c33eae98b9243a922ab13ae641e48a864d486a94`; `logs/shipping.log`; added E2E lint is recorded separately | 0 |
| PASS | Independent code/API review | Separate agent without implementation conversation; `.claude/agents/misskey-api-reviewer.md` checks included; `/workspace/endpoints-port/independent-review.md`; no actionable findings | — |
| SKIPPED | CodeRabbit review | CLI absent; installation attempted and `cli.coderabbit.ai/install.sh` returned HTTP 403; `logs/coderabbit-install.log`. No CodeRabbit success is claimed | curl 22 |
| SKIPPED | Product UI / full root build / benchmarks | No frontend product change; full frontend dependencies remain unavailable in this cloud due to GitHub API egress restrictions. Performance work is explicitly deferred | — |

The public differential uses the actual pre-port production bundle and Rust binary before either was rebuilt. It compares status, exact JSON body (therefore name order), Content-Type and Cache-Control for 11 conditions: normal object, extra properties, empty-name extra input, array, JSON null/number, missing body, malformed JSON, invalid auth, GET and a body over the public limit. Measured catalog size was 438; implementations and tests derive expectations from the registry rather than fixing this number. Healthy public responses match exactly. After killing the verified child, candidate `endpoints` and the first three APIs return fixed `RUST_BACKEND_UNAVAILABLE` 503; malformed JSON/schema errors still return 400, unknown API routes return 404, no replacement child appears, and normal parent shutdown exits 0.

Versioned fixtures in `fixtures/ports/http.json` now include the new operation's successful, extra-input, invalid-input, method and body-limit cases. The existing HTTP capture harness additionally exercises the first three APIs and every singular endpoint description. This is compatibility capture, not benchmark measurement.

The E2E server is a Rolldown bundle, while tests import source through Vite. Their namespace enumeration representations differ. Membership is checked against the complete source registry in the E2E test; additional-input responses are compared without sorting to the normal response. Exact original order is independently checked by the production baseline differential and synthetic ordered/duplicate UTF-16 core/bridge/service tests. No product sorting was added to accommodate the test runner.

## Isolation, failed attempts and limits

Host HTTP uses dedicated `endpoints-runtime` PostgreSQL and Redis DB 13. Backend E2E uses only `test-misskey` and its dedicated Redis in Compose project `cloud-endpoints`; DB-resetting suites are not run concurrently. The pre-port runtime migrations use explicit temporary configuration, including the fixed compiled-config path needed by an existing migration. No schema source was edited.

Federation uses project `cloud-endpoints-fed`, new DB/Redis named volumes, two apps and Nginx frontends, plus the existing cloud-built tester image. Only native build caches from the previous stopped test project are reused: the one-shot builder rebuilds the current slacc/backend-rust artifacts in container-only volumes before app readiness. Apps mount those outputs read-only; host build outputs are covered by these mounts. Thus old cache existence is an optimization, not evidence of the current source passing. Test config paths are explicit. Internal test hosts use NO_PROXY; outbound access retains the managed proxy and CA verification. Nginx reads container-local test certificate copies with its own uid and mode 0640. Fixture IP rate limiting is disabled, without a key-purging daemon or in-run DB/Redis mutation. This gives no rate-limit coverage. The 13 federation skips remain unverified.

Failed attempts are retained rather than rewritten as PASS:

- Initial API generation exited 127 because the npm-distributed Bun binary lacked a `bunx` executable link. Adding that tool link outside the repository allowed the unchanged generation command to pass; generated public types did not change.
- Initial added E2E assertions compared Vite namespace order directly with Rolldown namespace order and failed two new assertions. The runner-aware test described above corrected that test error; all 115 related cases then passed. Existing production HTTP order was already identical before/after the port.
- Initial temporary federation config accidentally renamed the cached image while changing the project name, causing a pull failure before any test. Correcting the image reference allowed startup and a fresh complete run.
- An optional repeated Python HTTP probe hit BrokenPipe while sending an oversized body to a server that rejects it early. The prior 11-case differential had already passed. Running the expanded fixtures through the existing Bun fetch harness completed all 482 expected status checks; the failed optional attempt remains recorded.
- An added-test lint invocation from repo root could not locate the package ESLint config; rerunning from backend cwd is the applicable check.

Historical cloud findings are not current-cycle successes or product-regression claims: the earlier complete backend E2E had five SystemWebhook timeouts reproduced on the exact pre-migration baseline with separate fresh DB/Redis, with localhost DNS deadline errors in stored queue failure reasons. A prior transient following-count mismatch was not conclusively classified. Neither problem was fixed in this unit. The complete backend suite is not claimed to have been rerun/passed locally here; current related E2E and federation results are the rows above. The earlier macOS/canary 404 disposal crash did not reproduce in the prior stable Linux investigation; no fix is bundled here.

## PR, CI and next boundary

The existing draft PR #48 is reused. Its description and final-head CI results are updated after publication; CI success must be checked against that exact pushed commit rather than inferred from the previous green head. No merge or close is performed. No CHANGELOG edit is included; candidate: “Backend: run the endpoint catalog listing in the required Rust subprocess while preserving its healthy public response.”

The recommended next approval unit is the read-only `get-online-users-count` handler plus the minimal Rust PostgreSQL adapter it needs. It should own the complete count operation rather than RPC individual helper functions, retain TS ingress/cache policy, preserve the strict time threshold and JS-safe numeric meaning, and add no schema changes or Redis acquisition. The pool/driver, secret transport, deadline/shutdown ownership and baseline-fixture plan should be specified before starting. That unit is not authorized by completing this one.
