# Common API kernel: cloud validation

Date: 2026-10-23. Branch `feat/ts-to-rs`; starting HEAD `48783a1dedb2a97f8bb1323324c0b74e64cd8d0b`, initially clean and equal to draft PR [#48](https://github.com/mothue/misskey/pull/48). The PR title and body are user-owned and must remain unchanged.

## Implemented boundary

Every `/api` route enters Rust before body parsing, CORS, token authentication, authorization, Redis rate limiting, validation/default insertion and HTTP response formatting. Fastify retains the public socket, non-API routes and WebSockets. The observed registry contains five native business bodies and 433 TS business callbacks, plus seven special route patterns. Password/passkey/sign-up business logic remains explicitly delegated to TS. Counts describe this checkout; they are not dispatch constants. This phase does not claim that all business operations run exclusively in Rust.

- `api-validation`: AJV-compatible validation/default mutation/first error, query/multipart casts, tagged JS values and iterative JSON/value processing. Primitive uniqueItems uses typed hash keys, exact AJV reverse duplicate indices and prototype-key exceptions.
- `api-auth`: native/app tokens, selected PostgreSQL snapshots, role conditions/policies, matching caches/invalidation, lastUsedAt/IP writes. Full policy evaluation precedes use of its numeric rate-limit factor.
- `api-runtime`: ordered registry routing, request/body/multipart/tempfile handling, native dispatch, Redis limiting/pubsub and public errors/headers/status/body.
- `storage-postgres`: typed queries reuse TLS/cancellation/shutdown; read targets and primary writes remain separate. No schema change.
- TS: raw streaming transport and a private authenticated, one-use business executor. `Endpoint.invokeValidated` avoids duplicate public validation. Unported business Date/custom-toJSON serialization stays in TS; received frames are validated at runtime.

Early responses drain requests without destroying the public socket; disconnect immediately revokes the callback capability. There is no TS fallback, request replay, automatic child restart or new generic business timeout. Frontend product source, slacc source, schema/migrations, locales, rocka and benchmarks are unchanged.

## Approved public behavior corrections

The user approved these three changes with “続けて” on 2026-10-23; see [the completion plan](api-kernel-compatibility-plan.md).

1. Effective `i` that is neither string, null nor omitted finishes an empty400. The baseline set400 without sending and timed out. No callback runs. Bearer precedence, empty-string authentication failure and null/omission are preserved.
2. Anonymous endpoints with requiredRolePolicy evaluate anonymous base policies: true proceeds; false returns existing ROLE_PERMISSION_DENIED403. Only an actual authenticated root bypasses the check; absent user/root IDs never imply root. This deliberately replaces the baseline users/search TypeError500. Policy-dependent anonymous requests also fail closed while invalidation is unavailable.
3. Accepted but unevaluable stored policy values retain500 with a stable structured INTERNAL_ERROR. Baseline JS-dependent message-only500 is deliberately replaced. Valid arrays and iterable strings retain their behavior; malformed values do not authorize requests.

All other observed differences are corrected to the baseline, including MIME handling, early413 socket ownership, internal pubsub envelope filtering, unmatched-method parsing/header scope, whole-component query fallback, TRACE/DELETE/QUERY body rules and nonfinite role-factor handling. The broad common-layer migration remains experimental and requires a PR merge for adoption.

## Current verification phase

The table below records new executions after approval. Earlier runs are historical evidence only. Commands are recorded verbatim with cwd, exit code and log path in `/workspace/api-layer-port/results.jsonl`; logs are under `logs/`. Dedicated private test configurations are omitted from Git and the shareable bundle.

| Status | Executed check | Exit | Log |
| --- | --- | --- | --- |
| PASS | Production build, including native Rust/slacc and TS bundle | 0 | `logs/approved-coercion-final-build.log` |
| PASS | Backend E2E bundle build | 0 | `logs/approved-coercion-e2e-build.log` |
| PASS | Rust workspace fmt, clippy -D warnings, tests: 87 passed; four default-ignored service tests | 0 | `logs/approved-rust-final-coercion.log` |
| PASS | Explicit live Redis tests and PostgreSQL lifecycle/read-write/CancelRequest fixture | 0 | `logs/approved-live-adapters.log` |
| PASS | Actual AJV: 438 root schemas / 58,057 cases; zero mismatches | 0 | `logs/validation-unique-items-all-schemas-final.log` |
| PASS | Actual AJV: 2,212 subschemas / 175,789 cases; zero mismatches | 0 | `logs/validation-unique-items-nested-schemas-final.log` |
| PASS | Main, test and federation TS typecheck after generation | 0 | `logs/approved-types-after-generation.log` |
| PASS | Updated numeric-policy E2E test typecheck | 0 | `logs/approved-policy-types.log` |
| PASS | Ten related TS unit suites: 31 passed | 0 | `logs/approved-unit.log` |
| PASS | Full backend E2E: 34 files / 1,456 passed / two skipped / 20 TODO | 0 | `logs/approved-backend-e2e-full.log` |
| PASS | Full container federation E2E: nine files / 92 passed / 13 skipped | 0 | `logs/approved-federation-full-tests.log` |
| PASS | 475-case differential: 473 exact matches plus two named approved corrections | 0 | `logs/approved-http.log` |
| PASS | Thirty multipart/MIME baseline/candidate matches | 0 | `logs/approved-multipart.log` |
| PASS | 40 routing/bad-path plus 36 raw method-class baseline/candidate exact matches | 0 | `logs/approved-method-comparisons.log` |
| PASS | Five production numeric-factor pairs: Infinity and controls; exact baseline match | 0 | `logs/approved-ratefactor-pair-final.log` |
| PASS | Five numeric-ToPrimitive pairs: three approved stable500 error bodies; two400 controls exact | 0 | `logs/approved-coercion-http.log` |
| PASS | Real child fault: seven APIs and health503, no restart; normal shutdown and parent SIGKILL collect child | 0 | `logs/approved-lifecycle-fixed.log` |
| PASS | Chromium actual login/API-console ping and test200 plus six same-origin APIs | 0 | `logs/approved-browser.log` |
| PASS | API SDK generation with explicit private config; tracked autogen diff empty | 0 | `logs/approved-api-generation.log` |
| PASS | Changed-file lint, SPDX and locale safety | 0 | `logs/approved-shipping-final.log` |
| PASS | Fresh-context independent source/correction review: five P2 findings and test polling note resolved | source review | `approved-independent-review.md` |
| SKIPPED | CodeRabbit CLI/install: unavailable, installer denied HTTP403 | 127 / 22 | `logs/coderabbit-{version,install}.log` |

Full E2E JSON totals: {"numPassedTestSuites": 230, "numTotalTests": 1478, "numPassedTests": 1456, "numFailedTests": 0, "numPendingTests": 2, "numTodoTests": 20, "success": true}. Explicit skips and TODO are not passes.

The 475-case public HTTP comparison requires precisely two named approved differences: completed400 for wrong-token and users/search array input reaching validation400 instead of the old500. The other 473 must match status, body and meaningful headers. The third approved exception is covered by the dedicated policy tests. Unfinished baseline responses are BASELINE, never passing HTTP responses.

Additional response matrices use real original-production captures: 30 multipart/MIME cases, 40 unmatched-method/bad-path cases and 36 raw TRACE/DELETE/QUERY cases. Raw requests retain exact framing; fetch cannot represent TRACE or every absent Content-Length case. Query-reference tests use the installed fast-querystring implementation; primitive uniqueItems fixtures come from installed AJV.

## Reproduction and environment

Linux x86_64, Bun1.4.4, Rust1.99.0, PostgreSQL18, Redis7; Docker/Compose available. Run Cargo from `packages/backend-rust` so `.cargo/config.toml` selects `built/target`. Debug cache size can be reduced with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. No host-native artifacts are mounted into Linux federation containers.

Dedicated host services: PG127.0.0.1:54312, Redis127.0.0.1:56312. Backend E2E uses `test-misskey`, port61812 and explicit private proxy config. HTTP/browser/lifecycle uses `api-layer-runtime`, RedisDB12 and port61912. Adapter tests use `online-adapter`. DB-reset suites are sequential. Runtime and E2E hostnames differ because Redis pubsub spans logical DBs. No shared/production services are used.

From `packages/backend`:

```sh
NODE_ENV=test MISSKEY_CONFIG_JSON=/workspace/api-layer-port/test-proxy-config.json \
  bun --preload /workspace/api-layer-port/localhost-dns-preload.mjs \
  ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts \
  --execArgv=--preload --execArgv=/workspace/api-layer-port/localhost-dns-preload.mjs \
  --reporter=verbose --reporter=json \
  --outputFile=/workspace/api-layer-port/approved-backend-e2e-full.json
```

The fixture resolves only localhost loopback for the dedicated Webhook server. Other DNS/proxy policy stays enforced. Bun ignores the earlier NODE_OPTIONS import attempt; explicit main/worker preloads are required in this environment. This is environment setup, not a product DNS change.

API generation uses explicit JSON-as-YAML input because the pre-existing `.config/default.yml` is empty:

```sh
MISSKEY_CONFIG_YML=/workspace/api-layer-port/test-config.json \
MISSKEY_CONFIG_JSON=/workspace/api-layer-port/api-generation-config.json \
  bun run generate:api-types
```

Run generation before type checking: generation rebuilds misskey-js declarations and the operations cannot safely overlap. No public SDK source difference is expected.

Federation uses private Compose project `cloud-api-layer-fed`, fresh PG/Redis volumes, CA/proxy fixtures and isolated networks. slacc and Rust are built inside the Linux builder into separate container-native volumes. The Cargo-less tester invokes compile-config and the exact federation Vitest config after that build. Disposable native cache volumes may be retained; application/data containers and volumes are collected.

The product UI check uses existing Vite assets plus the existing en-US LocaleInliner, not a fresh complete frontend build. Chromium performs the actual login, setup-dialog dismissal and API-console ping/test flow, then same-origin API requests. Private fixture credentials are removed; the credential switch is disabled before echo/screenshot.

## Review, failure and baseline classification

- Independent reviewers work from fresh context. Findings and correction review are in `independent-review.md` and `approved-independent-review.md`. Source review is distinct from executed tests. CodeRabbit installation was attempted in this phase: CLI absent127; installer HTTP403/exit22 under enforced network policy. CodeRabbit is SKIPPED; no external review job or policy bypass occurred.
- Pre-approval HTTP comparison failed on users/search and malformed-policy bodies; paired wrong-token requests timed out. These real failures remain in `pre-approval-evidence.tar.gz` and the historical behavior.csv rows. They are resolved only through the three explicit approved contracts above.
- The first approved focused E2E run had66 passes, one failure and eight skips after a suite-setup error. The routing failure was the real DELETE/no-length parser regression; policy setup incorrectly used Vitest expect.poll outside a test. Both were corrected and rerun. Skipped tests in that failed run are not successes.
- One lifecycle harness mistakenly launched the browser in kill-child mode without creating browser credentials and failed before the fault check. Restricting UI execution to browser-only mode fixed the harness; the separate browser run and corrected three-mode lifecycle run passed. This was a test-harness error, not a product regression.
- One approved typecheck overlapped API generation and saw temporarily absent misskey-js declarations. Sequential generation followed by typecheck passed. This was an artifact-generation race, not a product type regression.
- Earlier full E2E attempts failed on external raw.githubusercontent.com/localhost DNS and Webhook delivery, with a reporter error while rendering an unhandled failure. Original `d875b7e3` code in a separate namespace reproduced five Webhook failures (1,111 original files matched Git blobs). This establishes BASELINE under that cloud setup, not a proven product defect. Explicit proxy and correct Bun worker loopback preparation allowed later full runs to pass.
- Other preserved failed setup attempts: wrong PostgreSQL fixture env name; empty default.yml generation; unavailable process children interface; missing localized browser chunks, setup-dialog/button selection and UI-generated empty optional id. Corrected executions have separate records. None of these failures is silently converted to PASS.
- The historical local move.ts abort/unhandled-response defect is not proven reproduced in cloud. A cloud suite that passes move.ts does not erase the old local evidence or establish its cause.

## Git, CI and persistence

Native kernel commit: `ecde8692b7` (`feat(backend): add native Rust API execution kernel`), followed by the TS integration/tests/docs commit on the same branch. Normal push advances existing draft PR #48 without editing its title or body. Hosted CI must be checked on the final pushed SHA and captured separately in `ci-final.json` and the final task report; local successes do not imply hosted-CI success. This tracked local-validation record does not predict a CI outcome before push.

Tracked commits survive checkout. Workspace evidence requires explicit retention before this cloud workspace disappears; no remote persistence is implied. Ignored builds/caches, Docker images/volumes/networks, private configs, temporary uploads and browser state are disposable. Future sessions must not assume their presence.

The final credential-free archive is `/workspace/api-layer-port/api-kernel-evidence.tar.gz`, accompanied by SHA256 and a file manifest. It contains sanitized evidence and source hashes/patch, excluding private configs, credentials/keys, native outputs and docs/superpowers. `cleanup.json` records process/service/credential cleanup. The earlier `pre-approval-evidence.tar.gz` is historical and remains separate.

CHANGELOG candidate (not edited): Move common API routing, authentication, validation and HTTP response handling to the experimental Rust backend.
