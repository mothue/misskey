# Backend E2E investigation (2026-10-23)

## Results

- **PASS, scoped:** `test/e2e/timelines.ts`, 378 tests passed, 135.77 s. Log: `built/ts-to-rs/candidate-timelines-diagnostic.log`.
- **FAIL, full suite incomplete:** complete candidate E2E invocation exited 1 without a suite summary. Log: `built/ts-to-rs/candidate-backend-e2e-diagnostic-full.log`. It advanced beyond timelines and multiple later suite resets, then crashed with an unhandled stream error during Webfinger response disposal. No full E2E pass is claimed.
- **BASELINE reproduced:** a local, deterministic HTTP response disposal fixture exits 1 on both candidate and exact baseline `d875b7e3c737b7f9f75331fffb691f6a2b97f055`, under Bun `1.4.4-canary.1+ea6a2c7b7` (macOS arm64). Logs: `candidate-http-disposal-repro.log`, `baseline-http-disposal-repro.log`.

All host runs used explicit `MISSKEY_CONFIG_JSON=/Users/mothue/Desktop/projects/misskey/built/ts-to-rs/host-test-config.json`; DB 54312 and Redis 56312 were exclusively assigned to this investigation. Candidate E2E used the already-built current `built-test` output with `NODE_ENV=test bun --preload <ignored diagnostic preload> ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts`, from `packages/backend`. The scoped run adds `test/e2e/timelines.ts`.

## Root cause of the reproduced crash

`HttpRequestService.send()` registers a disposer for node-fetch response bodies. When a non-OK response is rejected, the catch path calls `disposeResponse()`. The disposer calls `cleanup()` before `controller.abort()`; cleanup removes its body error listener. The subsequent abort emits a stream error without a handler, crashing Bun. Full-run stack: `abort` -> response disposer -> `disposeResponse` -> `send` -> `getJson` -> `webfinger`.

The ignored `built/ts-to-rs/http-disposal-repro.ts` starts a loopback HTTP server returning status 404 and an unfinished body, invokes `HttpRequestService.getJson()`, catches its expected `StatusError`, and waits briefly for the asynchronous abort error. Both source roots fail identically without external networking or test DB involvement:

```text
REPRO_SOURCE_ROOT=<candidate-or-baseline-root> bun --tsconfig-override <root>/packages/backend/tsconfig.json built/ts-to-rs/http-disposal-repro.ts
```

Candidate and baseline `packages/backend/src/core/HttpRequestService.ts` have identical SHA-256 `069b1d83700b0f8b7b1e7d6937c0b5cdb078820d9ed6718f9386e0e2acbf9d57`. Baseline checkout HEAD was verified as the exact approved commit. This is existing response-lifecycle behavior, outside the three migrated business handlers. No source fix was made.

## Earlier timeout remains distinct

The earlier `candidate-backend-e2e.log` terminated on unhandled got `ETIMEDOUT`, followed by a Vitest formatter `inspectClass` crash. Its precise URL/cause was not recovered; do not label that exact timeout baseline-proven. A diagnostic preload recorded safe URL origin/path for unhandled rejections on retry, but none occurred in either retry. The standalone timelines rerun passed all 378 tests; direct HTTP requests to its GitHub fixture URL also succeeded. Those observations establish non-determinism, not the original timeout's exact cause.

## Cleanup

After the full candidate process exited, a process inventory found no remaining E2E Vitest processes or backend Rust children. No DB/Redis services were stopped, and benchmark port 61912 was untouched. No production code, native addon source, commits, or pushes were changed by this investigation.

## Completed per-file coverage

After the benchmark gate, all 28 remaining files ran sequentially in separate parent process groups, without diagnostic preload or error suppression. Each invocation used the same explicit configuration and the unmodified E2E setup, verbose plus JSON reporters, and a 600-second per-file timeout. The runner terminated only remaining descendants in its own process group after each file exit. No timeout occurred. Total runner duration from per-file records: 279.23 seconds.

**29-file inventory: 28 PASS, 1 BASELINE blocker, 0 new unexplained FAIL, 0 unexecuted files.** The 27 new successful reports contain 895 passed, 2 skipped, and 20 todo tests. With the prior 378-test timelines PASS, independently completed files establish **1,273 passed, 2 skipped, and 20 todo tests**. This is not a full aggregate-suite PASS; `move.ts` remains incomplete.

