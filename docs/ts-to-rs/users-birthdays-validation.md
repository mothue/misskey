# Held followed-user birthday candidate

`users/get-following-users-by-birthday` retains its TS business body. Accepted-extra-field compatibility is fixed, but the measured activation gate failed. No performance/compatibility exception was taken. [Generated selection](native-status.md) is authoritative.

## Behavior and foundation

The candidate reuses native UserLite packing, authenticated following/profile selection, original two-column projection, birthday-only ascending order, wrapped January-before-December ordering and offset/limit behavior. It adds no target visibility/block/mute filter, schema, index, cache or helper RPC. Authentication and read:account remain in the shared kernel.

The single-day schema can admit extra begin/end fields; the original TS then performs range arithmetic on them. Native preserves JS multiplication, addition, UTF-16 lexical comparison, NaN and null-access errors instead of incorrectly validating these extras as the unsuccessful range schema. The TS inconsistency remains [mothue/misskey#59](https://github.com/mothue/misskey/issues/59); public behavior is preserved.

The earlier three actual HTTP mismatches remain recorded. Independent review additionally found large-radix cancellation: TS200 versus pre-fix native500. JavaScriptCore ParseInt.h::parseIntOverflow uses right-to-left floating-point accumulation after the forward value reaches2^53; an ideal exact integer rounder also differs from pinned Bun. The corrected shared helper follows actual Bun behavior, with28 oracle cases. Mechanical extraction and behavior correction are separate commits. Final native89-case HTTP passes in three zones resolve both findings.

SQL bounds use typed PostgreSQL text-format parameters to preserve server-inferred SMALLINT/INTEGER rejection and ordinary QueryFailedError details. The direct bytes dependency exposes ToSql's buffer type and was already transitive. The existing finite-integer-only LIMIT/OFFSET literal wrapper remains: binding an unconstrained huge offset changes the ordinary PostgreSQL error versus captured TypeORM behavior. No arbitrary string enters SQL text. This narrow observed compatibility requirement takes precedence over the general parameterization style rule.

The calendar helper preserves setHours→setMonth→conditional setFullYear, fresh clocks, overflow and DST resolution. Jiff0.2.15/tzdb0.1.6 match actual Bun1.4.4 ICU2026a. OS-zone lookup is disabled. Intl's effective canonical zone is sent at bootstrap; typed errors are logged without values. This tested internal foundation is retained while birthday remains held. Extreme clocks and mutating process TZ after bootstrap are not claimed universally equivalent.

Rust returns its native selection in readiness; TS validates types, duplicates and supplied UTF-16 catalog membership, then rejects legacy callbacks using that set. Rust names absent from the TS registry fail before store startup. The copied TS native list was removed, and the status generator validates the same selection/guard. The public listener and unported business executor remain TS.

## This cloud run

Private evidence: `/workspace/native-queue/user-birthdays-resume`, results.jsonl, sanitized logs, raw HTTP/oracles, source/build hashes and independent reviews. The birthday/benchmark baseline is hybrid21b810d with a TS birthday body. SDK global samples separately use pure-TS d875b7e; local samples use hybrid21b810d. Dedicated DB/Redis/configs and serial database-reset suites were used. Historical successes are not substituted for this run.

| Check | Result | Evidence / limit |
| --- | --- | --- |
| Extra-field and radix reproduction before correction | FAIL | Earlier3mismatches retained; new89-case RED has2mismatches, including TS200/native500. |
| Final provisional native HTTP | PASS,exit0 | 89cases each in UTC/Tokyo/New_York; exact bodies/status/stable headers; only correlation UUID normalized. |
| Rust workspace / fmt / clippy | PASS,exit0 | 288passed,11ignored. Locked workspace/all-target -Dwarnings. Extra skill lint set run; unchanged legacy warnings remain, no new helper warning. |
| Types / build / API generation | PASS,exit0 | Backend source/test/federation checks and production/e2e builds; explicit-config SDK generation has no source diff. |
| Transport / script unit tests | PASS,exit0 | 27transport tests;28 ts-to-rs tests. |
| Related provisional E2E | PASS,exit0 | Four files:299passed,1skipped,10todo; extra-range/null and radix cancellation included. |
| Provisional native SDK | BASELINE,exit0;coveragePASS | All33native endpoints covered;105PASS,19identical inherited SDK discrepancies; no new mismatch. Evidence archived in candidate-sdk-evidence. |
| Product widget | PASS,exit0 before radix correction | Three actual TS/native page pairs: UTCempty and Honoluluone-row today/month; API/cards agree, zero page errors. Final held run recorded separately. |
| Benchmark execution | PASS,exit0 | Run22ac25d7-deeb-4e19-b561-2be7d5ae5e0f:36kmeasured+3.6kwarm-ups, allsemantic checks pass;66.77s, three AB/BA/AB pairs,C8. |
| Activation gate | FAIL,exit1 | Five conditions PASS; scoped18-user RPS-4.85%,p95-6.62%. Keep TS. No favorable-run retry. |
| Independent source/metrics review | PASS | Radix P2 resolved after actual green HTTP; all42CSVrows independently recomputed,0discrepancy. Final held recheck retained privately. |
| Final held HTTP/SDK/widget/E2E | PASS / BASELINE | 89HTTPcases PASS;32native SDK coverage PASS with105PASS+19identical BASELINE rows;3widget pages PASS;E2E299PASS,1skip,10todo. This verifies TS retention, not native birthday activation. |
| Fresh Linux federation | PASS,exit0 | Fresh native container build at5852041cac:92passed,13skipped,9files. All1263frozen source/test files and HEAD unchanged; dedicated containers/volumes removed. |
| Rust subprocess lifecycle | PASS,exit0 | One child starts; stopping it produces503 for birthday/core APIs; host survives, no restart, all test processes reaped. |
| Final pushed-SHA CI | PENDING | Check actual final push; record initial failures and retries separately. |

[All metrics and variation](users-birthdays-benchmark.csv) retain the failed gate. The metrics formatter supplies statistics; benchmark-gate.json separately applies the strict all-scenario gate. Skipped/ignored/todo cases are not successes. The first compile failed on a temporary borrow and was corrected. The initial SDK harness free-port probe rejected TIME_WAIT; SO_REUSEADDR corrected that harness check before fresh capture, without sharing a live listener.

## Next work

Revisit birthday only with a concrete performance change and repeat the same semantic/measured gates. Reuse existing packers and the canonical-zone/selection foundation. Remaining users/note/timeline reads require a shared full-ID-generation boundary; separate counters and timestamp-only substitutes alter date pagination/reaction cutoffs. Unblocked remaining reads can proceed while that dependency is designed.
