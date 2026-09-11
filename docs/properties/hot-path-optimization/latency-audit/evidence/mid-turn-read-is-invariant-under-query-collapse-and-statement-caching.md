# mid-turn-read-is-invariant-under-query-collapse-and-statement-caching

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

The discovery sections and first three investigation entries below are
historical. Reader and test links in those sections point to the immutable
discovery baseline, not the replaced implementation. The final investigation
entry records the current proof and its limits.

## Discovery trigger

The audit counts four or more prepared statements per pass for the mid-turn
read and proposes collapsing them or caching the prepared statements. Both
change how the answer is computed. The record fixes what the answer is, so a
cheaper computation can be checked against it, and fixes the connection a
cached statement may run on.

## Evidence trail

- [`isMidTurn`][ismidturn] returns `false` when the OpenCode database does not
  exist, runs [`isMidTurnFromOpenCodeDb`][midturndb] through
  `withReadOnlySessionDb`, and returns `true` on any thrown error.
- [`isMidTurnFromOpenCodeDb`][midturndb] issues one latest-assistant query
  ordered by `time_created DESC, id DESC` with `LIMIT 1`, excluding rows where
  `summary = 1` and `finish = 'stop'`; then [`hasNewerRealUserMessage`][newer]
  runs one candidates query and one part query per candidate; then, when the
  assistant completed without `tool-calls`, one part query at
  [`:124-126`][midturndb] filtered by `session_id` and `message_id`.
- Every JSON read goes through [`jsonField`][jsonfield], a
  `CASE WHEN json_valid(...)` guard, so a malformed `data` row reads as NULL
  instead of
  aborting the statement.
- The candidates query at [`:168-172`][newer] and the per-candidate part query
  at [`:182`][newer] filter parts by `message_id` alone. They are correct while
  `message.id` is unique across sessions.
- [`isRealUserMessage`][realuser] treats a partless message as real and
  otherwise needs one part that parses as an object and passes
  [`isRealUserPart`][realpart]; [`isMachineAuthoredPart`][machine] excludes
  `synthetic`, `syntheticTodoMarker`, `ignored`, and `metadata.marker.kind`.
  The assistant branch at [`:129-137`][midturndb] needs a `tool` part that is
  not provider-executed ([`isProviderExecuted`][provexec]) and not
  machine-authored.
- [`getReadOnlySessionDb`][dbcache] holds one `{ path, db }` in
  `cachedReadOnlyDb`, closes and replaces it when `getOpenCodeDbPath()`
  changes, and `closeReadOnlySessionDb` closes it outright.
- The adapter at [`shared/sqlite.ts`][sqlite] selects `bun:sqlite` under Bun
  and `node:sqlite` otherwise and adds no statement cache on either; the
  `node:sqlite` `prepare` override at [`:210-211`][prepareover] forwards to
  `super.prepare` per call. The "cached statements" text at
  [`:304-309`][stmttype] documents the `Statement` type alias only.
- The pass calls [`isMidTurn`][midturncall] once per run.
- The example-state suite at [read-session-db.test.ts:97-674][tmidturn]
  holds 39 cases against `isMidTurnFromOpenCodeDb`; [:920-951][tismidturn]
  covers idle, missing, and unreadable databases; [:953-1021][tdbpath] covers
  the `OPENCODE_DB` override. Every fixture declares `id TEXT PRIMARY KEY`
  ([:42-45][fixture]). [sqlite-bind-style.test.ts:32][tbind] pins spread
  positional binds.

## Failure scenario

