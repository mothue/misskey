# PostgreSQL original wire observations and cancellation routes

Cloud execution on 2026-10-23 (UTC). Start: clean `feat/ts-to-rs` at `4a80a9f5d099c4f61faba8932b7c7bf79cdf3db8`. Source commit: `2a250bf07afbff00310b048edafe544691526801`. This internal unit adds no business-handler activation or public error formatter. The existing five native business handlers and 433 TS callbacks remain; all 24 database-read activations and the four date-pagination APIs remain held.

## Coherent transport boundary

`storage-postgres/src/transport.rs` now creates the selected TCP socket, preserves the original ordered lookup/address traversal and native socket settings, and passes that original stream to `Config::connect_raw`. The internal `wire.rs` wrapper delegates every read/write/vectored-write/flush/shutdown without an independent read, probe, buffer, diagnostic connection or packet modification. It records only the known SSLRequest prefix actually transferred and the actual first reply byte (S/N/E/other); direct TLS/plaintext and unexpected ordering retain unknown reply status. No authentication, SQL or application payload is captured.

Concrete observations include transferred-byte counters, actual terminal I/O direction/kind/errno, verification callback facts, and the delegated TLS connector future's started/succeeded/failed state. **That future includes fallible local SSL setup before `SSL_connect`; its state does not establish actual handshake entry.** Initial independent review found misleading handshake naming; qualified `TlsConnectState` names and documentation resolved it. The original postgres-openssl stream, buffering, TLS policy/SNI/ALPN and channel binding remain delegated.

Connection owns live observation state so later driver I/O can be captured, while each returned fault snapshots immutable facts. Cancellation owns independent state. Debug/Display/Error::source remain redacted; no user/password, SQL, protocol payload or certificate/key contents are formatted. Context remains privately boxed, and opaque auth/count callers retain their existing outputs.

Using the raw driver removes its private SocketConfig, so cancellation changes in the same unit: save only the actual successful IP/port, original hostname/TLS negotiation and socket settings; use the actual token's `cancel_query_raw` over one socket to that saved route. The existing total 500 ms bound covers TLS-factory creation, connect/options, negotiation and capability delivery. No DNS, retry, SQL replay, primary fallback or plaintext downgrade is added. Native factory ordering is preserved (business after socket/options; cancellation before socket). Native Unix connection/cancellation remain unchanged because raw APIs hardcode hostname presence and could alter Unix TLS restrictions.

TCP_NODELAY, keepalive enabled/disabled/idle/interval/retries, Linux TCP_USER_TIMEOUT and per-address deadlines preserve native driver order/platform guards. The adapter adds a direct `socket2 0.6/all` dependency already present at locked 0.6.5 with the same upstream feature union; Cargo.lock changes only its dependency edge. No internal crate or RPC helper is added. Session SET stays outside configured connect/query deadlines, within the online-count operation's existing four-second total; cancellation ownership and shutdown bounds remain intact.

## Isolation, provenance and executed checks

Linux Debian 13 x86_64; Bun 1.4.4, Rust 1.99.0, Node 24.19.0. A fresh private baseline copies 64 backend bundle files plus the previous native executable and binds them with 65 SHA256 entries. Dependency/static links are checked separately. The seven committed source/dependency files match an independent fresh-context review. `final-build-provenance.json` binds the exact native build used for public HTTP and full local backend E2E; unlike the preceding unit, this full suite uses the final source without a later production amendment.

Dedicated project `cloud-pg-raw-transport` uses loopback PostgreSQL 18/Redis 7. Explicit private settings select `test-misskey`/Redis 14 for backend E2E, `online-adapter` for the real lifecycle, `api-layer-runtime`/Redis 12 for HTTP/benchmark, and Redis 15 with owned keys for live Rust Redis tests. Database-resetting tests sharing a DB are sequential. No shared/production service is contacted. Explicit loopback DNS/proxy exceptions leave other traffic under the managed network policy/proxy.

Exact commands, cwd, exit codes, durations and sanitized logs are in `/workspace/pg-raw-transport/results.jsonl` and `logs/`. Federation/benchmark retain their own command ledgers. Private settings/keys are excluded from export.

