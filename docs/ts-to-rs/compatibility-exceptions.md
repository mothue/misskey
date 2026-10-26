# Current compatibility decisions

The user's speed-first policy (2026-10-24) supersedes historical blanket fault holds. This is the single current list; older evidence keeps its original results.

Authentication, authorization/roles/scopes, rate limits, validation, HTTP status codes, API error codes and ordinary response shapes remain strict. Security checks fail closed. Infrastructure exceptions never permit bypassing TLS verification or replaying SQL.

| Boundary | Decision / known difference |
| --- | --- |
| Non-string effective authentication token | Previously approved: complete an empty400; preserve header precedence and null/omitted/string behavior |
| Anonymous required-role policies / root identity | Previously approved: evaluate anonymous base policies, deny with ROLE_PERMISSION_DENIED403; root bypass requires an authenticated id |
| Unevaluable role policy | Previously approved: structured INTERNAL_ERROR500 rather than engine-dependent message-only500 |
| DB/Redis connection refusal, DNS/TLS and driver-specific details | Approved best-effort infrastructure parity. Missing/different legacy `info.e` is an approved baseline exception and does not block the24native read candidates. Ordinary status/error-code/security contracts remain strict |
| Hashtag chart windows | Existing TS duplicate/cumulative local-Date intervals are preserved exactly; [issue61](https://github.com/mothue/misskey/issues/61). This is a retained baseline defect rather than a new compatibility exception |
| Saturated PostgreSQL checkout | Native typed CheckoutTimeout currently lacks the legacy `Error` / `timeout exceeded when trying to connect` projection; covered by the infrastructure-detail exception |
| TLS diagnostics | Complex SAN formatting, partial wildcards and competing certificate failures are not exhaustive TS-driver parity; verification remains enforced |
| Exotic pg-pool settings | Explicit legacy single-slot resource policy remains for unsupported exotic comparators; exact scheduling parity is not claimed |

Every activated slice uses native auth, DB, required Redis and response logic without a TS business callback. Measure each against its TS business baseline using the unchanged runner. Slower independent slices stay on TS; a dependency exception needs measured evidence of a faster complete slice. Prioritize hot reads, then writes, then admin/long-tail work. No new fault-injection matrices or wire-validation units without a user-visible bug.

The four date-pagination APIs remain separate: same-millisecond ID boundaries affect ordinary response membership, not infrastructure diagnostics. Minor newly found differences take the safe path and are recorded here without stopping work. One independent review covers a related commit batch; commit/push frequently without rewriting published history. Existing PR title/body, AGENTS.md, frontend product code, slacc and DB schema remain unchanged; never commit docs/superpowers.
