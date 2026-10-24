# PostgreSQL native compatibility controls (cloud, 2026-10-24)

This unit retains typed system resolver and TCP/TLS facts, adds an explicit optional legacy-error projection, and captures Bun's effective address-family selection settings at bootstrap. Existing opaque database callers retain their error contract. **The 24 database-read handlers remain disabled**; this evidence does not approve their activation or the four separately held date-pagination handlers. No TS fallback, SQL retry, request replay, schema change, or new database role is introduced.

## Changes and type boundaries

Linux resolution performs one actual `getaddrinfo` call with the pinned Bun defaults (`AF_UNSPEC`, `SOCK_STREAM`, `AI_ADDRCONFIG`, service `0`). The result owns its original EAI status, immediate worker-thread errno when applicable, and native hints. An owned C string crosses the blocking-task boundary; an RAII owner frees every result, including unwind/cancellation paths. Numeric/scoped IPv6 addresses preserve their socket identity without a DNS probe. Non-Linux resolution retains the native Tokio path without invented Linux facts.

Bootstrap captures `autoSelectFamily`, its attempt timeout and `dnsResultOrder` from the running Bun process. Address order is stable; family selection alternates distinct returned addresses, and disabling it selects only the first. Only failed TCP dials advance to the next address. A successful TCP dial selects the connection: socket-option, TLS and startup errors are terminal for that operation. Cancellation retains the successful IP, socket settings and original connection deadline.

The public `QueryError::legacy_error()` projection returns `Option<LegacyError>` from retained facts and symbolic native status constants. It does not parse Rust/OpenSSL display strings or perform a second DNS/HTTP/SQL operation. Missing/unsupported evidence remains `None`. Error diagnostics stay redacted; public projection is an explicit call, and existing opaque callers do not call it. Original servername bytes remain SNI; UTS #46 normalization, a terminal dot and IPv6 zone handling apply separately to certificate identity checks.

## Fresh execution evidence

Commands ran in `/workspace/misskey`; full commands, exit codes and logs are in `/workspace/api-native-phase/results.jsonl`. Temporary harnesses, immutable baseline `bebe5e7cb0f2dbf3ed10ac8122e149f439de0ba3`, source hashes, binaries and private test configuration are outside the repository. These are current cloud executions, not the historical local validation.

| Check | Result | Command / evidence |
| --- | --- | --- |
| Storage unit and real local socket/TLS tests | PASS, 68 passed, 1 explicitly ignored after scoped-identity review fix | `cargo test --locked --manifest-path packages/backend-rust/Cargo.toml -p misskey-backend-postgres`; `logs/rust-workspace-types.log`, exit 0 (earlier 64-test run retained in `logs/postgres-unit-context.log`) |
| Additional stable ordering/disabled-family/scoped-address tests | PASS, 67 passed, 1 explicitly ignored | `logs/postgres-selection-tests.log` |
| Effective PostgreSQL config tests | PASS, 11 tests | `bun run --bun vitest run --config /workspace/api-native-phase/vitest-config.mjs`; `logs/ts-postgres-config.log`, exit 0 |
| Backend TS source/test/federation type checks | PASS | `bun run typecheck`; `logs/backend-typecheck-retry.log`, exit 0. First attempt exited 127 because Bun was absent from PATH; retained separately |
| Fresh Bun/pg/TypeORM oracle | PASS, 18 modes × 3 client APIs | `fault-matrix/attempt2/bun-1.4.4-baseline.json`; actual Bun 1.4.4, pg 8.23.0, pg-pool 3.14.0, TypeORM 1.1.0 |
| Native query error comparison | PASS, 36/36 exact name/message/outcome comparisons, zero normalizations | `fault-matrix/attempt2/comparison.json`; native read and write paths, actual query execution and shutdown |
| Unicode and terminal-dot TLS identities | PASS, 6 fresh Bun and 4 native successes | `fault-matrix/identity-native/comparison.json`; actual decoded int32 result, extended-protocol queries, read-only setup and original raw SNI |
| IPv6 TLS success controls | SKIPPED | Plain/scoped fixtures stalled before ClientHello; environment/fixture/library cause unresolved, no product-bug claim; `fault-matrix/identity-controls.json` |
| Fixture/process/private-key cleanup | PASS | `fault-matrix/runtime-validation.json`; zero owned running fixtures, private keys or private input files remain |

Cargo used `CARGO_TARGET_DIR=/tmp/api-native-phase-cargo`, two build jobs and disabled debug/incremental output. The host-native helper was never mounted into a Linux container. Runtime fault fixtures used owned random loopback ports and dedicated certificates, not shared or production services. The helper lock audit retained all 112 resolved dependency versions/checksums; only its root package entry differs.

The first native matrix produced **32/36**, not PASS. Two certificate-chain projections differed and were fixed using actual retained chain facts. Two apparent FIN mismatches were a fixture fault: closing while pipelined Sync remained unread produced a real TCP reset. The second fixture half-closes and drains outstanding input. Fresh unchanged-Bun observations, a retained old-binary control and the complete candidate rerun establish the distinction. Attempt1 logs, failed comparisons and original program hashes remain available; they were not overwritten or counted as successes.

## Acceptance limits

The successful finite matrix does not prove all supported PostgreSQL/TLS configurations equivalent. The unresolved ledger `/workspace/api-native-phase/legacy-unresolved.md` includes partial-wildcard/hostname-check differences, complex SAN formats, chain-validation ordering, OpenSSL-only validation errors, unsupported noncertificate TLS failures and session-setup/shutdown cases. Some originate in the existing adapter; none is automatically a TypeScript product defect. Native read handlers stay held until their relevant public error paths have an accepted contract. Existing TS connection-pool capacity and Rust's current single-connection slots are also a separate unresolved review item.

Public HTTP, backend E2E, federation, benchmark, Clippy, final review and final-SHA CI are not claimed by the rows above. Their current executions must be recorded separately when completed.
