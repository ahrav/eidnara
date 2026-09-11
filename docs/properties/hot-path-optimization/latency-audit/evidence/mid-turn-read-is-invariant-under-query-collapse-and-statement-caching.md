# mid-turn-read-is-invariant-under-query-collapse-and-statement-caching

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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

[ismidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L73-L82
[dbexists]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L33-L35
[dbpath]: ../../../../../packages/opencode-plugin/src/shared/opencode-database-path.ts#L65-L66
[tsetup]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L8-L22
[dbcache]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L37-L67
[midturndb]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L84-L138
[newer]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L152-L189
[realuser]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L195-L201
[realpart]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L207-L214
[provexec]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L141-L149
[machine]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-formatting.ts#L29-L42
[jsonfield]: ../../../../../packages/opencode-plugin/src/shared/sqlite-helpers.ts#L25-L27
[sqlite]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L71-L79
[prepareover]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L210-L211
[stmttype]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L304-L309
[midturncall]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1091
[tmidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L97-L674
[tismidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L920-L951
[tdbpath]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L953-L1021
[fixture]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L42-L45
[fixture2]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L698-L706
[tbind]: ../../../../../packages/opencode-plugin/src/shared/sqlite-bind-style.test.ts#L32
