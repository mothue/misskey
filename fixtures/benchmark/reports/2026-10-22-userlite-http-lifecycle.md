# UserLite defaults / HTTP lifecycle benchmark (2026-10-22)

Baseline: `0e01a7825ed73f4359f01f297b2a6c44e7ab25c5`. Changed runtime built after both fixes; exact source and runtime hashes are in the [companion JSON](2026-10-22-userlite-http-lifecycle.json). All measurements ran in a dedicated quiet window with no tests, builds, typechecks or review processes running. Values below are medians of the three per-run statistics, not quantiles pooled across all requests.

## #6: real backend HTTP API

Three alternating before/after pairs. Each scenario: 128-user response-validating preflight, 200 warmup, 1,000 measured requests, concurrency 8, timeout 10s. HTTP latency measures through complete response body consumption. Same deterministic fixture dump and Redis snapshot restored for every scenario; PostgreSQL ANALYZE after each restore. One backend worker. Warmup/preflight/restore/build/startup excluded from latency.

| Endpoint | median p50 before → after | p50 reduction | median p95 before → after | median p99 before → after | median req/s before → after | throughput gain |
|---|---:|---:|---:|---:|---:|---:|
| timeline/home | 37.96 → 25.77 ms | 32.1% | 53.53 → 36.10 ms | 62.13 → 66.52 ms | 202.6 → 283.0 | 39.6% |
| notification/list | 48.38 → 26.82 ms | 44.6% | 74.00 → 39.90 ms | 106.23 → 48.89 ms | 155.6 → 277.5 | 78.3% |

All 12,000 measured API requests succeeded; HTTP errors, timeouts, transport errors and validation failures were all zero. The companion JSON contains every run, p50/p95/p99, throughput, counts and startup/readiness time. These local synthetic results establish improvement in these fixtures; they do not predict production latency. SQL query reduction is separate real-database service-test evidence, not HTTP-instrumented counts.

Home timeline p99 increased by 7.1% in the median of the three runs (62.13 to 66.52ms). Per-run p99 varied substantially; this experiment does not establish an improvement in home timeline tail latency. The real-database regression test separately observed the unnecessary detailed-data queries for one default-schema batch decrease from 11 to 0 for authenticated viewers and from 1 to 0 for anonymous viewers.

Fixture fingerprint: `547f1ce6c91fee92c7a8a5b30e351e5725d9387d3463975c94a78665b88102c4`.

## #25: real signed HTTP delivery lifecycle

A local HTTP wrapper invokes the actual production `ApRequestService.signedPost` and `HttpRequestService`. The peer sends 202 headers and one body chunk, then deliberately never finishes. Each run issues one real curl smoke request plus 64 additional HTTP requests with concurrency 8. The real RSA signature of all 65 upstream deliveries is cryptographically verified. Services use a fixed test key held only in ignored scratch storage.

In all three before runs, after 100ms there were **65 pending upstream responses, 65 sockets and 65 pending 5s service timers**. In all three after runs, all three counts were **0**. All wrapper requests returned 204 and all signatures verified. This demonstrates prompt release after accepted response headers. It is not an end-to-end remote federation test or a latency speedup claim. Lifecycle elapsed time includes the 100ms observation delay.

## #25 follow-up: ordinary completed responses / connection reuse

Separate non-timed functional runs used the same actual signedPost HTTP wrapper, curl smoke plus 64 HTTP requests, and real RSA verification. The upstream completed its 202 response immediately, with either an empty body or the eight-byte body `accepted`.

| Healthy peer | Total TCP connections before → after | Requests reusing connections before → after | Pending responses before → after | Pending 5s timers before → after |
|---|---:|---:|---:|---:|
| Empty 202 | 6 → 5 | 59 → 60 | 0 → 0 | 65 → 0 |
| Small completed 202 | 4 → 4 | 61 → 61 | 0 → 0 | 65 → 0 |

All 65 deliveries in each of the four runs were accepted and cryptographically verified; all wrapper calls returned 204. No increase in TCP connections was observed in these immediate-response cases. Idle keep-alive connections remained available after the change (five for empty responses, four for small responses); they are distinct from stalled pending responses. Connection totals can vary with request scheduling. These were single functional runs collected while other checks ran, with no timing or throughput measurement. Delayed bodies, TLS and internet peers are outside this follow-up's coverage.

Scratch sources/configs are `probe-healthy-{before,after}.ts` / `.config.ts`. Results are `http-healthy-{empty,small}-{before,after}.json` in the ignored scratch directory. To rerun an already-built helper:

```sh
PEER_MODE=small PERF_VARIANT=after bun .benchmark/perf-6-25/probe-healthy-after/probe-healthy-after.js
```

## Reproduction / artifacts

Full raw samples, sanitized curl response JSON, immutable before/after runtime bundles and scratch repro scripts are in ignored `.benchmark/perf-6-25/`. No fixture tokens, private keys, DB dump or full manifest are included in the committed compact summary.

After preserving both runtimes and the original snapshots, run from the repository root:

```sh
.benchmark/perf-6-25/reproduce.sh
```

Equivalent individual measured API run (harness sends real HTTP requests using Bun fetch):

```sh
PERF_REUSE=1 PERF_VARIANT=before-1 \
PERF_RUNTIME="$PWD/.benchmark/perf-6-25/runtime-before/entry.js" \
bun .benchmark/perf-6-25/controller.ts --requests 1000 --warmup 200 --concurrency 8 \
  --json .benchmark/perf-6-25/before-1.json
```

Actual curl smoke commands executed by the scratch harness (JSON request containing deterministic fixture token supplied privately on stdin):

```sh
curl --silent --show-error --fail --request POST --header 'Content-Type: application/json' --data-binary @- http://127.0.0.1:61912/api/notes/timeline
curl --silent --show-error --fail --request POST --header 'Content-Type: application/json' --data-binary @- http://127.0.0.1:61912/api/i/notifications
curl --silent --show-error --request POST --output /dev/null --write-out '%{http_code}' http://127.0.0.1:61914/deliver
```

API curl request bodies use `limit:20` and fixture authentication; notification requests add `markAsRead:false`. Curl responses are saved with sensitive key names removed. First baseline run predates the optional curl hook; all measured runs nevertheless use the same preflight/warmup phases.

Compiler-only build time: before 1.83s, after 1.53s; recorded separately and not claimed as a speedup. Readiness time is measured from spawn until both successful `/healthz/` HTTP and runtime ready IPC. The initial controller was paused after seeding while implementation/tests ran; its overall report timestamps must not be used as startup or benchmark duration.

Final source verification: the three core source hashes captured at build matched immediately before and after the measurement window. The three additional lifecycle caller sources predate the changed-runtime build and their final hashes are also recorded. All six source hashes and the changed runtime entry hash were checked again after measurement.

The feature branch is based on `origin/develop` (`1ae213dbefc9cff216ae26e533e3ff0f523574b1`). The benchmark baseline's additional local commit changes only a frontend test and the misskey-js generator tsconfig; all backend production sources are identical between these two baseline commits.

## Validation

Backend unit suite: 67 files passed, 876 tests passed, 2 skipped. Backend production, unit-test and federation-test typechecks passed. Changed-source ESLint, repository SPDX and locale-safety checks passed. Local CodeRabbit review covered all eight changed backend source/test files and raised 0 issues.
