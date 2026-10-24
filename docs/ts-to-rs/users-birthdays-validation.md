# Held followed-user birthday candidate

`users/get-following-users-by-birthday` continues to execute its original TS business body. The native candidate is compiled but excluded from `ACTIVE_READ_ENDPOINTS`; the TS callback rejection guard and startup protocol are unchanged. This is a reviewed checkpoint, **not an accepted native slice**. No compatibility exception was adopted.

## Candidate and compatibility blocker

The candidate reuses the existing native `UserLite` packer. It selects the original two columns through the existing following/profile join, uses the authenticated viewer's following graph, preserves birthday-only ascending ordering (January before December in wrapped ranges), and adds no target visibility/block/mute filter, secondary ordering, schema, index, cache or helper RPC. Existing authentication, `read:account` scope, validation, response handling and Rust-child lifecycle stay in the shared API layer.

Actual TypeORM query-builder output confirms explicit `OFFSET 0`. Pagination keeps finite integer JS Number formatting, including negative, rounded large and exponent values, rather than narrowing to unsigned or signed 64-bit bounds. Only a private finite-integer wrapper emits numeric SQL literals; no input string enters SQL text. WHERE values remain bound. The typed database row uses text IDs and SMALLINT birthdays.

Independent review found that the unchanged `oneOf` accepts a single-day object with additional own `begin`/`end` keys, while the TS handler selects range arithmetic from those keys. The candidate's stricter range decoder rejects some of these accepted inputs. Actual public HTTP reproduction before disabling the candidate:

| Additional range fields on `{month:10,day:24}` | Original TS body | Provisional native candidate |
| --- | --- | --- |
| `begin:{month:"1",day:1}`, `end:{month:"12",day:31}` | 200, coerced range 101–1231 | 500 |
| `begin:{month:1,day:0}`, `end:{month:12,day:31}` | 200, range 100–1231 | 500 |
| `begin:null`, `end:{}` | 500, legacy TypeError detail | 500, different detail |

