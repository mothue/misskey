# API kernel: approved compatibility corrections and completion plan

**Approved by the user on 2026-10-23 (JST) with “続けて”.** The three corrections are implemented and independently reviewed; final verification and Git/CI outcomes are recorded in [api-kernel-validation.md](api-kernel-validation.md). This plan resolves the three public behavior differences described in [api-kernel-validation.md](api-kernel-validation.md), then completes the already approved common-API migration unit. The user requested consultation when public behavior must change. Normal implementation/test/review/Git operations are already authorized; PR title/body must remain unchanged.

## Objective and scope

The user allows three deliberate deviations from the old TS failure behavior:

1. Non-string `i` returns a completed400 instead of leaving the request unfinished. String/null/omission and Authorization-header precedence retain existing behavior; no business callback runs for rejected input.
2. Anonymous APIs with `requiredRolePolicy` use anonymous base policies. In particular users/search proceeds when canSearchUsers is true and returns the existing ROLE_PERMISSION_DENIED403 when false. Only an actual authenticated root user gets the root bypass; absent user/root IDs must never compare as an administrator.
3. Unevaluable role-policy values retain500 but use the fixed structured INTERNAL_ERROR contract. Accepted policy input is unchanged. Valid arrays and currently supported iterable strings retain their evaluation/response; malformed values never authorize a request or expose runtime exception details.

Keep the approved five native business handlers, 433 legacy registry callbacks and special-flow delegation boundary. No additional business-handler migration, schema, frontend product, slacc, rocka, benchmark, external service or RPC boundary is added by this decision.

## Main changes

- `packages/backend-rust/api-runtime/src/lib.rs`: finish the invalid-token-type response; make root bypass require a user; evaluate requiredRolePolicy with optional user and empty anonymous assignments.
- `api-runtime/src/response.rs`: reuse existing stable public error formatting, adding a focused invalid-input response only if needed.
- `api-auth/src/lib.rs` and focused tests: preserve anonymous default-policy semantics and fail-closed invalid policy evaluation.
- `packages/backend/test/e2e/rust-api.ts` (or a related focused role-policy file): public HTTP token-type and policy cases using one dedicated DB, sequential reset/restoration and private fixture credentials.
- Migration contract, behavior records and evidence: document the exact approved baseline exceptions and final checks. No PR text edits.

## Types and compatibility

Explicitly distinguish token omission, null, empty string, false, zero, arrays and objects; valid Authorization overrides body `i` as before. Keep the distinction between absent user and a real root-user id. Policies are loosely typed stored JSON, so test accepted valid strings/arrays and wrong container/element types rather than asserting a TS interface. Preserve JS numeric/UTF-16/property-order behavior in all unaffected requests. Expected known API errors keep their code/id/kind/status/headers. Only the three explicit failure contracts above may differ from baseline.

## Validation and completion criteria

1. Public HTTP token-type requests finish within a bounded test timeout with400; no callback/fallback/retry/restart. Valid token/header precedence cases remain equal to baseline.
2. Anonymous search with root configured and unconfigured, true/false canSearchUsers, authenticated non-root and root all obey the intended policies and validation order. Forbidden anonymous search returns403 without invoking the business callback.
3. Actual accepted malformed uploadableFileTypes inputs `17`, `[false]` and `{}` return the fixed500; valid array/string probes and restored policies remain equal. Restore test settings in finally.
4. Run the 475-case differential and 30-case multipart matrix again. Only approved, named cases may deviate; paired timeouts are no longer accepted for invalid-token-type input.
5. Rerun affected Rust tests/fmt/clippy, TS types/lint/unit/public HTTP/E2E and required integration checks. Record every actual PASS/FAIL/BASELINE/SKIPPED; do not borrow earlier success for changed behavior.
6. Have an independent reviewer check the corrections, resolve findings, perform final repository safety checks, and commit the coherent migration unit without bypassing hooks.
7. Normally push `feat/ts-to-rs`, preserve draft PR #48 title/body, and inspect CI on the final pushed SHA. Fix migration-caused CI failures and repeat only relevant checks. Never merge/close/rewrite history.
8. Collect test processes/services and save a credential-free evidence bundle. Only then report implementation and acceptance complete, or explicitly identify a remaining external blocker.
