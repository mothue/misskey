# Reusable native migration validation

## Generated status

[The native/TS/held table](native-status.md) is generated from the actual endpoint registry, native Rust dispatch and TS callback rejection guard. Hold decisions are recorded in `scripts/ts-to-rs/native-holds.json`; counts are computed rather than maintained manually. The table records source selection, not CI acceptance.

```sh
bun scripts/ts-to-rs/native-status.ts
bun scripts/ts-to-rs/native-status.ts --check
```

## SDK response validation and semantic comparison

`response-validation.ts` compiles `packages/misskey-js/src/autogen/types.ts` with TypeScript and validates unknown response values against endpoint/method/status contracts. It distinguishes omitted properties, null and actual empty response bytes, checks unions, literal values, arrays, tuples and index signatures, and fails on unsupported types. SDK `any`/`unknown`/unconstrained object contracts remain explicitly opaque. It does not invent constraints absent from the SDK.

`differential.ts` requires a paired successful HTTP capture for every currently selected native endpoint, obtained from the same generated inventory. Each capture includes `id`, `endpoint`, `status`, `body`, `rawBody` and optional `method`. Nonempty response bytes must decode to the supplied body. Keep capture files private: authenticated API bodies can contain sensitive fields. The report contains diagnostics, not response values.

```sh
bun scripts/ts-to-rs/differential.ts BASELINE.json CANDIDATE.json
bun test scripts/ts-to-rs/*.test.ts
```

SDK conformance and semantic parity are independent. A new semantic difference or native-only type violation is **FAIL**, exit 1. Identical inherited SDK violations are **BASELINE**, reported explicitly rather than relaxed or counted as type successes. Coverage gaps fail. HTTP `ping` timestamps are checked against each capture's observed request interval before comparing a clock-independent view; SDK validation receives the original body. Every other response field remains part of the comparison.

Fresh paired captures cover every native endpoint against unmodified pure-TS develop `d875b7e3c737b7f9f75331fffb691f6a2b97f055`: semantic parity and coverage **PASS**. There are 30 cases, including an extra registry empty-response case: 23 SDK **PASS**, seven identical SDK **BASELINE**, zero native-only failures. The registry arbitrary JSON/204 and role policy/condition declaration gaps are tracked in [mothue/misskey#55](https://github.com/mothue/misskey/issues/55). No schema or API behavior exception was added.

## Mutation witness

In an isolated checkout/binary, the native `test` handler was deliberately changed to omit an explicit `nullableDefault: null`. The actual HTTP comparison failed with exit 1 on `test`, both in semantic parity and SDK conformance. Restoring the exact private source and original binary returned semantic parity and coverage to **PASS** (the same seven inherited SDK diagnostics remain **BASELINE**). Main product source and binary hashes stayed unchanged throughout. Unit coverage also checks an optional-field omission permitted by the SDK, which must still fail semantic comparison.

## Pure TypeScript measurement

A separate paired cloud run compares native `notes/show` with unmodified pure-TS develop `d875b7e`, rather than only the hybrid baseline. Identical dedicated public-note fixtures, the same Bun version and the unchanged benchmark `phase`/`summary` runner were used. The baseline had zero Rust children; the candidate had one. Three AB/BA/AB pairs each used 100 warm-up and 1,000 measured requests, concurrency 8; 6,000 measured requests passed response comparison.

For the representative anonymous public-note scenario, median throughput was 534.4 → 1,729.1 req/s (+223.6%), and p95 was 23.504 → 6.997 ms (-70.2%). This is one endpoint/scenario on a small test fixture, not a backend-wide or production result.

Private commands, exit codes and logs: `/workspace/native-queue/partial-bulk/results.jsonl`. Measurement provenance and six paired runs: `/workspace/native-queue/pure-ts-benchmark/`. Independent reviews are recorded separately. Product UI checks are not applicable to these CLI tools; no frontend product code changed.