`move.ts` exited 1 after 8.81 seconds, with the identical baseline-proven Webfinger response disposal crash, and produced no JSON report. Its verbose log records the first three local-alias tests passing; the next test in source is `Able to set remote user (but may fail)` at line 95, which resolves `@syuilo@example.com`. This correlates the fatal E2E file and source trigger without suppressing the failure. Remaining tests in that file are unverified. The exact source-level local 404 repro described above, rather than a complete baseline `move.ts` invocation, establishes baseline equivalence.

Artifacts: `built/ts-to-rs/e2e-per-file/results.json` (exit codes, duration, timeout/report presence), one `.log` and successful `.json` per file, `e2e-per-file-progress.log`, and `run-e2e-per-file.py`. Typical invocation from `packages/backend`:

```text
MISSKEY_CONFIG_JSON=/Users/mothue/Desktop/projects/misskey/built/ts-to-rs/host-test-config.json NODE_ENV=test bun ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts --reporter=verbose --reporter=json --outputFile.json=<absolute-report-path> test/e2e/<file>.ts
```

| File | Status | Completed test counts |
| --- | --- | --- |
| `test/e2e/2fa.ts` | PASS | 7 passed; 0 skipped; 0 todo |
| `test/e2e/antennas.ts` | PASS | 87 passed; 1 skipped; 4 todo |
| `test/e2e/api-visibility.ts` | PASS | 78 passed; 0 skipped; 0 todo |
| `test/e2e/api.ts` | PASS | 20 passed; 0 skipped; 0 todo |
| `test/e2e/block.ts` | PASS | 6 passed; 0 skipped; 0 todo |
| `test/e2e/channel.ts` | PASS | 1 passed; 0 skipped; 0 todo |
| `test/e2e/clips.ts` | PASS | 97 passed; 0 skipped; 2 todo |
| `test/e2e/drive.ts` | PASS | 4 passed; 0 skipped; 0 todo |
| `test/e2e/endpoints.ts` | PASS | 92 passed; 0 skipped; 0 todo |
| `test/e2e/exports.ts` | PASS | 5 passed; 0 skipped; 0 todo |
| `test/e2e/fetch-resource.ts` | PASS | 84 passed; 0 skipped; 4 todo |
| `test/e2e/fetch-validate-ap-deny.ts` | PASS | 2 passed; 0 skipped; 0 todo |
| `test/e2e/ff-visibility.ts` | PASS | 20 passed; 0 skipped; 0 todo |
| `test/e2e/move.ts` | BASELINE blocker | Crashed; no complete JSON report |
| `test/e2e/mute.ts` | PASS | 19 passed; 0 skipped; 0 todo |
| `test/e2e/nodeinfo.ts` | PASS | 2 passed; 0 skipped; 0 todo |
| `test/e2e/note-notify.ts` | PASS | 8 passed; 0 skipped; 0 todo |
| `test/e2e/note.ts` | PASS | 52 passed; 0 skipped; 0 todo |
| `test/e2e/oauth.ts` | PASS | 52 passed; 0 skipped; 0 todo |
| `test/e2e/renote-mute.ts` | PASS | 7 passed; 0 skipped; 0 todo |
| `test/e2e/reversi-game.ts` | PASS | 1 passed; 0 skipped; 0 todo |
| `test/e2e/streaming.ts` | PASS | 53 passed; 0 skipped; 0 todo |
| `test/e2e/synalio/abuse-report.ts` | PASS | 8 passed; 0 skipped; 0 todo |
| `test/e2e/synalio/user-create.ts` | PASS | 3 passed; 0 skipped; 0 todo |
| `test/e2e/thread-mute.ts` | PASS | 2 passed; 0 skipped; 0 todo |
| `test/e2e/user-notes.ts` | PASS | 1 passed; 0 skipped; 0 todo |
| `test/e2e/users.ts` | PASS | 179 passed; 1 skipped; 10 todo |
| `test/e2e/well-known.ts` | PASS | 5 passed; 0 skipped; 0 todo |
| `test/e2e/timelines.ts` | PASS (prior isolated run) | 378 passed |

After the runner finished, process inventory confirmed no runner, E2E Vitest process, or backend Rust child remained. No production source was edited.