| Status | Command / fixture | Exit | Result / evidence |
| --- | --- | ---: | --- |
| PASS | `cargo check --locked -p misskey-backend-postgres --lib` | 0 | First applied candidate compiled; `logs/raw-transport-check.log` |
| PASS | `cargo fmt --all -- --check` | 0 | `logs/rust-fmt.log` |
| PASS | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | `logs/rust-workspace-clippy.log` |
| PASS | `cargo test --workspace --locked` | 0 | 120 passed, 4 explicitly ignored live cases; `logs/rust-workspace-tests.log` |
| PASS | `cargo test --workspace --locked -- --ignored --test-threads=1` with dedicated settings | 0 | All 4 live cases passed, including actual PostgreSQL lock cancellation/recovery; `logs/live-adapters.log` |
| PASS | `bun run build:e2e` | 0 | Final Linux Rust/slacc/backend build; `logs/backend-build.log` |
| PASS | `bun run --bun tsc --noEmit -p tsconfig.json`, `test/tsconfig.json`, `test-federation/tsconfig.json` | 0 each | `logs/backend-types.log`, `test-types.log`, `federation-types.log` |
| PASS | `bun ./node_modules/vitest/vitest.mjs run --config vitest.config.unit.ts src/server/api/endpoint-base.test.ts src/server/rust` | 0 | 52 passed in 8 files; `logs/related-ts-unit.log` |
| BASELINE | `python3 http-foundation.py --baseline-only` | 0 | 30 old-build cases captured before rebuilding; not candidate validation |
| PASS | `python3 http-foundation.py` | 0 | 30 paired public HTTP cases, zero differences; `http-comparison.json` |
| PASS | `python3 lifecycle-public.py child-loss` | 0 | Five APIs twice/build remain 503, no child respawn; parent exits0 |
| PASS | `python3 lifecycle-public.py db-refusal` | 0 | Dedicated proxy fault preserves native count 503 and legacy TS 500 with matching info.e |
| PASS | `bun --preload <evidence>/localhost-dns-preload.mjs ./node_modules/vitest/vitest.mjs run --config vitest.config.e2e.ts --execArgv=--preload --execArgv=<evidence>/localhost-dns-preload.mjs --reporter=verbose --reporter=json --outputFile=<evidence>/full-backend-e2e.json` | 0 | Final-source 37 files, 1,475 passed, 2 skipped, 20 TODO; `logs/full-backend-e2e.log` |
| PASS | `python3 federation/probe-cargo-target.py --execute`, `python3 federation/execute-with-monitor.py --execute --minimum-free-gib 5 --reason <recorded rationale>` | 0 each | Nine files, 92 passed, 13 skipped; every phase and cleanup exit 0; `federation/evidence.json`, `federation/results.jsonl` |
| PASS | `bun /workspace/pg-raw-transport/benchmark/measure.ts --execute-authorized` through `benchmark/run.py`, then `python3 benchmark/summarize.py` | 0 each | 34,000 measured + 3,400 warmup; zero errors; `benchmark/results.json`, `benchmark/summary.json` |
| PASS | Independent fresh-context seven-file source review | n/a | Initial naming issue resolved; `review-source.json` |
| PASS | `node scripts/checks/shipping.mjs --base 4a80a9f5d099c4f61faba8932b7c7bf79cdf3db8` | 0 | SPDX/locale safety; ESLint SKIPPED, no target TS/Vue files |

The seven new default PostgreSQL tests and strengthened prior tests exercise partial/vectored/Pending I/O, zero-capacity/prefilled ReadBuf, SSL S/N/E/other and malformed/unexpected ordering, exact forwarded errors/bytes, EOF/zero writes, all terminal I/O directions, no extra reads and redaction. Delegated-future fixtures establish only its actual polling/result lifecycle. Loopback integration covers TLS trusted/untrusted/hostname/direct/SNI/ALPN controls, live query I/O, immutable fault/cancellation isolation, and actual socket flags on business and cancellation TcpStreams through an unchanged-delegating TLS connector. Fake-peer capability tests are distinct from the separate real PostgreSQL lock-release test. Non-Linux platform guards were source-reviewed; their runtime behavior is SKIPPED on this Linux host.

HTTP comparisons preserve statuses, meaningful headers, exact body/property order, omission/null, validation/defaults/additional properties, numeric rounding/nonfinite conversion, duplicate/index keys and UTF-16 edge cases. Only Date headers, range-checked current pong timestamps and validated fresh inner UUIDv4 values are normalized. Unknown endpoint 204, unknown route 404 and method/input errors remain strict.

## Federation isolation and capacity

The dedicated `cloud-pg-raw-transport-fed` project built its own Linux Rust/slacc artifacts. Fresh `nocopy` native volumes and an initially empty private `/tmp/cloud-pg-raw-transport-fed-cargo-target` prevent host-native mixing. An actual bind-marker probe passed before release. All 1,916 frozen source files, native host artifacts and harness files remained unchanged. Default capacity guard is 6 GiB; the leader explicitly released this run at the recorded 5 GiB threshold only after other host Cargo/E2E work ended and this task's host Cargo cache moved to the separate `/tmp` filesystem. Combined nginx cache capacity was bounded to 512 MiB. Sampling every 0.5 seconds recorded minimum free space of 2,074,648,576 bytes on the overlay and 5,402,468,352 bytes on `/tmp`; build, all nine federation files and cleanup completed with exit 0. This is an executed capacity result for this fixture, not a general promise for larger simultaneous builds.

The exact final native executable used by local HTTP, backend E2E and benchmark has SHA256 `d68e992cca9e40487fdab27213f521dcc31e8b5442588d403a4f658f90d639ed`; the paired baseline executable has SHA256 `59d9b0b01518852ee6c2847ac0e9391935475fd6740b677acd0bf4de6c025d0e`. Container federation builds from those same frozen sources independently, without copying either host executable into Linux containers.

## Paired cloud measurement