A single-statement collapse changes tie-breaking on same-millisecond rows,
drops the `json_valid` guard so a malformed row throws (which reads as
mid-turn instead of the row's true classification), or scopes the part
subqueries by `session_id` in a way the current predicate does not, so a pass
is treated as mid-turn when it is idle or idle when it is mid-turn. A
statement cache keyed on SQL text keeps a statement compiled on a connection
that `getReadOnlySessionDb` has since closed; under `bun:sqlite` a finalized
statement throws, which `isMidTurn` reports as mid-turn on every pass.

## Timing windows and dependencies

The two table reads run without a transaction, so a writer landing between
the assistant query and the candidates query can make them inconsistent. The
connection is replaced only when the resolved path changes, so the
replacement clause is reachable in production only through a mid-process
`OPENCODE_DB` change; `closeReadOnlySessionDb` has no production caller. The
synchronous API and one connection per process rule out concurrent readers.

## What a test must construct

A differential harness that runs the current predicate and the candidate over
the fixture states plus rows from a second session in the same database, a
read error on an existing file, same-millisecond rows, and malformed JSON
rows and parts, asserting equal booleans. For the connection clause, prepare
statements, replace the connection through the override, and assert the next
execution runs on the new `Database`. The
[plugin checks](../existing-checks.md#plugin-pre-send) cover the example
states and the override; none is a differential and none has a second
session.

The reference is a frozen copy of HEAD's `isMidTurnFromOpenCodeDb` kept in
the test file, not the live function, because the change replaces it. For
the file-identity clause, open the cached connection, replace the file at
the same path (rename a second database over it, or delete and recreate it
with different rows) between two `isMidTurn` calls, and assert the second
call either answers from the new file or the record carries the evidence
that OpenCode never replaces the file; at HEAD the cache compares the path
only ([`getReadOnlySessionDb`][dbcache] at `:55`), so the second call reads
the old inode. `closeReadOnlySessionDb` is exported and already called in
test setup ([`read-session-db.test.ts:8`, `:22`][tsetup]); no test replaces
the file at the same path.

## Investigation log

### Q: Does OpenCode replace `opencode.db` in place?

- Sources examined: [`getReadOnlySessionDb`][dbcache];
  [`openCodeDbExists`][dbexists] and its per-call check in
  [`isMidTurn`][ismidturn] at `:75`; the repository's mentions of
  `opencode.db` ([`opencode-database-path.ts:65-66`][dbpath] describes
  channel databases competing on last activity; the plugin's own marker
  writer in `features/context/compaction-marker.ts` writes rows, not files).
- Findings: Nothing in this repository states whether OpenCode renames over,
  deletes, or recreates its database file. The plugin's cache survives such
  a replacement because the key is the path alone; a deleted file answers
  idle while the handle stays cached.
- Missing evidence: OpenCode's write behavior for its database file.
- Conclusion: needs external input.

### Q: Does a collapsed plan depend on an index in OpenCode's schema?

- Sources examined: The fixture schemas at [:42-45][fixture] and
  [:698-706][fixture2]; the repository for OpenCode's own schema.
- Findings: Every fixture declares only `id TEXT PRIMARY KEY`. OpenCode's
  `message` and `part` DDL is not in this repository.
- Missing evidence: The pinned OpenCode schema and its indexes.
- Conclusion: unresolved, needs the pinned OpenCode schema.

### Q: Is the missing `session_id` in the part subqueries a correctness concern?

- Sources examined: [`hasNewerRealUserMessage`][newer]; the assistant part
  query at [`:124-126`][midturndb], which does filter by `session_id`.
- Findings: The two subqueries return the right rows when `message.id` is
  globally unique. That uniqueness is a property of OpenCode's id scheme, not
  of this code, and the fixtures assert only the primary key.
- Missing evidence: OpenCode's id generation contract.
- Conclusion: unresolved, needs the OpenCode schema or id contract.

### Q: What do the local differential and native cache checks establish?

- Sources examined: [Frozen reference][reference], [differential suite][differential],
  [reader/cache][current-cache], [collapsed queries][current-predicate],
  [native contract][native-contract], [runtime launcher][runtime-launcher],
  and [transform hook witness][hook-test].
- Approval / PR provenance: In the 2026-09-11 continuation for #416 and draft
  PR #480, the user explicitly approves strict session scoping and limits
  frozen-predicate equivalence to associations where `part.session_id` matches
  the owning message's `session_id`. Tests cover both inconsistent directions:
  foreign-only machine/compaction parts produce old-false/new-true, while local
  machine parts plus a real foreign part produce old-true/new-false. This is
  approval of association scope, not arbitrary concurrent-writer equivalence.
- Reference identity: `7ed1e9845af1a76ff04c31d95ea811367a926bb0`.
  All six copied predicate/helper bodies compare identically with that commit
  after TypeScript printing without comments. The reference does not import
  candidate predicate helpers. Its complete file is SHA-256 pinned at
  `71ffca14e993205825465bda9ff34af779289757b3ab0c574f2d7112929b2bff`.
  The shared `jsonField`, `isMachineAuthoredPart`, and `isMeaningfulUserText`
  primitives are live imports on both sides and are not pinned: they change
  together, so the differential proves the query collapse, not those
  primitives. An earlier revision pinned five shared source files; with 16, 13,
  and 8 commits in the prior 90 days on three of them, every unrelated edit
  would fail this test and invite a digest bump. The second-session additions
  roll back after each comparison, and a repeated-comparison test proves reuse.
- Proof-first execution: Before production edits,
  `bun run --cwd packages/opencode-plugin test src/hooks/context/read-session-db.test.ts src/hooks/context/read-session-db-cache.test.ts`
  reported 80 pass, 2 fail. The original example-state differential passed.
  The count witness observed four prepares instead of at most two; both native
  runtimes observed eight prepares after two reads instead of four.
- Earlier local execution: The limited lifetime suite reached 186 pass,
  0 fail, 775 assertions, and the wider reader run reached 394 pass. Those
  checks did not cover native handles removed from the map. Their claimed
  lifetime/accounting sufficiency is superseded by the native-bypass tests.
- Native-bypass red: Before the ownership correction,
  `bun run --cwd packages/opencode-plugin test src/hooks/context/read-session-db-cache.test.ts`
  reported 0 pass, 1 fail. Its subprocesses exposed 21 failures: ten Bun
  survivors after close/replacement, ten Node natives waiting for GC before
  teardown, and Bun skipping native close after a finalizer threw. The removal
  paths are eviction, oversized SQL, oversized binds, and throwing executions
  on the oversized SQL/bind paths. Each uses a saved original native getter,
  not the guarded query handle.
- Budget-sizing evidence: [Time lookups][time-chunks] and
  [part lookups][part-chunks] each cap chunks at 800 IDs.
  The [message-ID producer][message-ids] emits
  `msg_` plus 26 characters. For 800 such IDs and a 30-character session ID,
  the logical bind charge is `2 * (800 * 30 + 30) = 48060` bytes. Evaluating
  the SQL expressions in the five reader modules found 27 prepare sites,
  23 other fixed texts and exactly two chunk texts, or 25 texts for all list
  lengths. Both chunk readers build 800 placeholders and prepare once per
  function call; each final slice is padded with SQL NULL. NULL cannot match
  an ID through `IN`, and empty inputs still avoid preparation. The longest
  SQL text is 2,548 UTF-16 units, below the 4,096-unit cap. Each chunk always
  binds 801 values including the session and NULL slots, below the 16,384-value
  limit. NULL slots add no logical payload bytes, so the normal maximum remains
  48,060 bytes, within 128 KiB. A 64-statement cache fits the entire corpus;
  growing remainders produce no additional SQL texts.
- Chunk red/green: Before resizing the budgets,
  `bun run --cwd packages/opencode-plugin test src/hooks/context/read-session-db-cache.test.ts`
  failed its real 800-ID interleaving case on both adapters: Bun recompiled
  chunk statements and Node recompiled predicate statements, each observing
  seven native prepares instead of five. With the selected budgets, both
  execute repeated time/part chunks between mid-turn reads with exactly five
  prepares total, one native connection and zero native closes. Updated
  retirement tests cross the actual 64-statement and 128-KiB limits.
- Growing-remainder red/green: The variable-width implementation still
  produced 155 native prepares over lists of 801 through 870 IDs. Node also
  closed twice and used three connections; Bun kept one connection but
  recompiled after eviction. The cache-contract command failed on both
  adapters before padding. The fixed-width implementation returns every
  expected timestamp and ordered message/part payload for all 70 remainders,
  accepts frozen input lists, and observes five prepares, one connection and
  zero closes. Mid-turn reads run between every list size. The prior 1,623-text
  inventory and accepted remainder churn are superseded by this fixed shape.
- Integration corrections: The [graph guard][graph-guard] names only the
  child-process helper in `AWAITING_CONSUMER` and updates the exact readonly
  constructor line. It still requires one readonly opening and the same sole
  writer. No directory-wide exemption or graph weakening is added.
- Current execution: `bun run check:repo` passes typechecks, lints, tests,
  builds, the comment gate and compiled-TUI cleanliness. Test counts are
  5,126 pass, 5 skip, 0 fail, with 19,743 Bun assertions: shm-native 21;
  retina-local-fs 56; opencode 3,785; Pi 379; CLI 596 plus 3 skips; e2e 265
  plus 2 skips; root scripts 24. Lints report 73 warnings and one info, no
  errors. `bun run --cwd packages/opencode-plugin smoke` passes all nine
  checks (five WASM, four TUI). Logs are
  `/tmp/opencode/session-db-verified-repo.log` and
  `/tmp/opencode/session-db-verified-smoke.log`. The focused reader/cache
  command (`bun run --cwd packages/opencode-plugin test src/hooks/context/read-session-db-cache.test.ts src/hooks/context/read-session-db.test.ts src/hooks/context/read-session-raw.test.ts`)
  passes 124 tests, 2,258 assertions; its native launcher executes 28
  subprocess cases on Bun 1.3.14 and Node 24.18.0 without adapter skips.
  Earlier focused/wider runs passed 188/465 tests but did not establish the
  full integration gate or normal-chunk budget fit. Counts overlap and are
  not additive. Production changes total 377 added/deleted lines against the
  base; shared writer adapters have only type declarations added.
- Caller/API audit: The [raw readers][raw-reader], [work metrics][work-metrics],
  [availability resolver][availability-reader] and [marker lookup][marker-reader]
  use only `prepare().get/all`; no production caller retains native statements
  or configures native statement modes. `SqliteReader` exposes only this
  capability. Frozen logical query handles retain SQL and owner references,
  not native statements. Native preparation is deferred to execution.
  Repeated `all` calls across eviction and oversized
  chunks reprepare as needed; named binds deliberately execute uncached. A
  single array bind is normalized before admission, so bounded array binds
  reuse the native statement on both adapters. No native `bind`, iterator,
  result-mode setter, or database handle escapes this owner.
- Lifetime mechanism: Bun finalizes on eviction and in `finally` after each
  uncached execution, including failures. Node 24.18.0's `StatementSync`
  prototype has no finalizer; retirement closes its native connection and
  clears the whole cache. The next execution reopens through the same identity
  checks. This trades warm Node statements for bounded ownership under cache
  pressure, not for the verified normal corpus or growing remainders. More
  than 64 distinct SQL texts outside that corpus, or over-budget bindings,
  still trade Node cache warmth for
  deterministic native release. Close attempts all cached
  finalizers and calls native close in `finally`. Tests retain original native
  getters while applying 128-query pressure, require at most 64 live natives
  between calls, and require every getter to fail after teardown, without GC.
- Query semantics: One user-candidate statement replaces the
  candidate-count-dependent read sequence. The 20-user witness counts three
  executions (assistant row, candidates, completed assistant's parts) versus
  23 frozen-reference executions. The assistant statement selects the same
  ordered row without its parts; the parts statement runs only after the
  `time.completed` and `tool-calls` exits, in the reference's order. A
  materialization witness inserts 40 tool parts under a streaming and under a
  `tool-calls` assistant and requires at most one materialized row across all
  reads; before that ordering, a `LEFT JOIN` on the assistant statement
  returned 40 rows per call. The user statement joins same-session parts
  and scopes the compaction subquery by session. JavaScript still decides
  part-object validity, exact provider booleans, machine flags, and cleaned
  user text. SQL NULL part data
  does not mean a partless message. The absent-assistant sentinel remains
  `(time = -1, id = "")`, not an unconditional match of all user timestamps.
- Corpus scope: Every original predicate example runs before and after adding
  another session, including checks of that session and an absent session.
  Added idle-assistant cases remove the old tool-calls tail that could mask a
  wrong user-part classifier. NULL, malformed JSON, BLOBs, duplicate JSON keys,
  numeric/string/boolean extraction, and nonstandard timestamps are exercised.
  Duplicate message IDs across sessions violate the fixture's primary key.
  Inconsistent associations follow the approval above. Global message-ID
  uniqueness alone does not imply session-consistent part associations.
- Hook integration: [The production call][hook-call] passes the predicate to
  the existing `mid_turn` field. Four hook calls observe false, false, true
  after a committed part edit, then false after renaming an empty database over
  the cached path. No test-only production callback is added.
- Retained-state ledger: One read-only owner holds at most 64 cached native
  statements and one synchronous temporary statement. No removed native
  statement waits for GC. Each SQL key has at most 4,096 UTF-16 units, and each
  cached statement's supported positional binds have at most 131,072 logical
  bytes and 16,384 values. Oversized SQL, oversized binds, and named binds are
  temporary and release in `finally`. Thus logical retained SQL
  payload is at most 524,288 bytes and logical bound payload is at most
  8,388,608 bytes. Results, parsed parts, and candidate arrays are not cached.
  The retained-resident total adjustment is
  `R_after = R_before + L_SQL + L_bind + R_map_wrappers + R_native_statements + R_identity`.
  `L_SQL + L_bind <= 8912896` (8.5 MiB); the other terms include map entries,
  logical handles, native compiled statements and their representations, and bigint
  identity fields. Their heap/RSS sizes are not invented. The path and at most
  one connection remain in `R_before`. Caller-retained logical handles have no
  native resource; an oversized execution's temporary native allocation ends
  before return or throw. This is not a hard RSS or allocator-residency ceiling.
- Assumptions and limits: OpenCode may replace its file, so identity is checked
  regardless of the unanswered maintainer question. `(dev, ino)` is compared
  before reuse and around open. Ordinary same-inode commits do not replace the
  cache. Replacement after the last stat is detected at the next lookup; an
  adversarial open-time ABA sequence is not proven. Equivalence is limited to
  static snapshots: the three reads keep the reference's order, but the user
  candidate class is one joined statement and none of the reads are jointly
  atomic under
  concurrent writers. No transaction is added to equate arbitrary schedules.
  Replacement fixtures use closed
  rollback-journal files, not concurrent WAL-family publication.
  OpenCode's pinned schema/indexes and replacement behavior remain external
  evidence gaps. The association policy is resolved by explicit user approval.
- Conclusion: The enumerated equivalence, native reuse/invalidation, retention
  bounds, and hook-seam witnesses pass locally. All-state equivalence for
  inconsistent cross-session associations is not claimed.

[reference]: ../../../../../packages/opencode-plugin/src/hooks/context/__tests__/mid-turn-reference.ts#L5-L143
[differential]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L57-L892
[current-cache]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L32-L226
[current-predicate]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L228-L358
[native-contract]: ../../../../../packages/opencode-plugin/src/hooks/context/__tests__/session-db-cache-contract.ts#L1-L392
[runtime-launcher]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db-cache.test.ts#L6-L53
[time-chunks]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L373-L411
[part-chunks]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-raw.ts#L131-L164
[message-ids]: ../../../../../packages/opencode-plugin/src/features/context/compaction-marker.ts#L34-L72
[graph-guard]: ../../../../../packages/opencode-plugin/src/testing/module-graph.test.ts#L46-L171
[raw-reader]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-raw.ts#L138-L180
[work-metrics]: ../../../../../packages/opencode-plugin/src/features/context/work-metrics.ts#L268-L312
[availability-reader]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L149-L174
[marker-reader]: ../../../../../packages/opencode-plugin/src/plugin/conflict-warning-hook.ts#L207-L218
[hook-test]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1612-L1666
[hook-call]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1072
[ismidturn]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L73-L82
[dbexists]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L33-L35
[dbpath]: ../../../../../packages/opencode-plugin/src/shared/opencode-database-path.ts#L65-L66
[tsetup]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L8-L22
[dbcache]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L37-L67
[midturndb]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L84-L138
[newer]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L152-L189
[realuser]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L195-L201
[realpart]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L207-L214
[provexec]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.ts#L141-L149
[machine]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-formatting.ts#L29-L42
[jsonfield]: ../../../../../packages/opencode-plugin/src/shared/sqlite-helpers.ts#L25-L27
[sqlite]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L71-L79
[prepareover]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L210-L211
[stmttype]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L304-L309
[midturncall]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1091
[tmidturn]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L97-L674
[tismidturn]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L920-L951
[tdbpath]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L953-L1021
[fixture]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L42-L45
[fixture2]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L698-L706
[tbind]: ../../../../../packages/opencode-plugin/src/shared/sqlite-bind-style.test.ts#L32
