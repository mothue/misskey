# Native notes/show slice (cloud, 2026-10-24)

The native operation and its dependencies are implemented as internal modules under `api-runtime/src/business/notes/`. Activation remains held until the existing-runner performance gate is evaluated. The TypeScript business path remains available for endpoints outside the native activation list; no failure-triggered fallback, replay or automatic restart is added.

The native operation owns root lookup and its three ordinary API errors, viewer visibility, nested replies/renotes, UserLite, public attachments, channels, polls/votes, legacy and buffered reactions, remote emoji and instance representations. It reuses the existing native authentication/role evaluator, PostgreSQL source pools and Redis infrastructure. A separate effective reactions Redis configuration is transferred during bootstrap. No business/helper callback enters TS. Frontend product code, slacc, schema, endpoint metadata and public SDK types are unchanged.

## Evidence so far

Baseline: `57fdf2198bb66b1fe761ef23fa983e3d9d1e6865`, detached source at `/workspace/notes-native/baseline-source`; freshly bundled production TypeScript with its own copied baseline native executable. This is the existing hybrid API infrastructure with a TS notes/show body, not the original pure-TS backend. Candidate evidence is a reviewed working-tree implementation on that baseline; exact source/binary/bundle hashes are captured separately for measurement.

| Check | Result | Evidence under `/workspace/notes-native/` |
| --- | --- | --- |
| Rust workspace | PASS, 233 passed, 8 ignored excluded, exit 0 | `logs/rust-workspace.log` |
| Dedicated Redis integration | PASS, 13 passed including the new cache invalidation regression, exit 0 | `logs/native-redis-integration.log` |
| Live PostgreSQL metadata refresh | PASS, 1 explicit ignored test executed, exit 0 | `logs/postgres-meta-regression.log` |
| Rust fmt / all-target Clippy with warnings denied | PASS after three style corrections, exit 0 | `logs/rust-format.log`, `logs/rust-clippy-retry.log` |
| Backend source/test/federation type checks | PASS, exit 0 | `logs/backend-typecheck.log` |
| API generation and production build | PASS, exit 0; no generated public SDK source changes | `logs/api-types-and-production-build.log` |
| Final rebuilt public HTTP differential | PASS, 105 cases, zero differences, exit 0 | `http-comparison.json`, `logs/final-candidate-http.log` |
| Focused and full backend E2E | PENDING | Results will be recorded after execution |
| Performance and activation | PENDING; held | Six ordinary complete HTTP scenarios, unchanged runner |
| Final pushed-commit CI | PENDING | Not yet published |

The 105 cases compare status, semantic JSON (including absent/null distinctions and array order), and content-type/cache-control/vary/access-control-allow-origin headers. Date and transport lengths are excluded. Coverage includes anonymous/owner/other/follower/root viewers, public/home/followers/specified notes, user/server visitor restrictions, shallow replies and detailed renotes, channels, duplicate/missing attachment references, Unicode, public/private badge order, repeated avatar decorations, remote instance/emoji, reaction normalization collisions, buffered reactions, votes, duplicate poll labels and millisecond date truncation. Reactions use dedicated Redis DB11 and a different prefix from main DB13, proving the operation does not silently read the main instance. The SQL/API fixtures are private and dedicated; no shared or production service is used.

## Corrections and limits

Independent review caught an anonymous-view regression: the packed UserLite omits its false signin flag, while the initial note visibility check required a boolean. It now handles the packed optional field explicitly, retaining strict DB decoding; a normal anonymous public-note unit covers the case. The new cache also guards in-flight reads/fills with invalidation generations and serializes Redis publication/invalidation. An explicit dedicated Redis regression proves an older fill is rejected. Replica-eligible reads retain the existing TypeORM source selection; this does not promise stronger database replication consistency than TS.

The first fixture seed expected poll voting to return 200; the existing API correctly returns 204. The fixture assertion was corrected. An added nested E2E initially compared a creation-time nested note to a later read, although creation increments its parent's counters after capturing that nested representation. Its intended detail-level contract is now asserted directly; the separate stable-data differential compares complete nested responses. These were validation-fixture assumptions, not TS product bug findings.

Infrastructure error details remain best effort under [compatibility-exceptions.md](compatibility-exceptions.md). All ordinary auth/security/status/error-code/response contracts remain strict. Performance measurement must pass before activation. Existing two statistics performance holds and four date-pagination holds remain unchanged.

Commands, cwd, exit codes and sanitized logs are recorded in `/workspace/notes-native/results.jsonl`; independent batch review in `/workspace/notes-native/review.json`. Private evidence needs workspace retention. Rebuildable binaries, Cargo targets and test DB/Redis state can be cleaned when capacity is needed; source, logs, measurements and tracked records are retained.