The existing common runner is unchanged. Baseline `4a80a9f5d099c4f61faba8932b7c7bf79cdf3db8` and candidate `2a250bf07afbff00310b048edafe544691526801` have identical 64-file TS bundles; the 65-file baseline artifact manifest and both observed child executable hashes are verified. Source-set SHA256 is `bc873188ad1a5b332fc2dcc78889cb5c799602144665934750128bdf2e2df744`, and the independent seven-file source review SHA256 is `8c4ca18261ee16e5a88cc20aa7d102b9bdd68e54efe0e631d4f4c58b0e5e4313`. Full runner, harness, source, config-hash and process provenance is retained in `benchmark/provenance.json`.

Five existing native APIs use three paired runs (AB/BA/AB), 100 warmup and 1,000 measured requests per API/build, concurrency 8. Two unchanged TS callback scenarios add infrastructure smoke. All 34,000 measured and 3,400 warmup requests completed with zero HTTP, timeout, transport or validation errors. All eight server parents exited 0; Rust children were collected and owned process groups had no survivors. Startup is measured separately. The interruption fixture was not rerun in this cycle: its prior result is not counted as a new PASS.

| API | Baseline req/s | Candidate req/s | Throughput Δ (%) | Baseline p95 ms | Candidate p95 ms | p95 Δ (%) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `ping` | 4326 | 5208 | +20.4 | 3.572 | 2.351 | -34.2 |
| `test` | 5633 | 5467 | -2.9 | 2.686 | 2.479 | -7.7 |
| `endpoint` | 5887 | 6248 | +6.1 | 2.439 | 2.085 | -14.5 |
| `endpoints` | 4810 | 4541 | -5.6 | 3.487 | 3.639 | +4.4 |
| `get-online-users-count` | 2465 | 2785 | +12.9 | 5.451 | 4.945 | -9.3 |

Values are medians of three runs. [Full 35-row CSV](postgres-wire-benchmark.csv) includes p50/p95/p99, throughput, sampled TS/Rust/combined RSS, ranges, population variance/standard deviation, CV and each paired delta. The comment color heuristic requires all three paired signs to agree and the absolute median-of-runs delta to exceed max(5%, baseline CV, candidate CV); green indicates beneficial direction, red adverse and white otherwise. Here the displayed `ping` and online-count throughput cells are green; the other eight displayed cells are white. This is a short measurement on a shared cloud host, with a small empty dedicated dataset and no CPU isolation. It is neither a significance test nor evidence that this internal change caused a performance improvement. RSS is sampled Linux VmRSS, not peak or unique physical memory. Performance is not an acceptance gate.

## Cleanup and saved evidence

`cleanup.json` records both owned Docker projects with zero containers/volumes/networks, five dedicated loopback ports closed, all final native/backend artifact hashes unchanged, host Cargo cache restored, private container Cargo cache removed, and thirteen private setting/CA files deleted. Known credential values and private-key markers were scanned before private configs were removed; no secret was found in selected sanitized evidence. Logs were also sanitized when captured. Other tasks' settings, volumes and processes were not removed.

Committed documentation and this CSV persist on GitHub after push. Raw sanitized commands, logs, source/review manifests, JSON measurements and export archives are under `/workspace/pg-raw-transport`; they remain workspace artifacts and are not a substitute for Git persistence or CI artifacts. Native build/cache files, test database contents, private settings and process state are not source deliverables. The separate final pushed-SHA CI snapshot and benchmark-comment publication record will be stored with that evidence after remote completion. Actual GitHub visual rendering is SKIPPED unless a browser check is performed; local MathJax parsing alone does not establish it.

## Acceptance boundaries and follow-up

No new runtime test failure or confirmed TS-to-Rust regression was observed in this unit. The initial source-review finding was corrected before commit. Passing these internal transport tests does not establish Bun's public fault strings or full hostname/address behavior. DNS EAI visibility and actual Bun-versus-Rust hostname/multi-address baseline remain unresolved; do not infer EAI from ErrorKind/Display or infer public class/message from an OpenSSL code.

Source research now identifies a further comparison obligation: Bun pg starts TLS/startup after TCP connection selection, whereas the retained native tokio-postgres traversal can proceed to another address after TLS/startup failure. This is source evidence, not a newly measured public regression; controlled two-address runtime comparison must precede formatter/activation decisions. The held 24 APIs are not reactivated on source assumptions.

The independent real-ioredis TS-body baseline from the preceding unit found replayed Reversi cancellation removing an intervening rejoin; [Issue 53](https://github.com/mothue/misskey/issues/53) records the unchanged TS/frontend source and executed body-level scope. The old full runtime/public HTTP was not run, and no fix or native cancellation API is activated here. That Redis operation still needs compatible driver queue/retry/error/recovery behavior or an explicit contract decision.

Local UI is SKIPPED because no frontend/product UI flow changed; HTTP checks do not establish exhaustive visual equivalence. API generation is SKIPPED because meta/paramDef/res/public schemas did not change. No AGENTS.md, frontend/slacc source, schema, locale or CI workflow behavior was changed. No GitHub CodeRabbit invocation or PR title/body edit occurs. Final pushed-SHA CI is pending until publication and will be recorded separately in `ci-final.json`; earlier CI success is not reused as this commit's result.
