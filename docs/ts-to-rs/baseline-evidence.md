# First port baseline evidence

Source baseline: `d875b7e3c737b7f9f75331fffb691f6a2b97f055`, preserved using git archive in ignored `built/ts-to-rs/baseline` before production edits. The isolated benchmark backend was built with the existing Rolldown benchmark configuration. This is actual HTTP evidence, not source-only inference.

`behavior.csv` inventories 474 captured requests, including all 438 endpoint descriptions in registry order. JSON fixtures are in `fixtures/ports/http.json`. Private local machine-readable response records are `built/ts-to-rs/baseline-http-expanded.json` and `built/ts-to-rs/candidate-http-expanded.json`; no real credentials appear in the records. Numeric overflow and duplicate keys are sent as raw JSON. Body limit fixtures generate exact UTF-8 sizes around 1MiB. The preserved responses cover validation errors, defaults/null, arbitrary extra objects, lone UTF-16 surrogates and 204 responses.

Capture with `bun scripts/benchmarks/ports/capture.ts URL OUTPUT [BASELINE]`. The optional baseline argument compares status, complete body text and every header after omitting only Date. Ping alone is checked against the request's before/after Unix milliseconds and replaced by a marker. No array sorting, JSON key sorting or error-message normalization is performed.

The preliminary benchmark sample uses the existing load runner with 100 warmup,1000 measured requests and concurrency8. This sample is not acceptance evidence for paired performance. Three alternating pairs, memory snapshots and startup measurements subsequently completed; see [benchmark-evidence.md](benchmark-evidence.md).

Expanded differential passed against the candidate:474 requests, including depth128/1000/10000(all200), exact1MiB exponent arrays and escaped lone-surrogate inputs, and1MiB+1 errors. Explicit expected statuses guard every fixture. Expanded records preserve exact request byte sizes. Headers canonicalize case and order; all values except Date remain significant. Paired-run measurements completed in an exclusive quiet window; reproduce with `bun scripts/benchmarks/ports/paired.ts measure`.