These are candidate compatibility failures, not approved infrastructure-fault exceptions. Ordinary strict field checks remain useful, but the successful schema branch cannot justify assuming every extra field is typed. Retain the original arithmetic, property access, null/omission and error behavior before activation. Two success-path regressions are now in `test/e2e/users.ts`; the full three-case actual HTTP RED evidence is retained privately. The TS validator/handler disagreement is tracked separately in [mothue/misskey#59](https://github.com/mothue/misskey/issues/59); no fix or public behavior change is included. These cases were reproduced against the hybrid baseline's original TS body, not pure-TS develop.

## Local calendar work

The internal date helper follows the original `setHours`, `setMonth`, conditional `setFullYear` order, including calendar overflow, fresh clock reads and compatible DST gap/fold disambiguation. The original Bun handler statements supplied the oracle: 120 birthday fixtures, 20 setter probes and 156 explicit-zone offset/field samples were exercised privately. Inline Rust tests retain the birthday/setter fixtures, including midnight gaps, folds, skipped dates and leap-day rollover.

Bun 1.4.4's actual embedded ICU data reports timezone version 2026a. Jiff 0.2.15 and bundled jiff-tzdb 0.1.6 are pinned to that data; system-zone discovery and OS zonefile features are disabled. No additional internal crate was created: this is a business module.

The provisional bootstrap transmitted Bun's effective canonical zone from `Intl.DateTimeFormat().resolvedOptions().timeZone`, rather than interpreting raw `TZ` differently in Rust. That bootstrap change was removed when the candidate was held, avoiding new startup requirements for all existing APIs. The dormant configuration uses an explicit bundled UTC placeholder. **Restoring effective-zone initialization is required before activation**; the shipped held path does not claim native timezone parity. No universal extreme-clock or runtime `TZ` mutation compatibility is claimed.

## This checkpoint's execution evidence

All commands used dedicated cloud PostgreSQL/Redis and explicit runtime/test configurations. Database-resetting suites ran serially. Private root: `/workspace/native-queue/user-birthdays`; `results.jsonl` records full command, exit code, duration and sanitized `logs/*.log`. Raw comparisons, original sources, oracle outputs, source hashes and independent review are retained there. No credentials are committed.

| Check | Result | Evidence / practical limit |
| --- | --- | --- |
| Original TS HTTP | PASS, exit 0 | 62 cases each in UTC, Asia/Tokyo and America/New_York, captured before product integration. |
| Provisional native ordinary HTTP | PASS, exit 0 | Same 62 cases in each of those three zones: statuses, bodies and relevant headers match. Only per-request correlation UUID normalized. This limited result does not override the later RED failure. |
| Review-added actual HTTP differential | FAIL, exit 1 | Three mismatches out of 65 cases: two 200→500 and one 500 detail difference. `review-red-http-comparison.json`; candidate activation removed. |
| Final held HTTP | PASS, exit 0 | All 65 cases match original TS body; `held-http-comparison.json`. This verifies continued TS execution, not native parity. |
| Final Rust workspace tests | PASS, exit 0 | 281 passed, 11 ignored; `held-rust-tests.log`. Ignored tests are not counted as successes. |
| Final Rust fmt / clippy | PASS, exit 0 | `cargo fmt --all -- --check`; workspace/all-target clippy with `-D warnings`; `held-rust-lint.log`. |
| TS checks / production build | PASS, exit 0 | Backend production/test/federation type checks and production TS/Rust build; `held-types.log`, `held-build.log`. |
| API generation | PASS, exit 0 | Explicit private JSON/YAML config; `bun run generate:api-types`. No generated SDK source change. |
| Related backend E2E, provisional native | PASS, exit 0 | users + API visibility: 269 passed, 1 skipped, 10 todo, before adding the newly discovered extra-field regression checks. |
| Related backend E2E, final held state | PASS, exit 0 | 269 passed, 1 skipped, 10 todo, including accepted-extra-field regressions; `held-related-e2e.log`. After the cleanup lint adjustment, focused birthday tests passed 2/2 (200 unrelated tests skipped); `held-fixture-cleanup-e2e.log`. |
| Rust subprocess lifecycle, provisional candidate | PASS, exit 0 | One child starts, SIGTERM produces 503 for birthday and existing core operations, host survives, no automatic restart; `lifecycle-result.json`. |
| All-native SDK differential | SKIPPED | No new endpoint activated; preparation stopped at the compatibility hold. Existing results are not presented as fresh runs. |
| Paired benchmark / activation gate | SKIPPED | Semantic acceptance failed first; no measurement, speed claim or benchmark comment for this candidate. |
| Product birthday widget | SKIPPED | Source-only real-widget setup retained privately; no native UI acceptance claimed. |
| Fresh local federation | SKIPPED | Inactive handler checkpoint with no changed shared execution/startup path. Final pushed-SHA CI federation still needs checking. |
| Independent review | PASS for inactive checkpoint | Fresh-context reviewer found the three-case compatibility blocker; confirmed it cannot execute after the hold. Candidate activation remains unapproved. |
| Shipping / SPDX / locale / generated status | PASS, exit 0 | Configured lint has no eligible source changes. |
| Explicit whole `users.ts` ESLint | BASELINE, exit 1 | Three remaining diagnostics are on unchanged pre-existing source lines; no new-region errors. New cleanup `no-unsafe-finally` finding was corrected. First CLI attempt used an unavailable package-local binary; corrected invocation retained separately. |
| Final pushed-SHA CI | PENDING | Check the actual final pushed commit; no historical success substituted. |

## Resume conditions

Preserve accepted additional-field JS arithmetic/comparison/property-access errors without narrowing the public schema; keep security gates strict. Extend the failing HTTP differential and committed regressions before changing the decoder. Restore explicit effective local-zone bootstrap, re-run cross-zone parity, and validate every native response against the real generated SDK types. Then run the existing paired benchmark and keep the handler held if its gate fails. Activation also requires related E2E, applicable product UI/federation checks, independent review, commit/push and final-SHA CI. None of these remaining conditions is waived by retaining the compiled candidate.
