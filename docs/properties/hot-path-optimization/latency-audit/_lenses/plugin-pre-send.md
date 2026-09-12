# Plugin pre-send surface

Lens: the OpenCode TypeScript plugin's work between receiving OpenCode's
`experimental.chat.messages.transform` call and handing the `transform` body to
the host transport. Source read at HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`
on 2026-09-10 with Bun 1.3.14 and Node v24.18.0. `docs/properties/` has no
TypeScript or plugin catalog part at this HEAD: the README assigns `cli` and
`historian-ts` to wave U7 and no such directory exists, and the
`hot-path-optimization/catalog.md` records are Rust-daemon only. Nothing here
duplicates an existing record.

Every pass of [`run`][run] first freezes the two tool-availability verdicts
from the live message array and then the DB ([availability at
1054-1066][avail]). The `todowrite` verdict then combines the frozen map verdict
with a live permission read: [`resolveCombinedTodowriteVerdict`][combined]
seeds `permissionDenied` from [`cachedToolPermissionDenied`][cached] and, when
`deps.client` is present (it is in production, [hook.ts:138-139][hookclient]),
awaits [`resolveToolPermissionDenied`][permdenied], which issues
`app.agents()` and `session.get()` in parallel, evaluates
[`permissionDisabled`][permdisabled] over agent rules then session rules, and
writes the result into `permissionDeniedBySession` (a 2000-entry
[`BoundedSessionMap`][permmap]). The read is bounded by
[`HOST_SDK_READ_TIMEOUT_MS = 2000`][timeout]; a rejection or timeout is logged
and the seeded value stands ([:97-104][combinedcatch]). The same cache is also
written and read by the `tool.execute.before` capture hook
([hook-handlers.ts:270-292][capture]). The only explicit invalidation is
[`clearToolPermissionDenied`][clearperm] on `session.deleted`
([hook.ts:375][hookclear]); the plugin subscribes to no `permission.*` event
(the event handler dispatches `session.created`, `session.error`,
`message.updated`, `message.removed`, `session.compacted`, `session.deleted`,
and `server.instance.disposed` only). The host consumes `todo_tool_present`
only to decide whether a synthetic `todowrite` pair is captured or injected on
bust passes ([injection.rs:195-230][injection]); the verdict changes prompt
bytes, not authorization.

After the verdict, [`isMidTurn`][ismidturn] opens the cached read-only
connection ([read-session-db.ts:37-67][dbcache], one connection per
`OPENCODE_DB` path, replaced only when the path changes) and runs
[`isMidTurnFromOpenCodeDb`][midturndb]: one latest-assistant query, one
newer-user-candidates query, one part query per candidate
([`hasNewerRealUserMessage`][newer]), and one part query for the latest
assistant when it completed without `tool-calls`. Every `db.prepare` compiles
anew; the adapter ([`shared/sqlite.ts`][sqlite]) selects `bun:sqlite` under Bun
and `node:sqlite` elsewhere and has no statement cache (the "cached
statements" comment at [:304-309][stmttype] describes the `Statement` type
alias only). The pass then builds the body ([`buildTransformBody`][body]),
measures it with `JSON.stringify` in
[`buildPagedModuleTransformPayloads`][paged] against
[`MODULE_PAGE_MAX_BYTES = 512 KiB`][pagemax], and sends each page unchanged
through [`HostModuleTransport.call`][transportcall] to
[`HostClient.request`][request], where [`encodeBody`][encodebody] stringifies
again and [`utf8FrameBody`][utf8body] declares the byte length after replacing
lone surrogates. Delta eligibility depends on [`prefixContentSnapshotsMatch`][prefix]
and [`messageCacheSignature`][signature], which stringifies each message twice
more. Logging on the path goes through [`sessionLog`][sessionlog] and
[`logTransformTiming`][stagelog] into a buffer that [`flush`][flush] writes
synchronously every 50 lines or 500 ms via [`ensureLogDir`][ensuredir] and
[`appendPrivate`][appendpriv]; [`sanitizeField`][sanitize] is the only
transformation applied to log text. The `message.updated` handler logs two
lines per event ([event-handler.ts:175, 191, 222][evlog]). No secret redaction
runs on this path: [`shared/redaction.ts`][redaction] is imported only by
`packages/cli` and `packages/e2e-tests`. The audit's "regex-scans the body once
more" claim is not reproduced on this path; the only body check before send is
[`isModuleCallBodyValid`][bodyvalid], a field test, so that claim is recorded
as unresolved.

## Candidate properties

### todowrite-permission-fallback-never-lifts-a-cached-deny

Type: safety
Check: `always` - on every pass whose live permission read rejects or times
out, `passInputs.todo_tool_present` equals `!cachedToolPermissionDenied(
sessionId, "todowrite")` when that cache entry exists, so a cached `true`
(denied) yields `todo_tool_present: false`. `always` because the fallback
branch runs on every failed read and the condition is defined by code
position, not by an observed defect.
Guarantee: A failed or slow SDK read never turns a previously observed deny
into a present verdict.
Fault/timing angle: The read is a `Promise.all` over two SDK calls raced
against a 2000 ms timer; either call rejecting, or the pair exceeding the
timer, enters the catch at [:97-104][combinedcatch]. A cache rewrite that
returns early on a hit, or that stores a default on miss, changes what the
catch branch sees.
Required faults and enabling state: A prior pass or capture-hook call that
stored `true` for `(todowrite, session)`, then a pass whose `app.agents()` or
`session.get()` rejects or hangs past 2000 ms. `availability.frozen &&
availability.callable` must hold and `compactionOff` must be false, or the
function returns before the read.
Reachability: default-production - `deps.client` is the plugin SDK client
([hook.ts:138-139][hookclient]), so the read runs on every production pass;
SDK timeouts under a slow OpenCode server are plausible and not verified here.
Existing check: [rust-mode-transform.test.ts:466][t466] and
[hook-handlers.test.ts:119][t119] both exercise a hung read with an empty
cache and assert the fail-open outcome (`todo_tool_present: true`, capture
forwarded). Neither constructs a cached deny first. None found for the
deny-then-failure case.
Open questions:
- When no verdict is cached and the read fails, the pass sends
  `todo_tool_present: true` ([:85][combinedseed] `?? false`). Is fail-open
  the intended default, given the comment at [:1057][failclosed] says
  synthesis fails closed when evidence is missing? (needs human input)

### cached-todowrite-verdict-matches-live-evaluation-for-the-pass-inputs

Type: safety
Check: `always` - for every pass that skips the live read and serves a cached
verdict, the served value equals `permissionDisabled("todowrite",
[...agentRules(activeAgentFromMessages(messages)), ...sessionRules])` as
evaluated against the SDK state at the most recent invalidation-free read for
the same `(sessionId, activeAgent)` inputs. `always` because a cached verdict
that diverges from the value the live evaluator would produce for the same
inputs is wrong on every pass, not only when it is observed.
Guarantee: A cache never serves a verdict computed for a different active
agent or an older session permission overlay than the pass's own inputs.
Fault/timing angle: The current cache key is `(tool, session)` only
([permissionCacheKey][permkey]); the active agent is an input to the live
call but not to the key. A user message with a new `info.agent` (read by
[`activeAgentFromMessages`][activeagent]) or an edited session permission
overlay between two passes changes the correct answer without any plugin
event. The plugin has no `permission.*` subscription, so no invalidation
source exists today beyond `session.deleted` and LRU eviction.
Required faults and enabling state: Two consecutive passes for one session
whose last user messages carry different `info.agent` values with different
`todowrite` permissions, or a session permission edit between passes; a
cache design that skips the live read on a hit.
Reachability: default-production - agent switching is an ordinary OpenCode
action (`chat.message` records `input.agent` per message at
[hook-handlers.ts:133-135][agentset]); permission overlay edits mid-session
are not verified as reachable here.
Existing check: [ctx-reduce-availability.test.ts:318][t318] shows the agent
input changes the live answer (`plan` deny vs `undefined` allow) but tests
the evaluator, not a cache. None found for cache staleness across an agent
switch.
Open questions:
- What staleness is acceptable for the permission verdict, in passes or in
  time, and which events must invalidate it? (needs human input)
- Does the OpenCode SDK emit a permission-change event the plugin could
  subscribe to? Unresolved, needs the SDK event list for the pinned OpenCode
  version.

### mid-turn-predicate-is-invariant-under-query-collapse

Type: safety
Check: `always` - for every database state, the collapsed `isMidTurn`
returns the same boolean as this reference predicate: let A be the latest
message by `(time_created DESC, id DESC)` with `role = 'assistant'` that is
not both `summary = 1` and `finish = 'stop'`, using the `json_valid` CASE
guard so malformed `data` reads as NULL. Return true if any message with
`role = 'user'`, `(time_created, id)` greater than A's (or every user row
when A is absent), and no part whose `type = 'compaction'`, is real: it has
no parts, or some part parses as an object, is not machine-authored
(`synthetic`, `syntheticTodoMarker`, `ignored`, or `metadata.marker.kind`),
and is either a non-text typed part or a text part whose cleaned text is
non-empty. Otherwise return false if A is absent; true if A's
`time.completed` is not a number; true if A's `finish = 'tool-calls'`; else
true iff some part of A parses as an object with `type = 'tool'`, is not
provider-executed (`providerExecuted` at top level or under `metadata`), and
is not machine-authored. A missing database returns false; any error on an
existing database returns true. `always` because the predicate must agree on
every state; a differential test against the current function as oracle is
the natural construction.
Guarantee: Query collapse changes cost, never the mid-turn answer, including
the fail-closed answer on a read error.
Fault/timing angle: The predicate reads across two tables without a
transaction, so a writer landing between the assistant query and the
candidate query can make the two reads inconsistent; a single-statement
collapse removes that window and must not introduce a different answer for
the states the current tests enumerate. The `NOT EXISTS ... part p WHERE
p.message_id = m.id` and the per-candidate part query omit `session_id`;
they are correct only while `message.id` is globally unique across sessions
(a PRIMARY KEY in every fixture), so a collapse must not rely on
`(session_id, message_id)` scoping the fixtures never assert.
Required faults and enabling state: A populated OpenCode `message`/`part`
pair with the shapes the tests build (streaming assistant, `tool-calls`
tail, compaction summary after `tool-calls`, same-millisecond rows, malformed
JSON rows and parts, marker-only user parts); a read error on an existing
database (unreadable file, locked WAL) for the fail-closed clause.
Reachability: default-production - `isMidTurn` runs on every pass with an
existing OpenCode database ([:1091][midturncall]); the read-error branch is
reachable in production but not verified as occurring here.
Existing check: [read-session-db.test.ts:97-742][tmidturn] enumerates about
forty example states against `isMidTurnFromOpenCodeDb`; [:920-951][tismidturn]
covers idle, missing DB, and unreadable DB. None found for the cross-session
`message_id` scoping or for a differential oracle.
Open questions:
- OpenCode's real `message` and `part` indexes are not in this repository;
  every fixture declares only `id TEXT PRIMARY KEY`. Whether a collapsed
  query's plan depends on an index that exists in OpenCode's schema is
  unresolved, needs the pinned OpenCode schema.
- Is the missing `session_id` in the part subqueries a correctness or only a
  plan concern? Correctness holds under global `message.id` uniqueness; that
  uniqueness is a property of OpenCode's id scheme, not of this code.
  Unresolved, needs the OpenCode schema.

### cached-statements-execute-only-on-the-live-connection

Type: safety
Check: `always` - every statement executed through `withReadOnlySessionDb`
was prepared on the `Database` instance currently held in `cachedReadOnlyDb`,
and every `get`/`all` observes committed database state as of its own start.
`always` because a statement bound to a closed or replaced connection is
wrong on every execution.
Guarantee: A statement cache keyed on SQL text never outlives the connection
that compiled it and never returns rows from a previous execution.
Fault/timing angle: [`getReadOnlySessionDb`][dbcache] closes and replaces the
connection when `OPENCODE_DB` resolves to a new path; `closeReadOnlySessionDb`
closes it outright. A cache keyed only on SQL survives both unless it is
scoped to the connection. Under `node:sqlite`, `StatementSync` has no
`finalize`; under `bun:sqlite`, `Statement.finalize()` exists and a finalized
statement throws on use. Neither adapter path currently caches, so the
lifecycle is untested.
Required faults and enabling state: A connection replacement or close between
two passes while cached statements exist; concurrent readers on the same
connection are impossible (synchronous API, one connection per process).
Reachability: default-production - the freshness clause runs on every pass;
the replacement clause is explicit-config-only (a mid-process `OPENCODE_DB`
change) or test-only (`closeReadOnlySessionDb` has no production caller).
Existing check: [read-session-db.test.ts:953-1021][tdbpath] covers path
override and replacement without a statement cache;
[sqlite-bind-style.test.ts:32][tbind] pins spread positional binds, which a
cache wrapper must preserve. None found for statement lifecycle across a
connection replacement.
Open questions: None.

### page-byte-count-equals-declared-frame-length

Type: safety
Check: `always` - for every `{ page, bytes }` emitted by
`buildPagedModuleTransformPayloads`, `bytes` equals
`utf8FrameBody(JSON.stringify(page)).byteLength` and equals the byte count
`writeUtf8` emits for that body. `always` because
[`ModuleTransformWirePage.bytes`][pagecontract] is documented as the exact
UTF-8 length and `writeUtf8` throws `RangeError` on any mismatch.
Guarantee: The length the plugin measures for paging and telemetry is the
length the frame writer declares and emits for the same object.
Fault/timing angle: The two measurements are separate `JSON.stringify` calls
on the same object; a serialize-once change must hand the frame channel the
same text it measured. `utf8ByteLength` replaces lone surrogates before
`Buffer.byteLength`; `JSON.stringify` never emits a lone surrogate (it
escapes them as `\udXXX`, verified in Bun 1.3.14 and Node v24.18.0 at
authoring), so both measures agree for any JSON-serialized body. A body
mutated between measure and send, or a page serialized once and then
altered, breaks the equality.
Required faults and enabling state: A body containing a lone surrogate in a
string field (the escape path), a body above 512 KiB (the paged path), and
an ordinary unpaged body.
Reachability: default-production - every pass measures and sends at least
one page.
Existing check: [module-wire.test.ts:1308][t1308] (unpaged length equals a
later stringify), [:1371][t1371] (each page's `bytes` equals a later
stringify), [frame-channel.test.ts:181][t181] and [:193][t193] (declared
length equals written length for lone surrogates). None found that runs a
body containing a lone surrogate through paging and the frame writer
together.
Open questions: None.

### paging-decision-is-consistent-with-host-caps

Type: safety
Check: `always` - every page the plugin sends is admitted by the host's byte
caps: an unpaged transform body is at most 512 KiB by the plugin's measure
and therefore under the 1 MiB facade cap and 32 MiB transform cap
([`enforce_request_byte_cap`][hostcap]); every paged page satisfies
`serde_json::to_vec(&request).len() <= TRANSFORM_PAGE_MAX_BYTES`
([constant][hostpage], [check at lib.rs:9324-9330][hostpagecheck]). `always`
because a page rejected for size fails the
whole series with `buffer_overflow`.
Guarantee: The plugin's 512 KiB decision is never less strict than the host's
512 KiB page check.
Fault/timing angle: The plugin measures `JSON.stringify` bytes; the host
measures the `serde_json` re-serialization of the parsed page. Number
formatting differs between the two for some `f64` values (documented for
digests at [module-wire.ts:57-109][numbers]), so a page at the boundary can
measure differently on each side. A serialize-once change that switches the
measured text (for example to a `serdeJsonCompact` rendering) changes which
side is conservative.
Required faults and enabling state: A body whose `JSON.stringify` length is
within a few bytes of 512 KiB and whose numeric fields render longer under
`serde_json`.
Reachability: default-production - paging engages on large sessions;
[rust-mode-transform.test.ts:532][t532] shows the paged path is ordinary.
Whether a real body can cross the boundary is unresolved.
Existing check: [module-wire.test.ts:1391][t1391] pins the pageable field
list against the Rust literal. None found for the byte-measure boundary.
Open questions:
- Can any body pass the plugin's `JSON.stringify` measure at 512 KiB and
  fail the host's `serde_json` measure? Unresolved, needs a boundary
  construction with `f64` fields.

### log-lines-keep-sanitizer-and-file-hardening-guarantees

Type: safety
Check: `always` - every line the logger appends begins with an ISO-8601
timestamp in brackets, contains no code point in `0x00-0x08`, `0x0b-0x1f`,
or `0x7f`, has `\n`, `\r`, and `\t` flattened to spaces, has each field
truncated at `MAX_FIELD_CHARS = 2048` with a trailing ellipsis, and is
written through a descriptor opened with `O_WRONLY|O_APPEND|O_CREAT|
O_NOFOLLOW|O_NONBLOCK` at mode `0600` under a managed `0700` chain owned by
the current uid; a write failure increments `swallowedWriteCount` and never
throws. `always` because the sanitizer and hardening are the only defense
against log forgery and symlink redirection, and a level gate changes which
lines exist, not what a written line may contain.
Guarantee: A level gate removes lines; it never weakens the sanitization,
truncation, permissions, or swallow accounting of the lines that remain.
Fault/timing angle: A gate placed inside `log()` before `buffer.push` keeps
`sessionLog` observable to callers and spies; a gate at call sites removes
the calls that [rust-mode-transform.test.ts:244][t244] observes through
`spyOn(logger, "sessionLog")`. A gate that bypasses `sanitizeField` for
"cheap" levels reintroduces newline injection. `sanitizeField` does not
strip C1 controls (`0x80-0x9f`) or `U+2028`/`U+2029`; that is the current
contract, not a defect claim.
Required faults and enabling state: Untrusted text with embedded newlines and
control characters in a message or data field; a planted symlink at the log
path; a directory owned by another uid in the managed chain.
Reachability: default-production - `NODE_ENV !== "test"` in production, so
every pass logs; the hardening branches run on every flush.
Existing check: [logger.test.ts:358][t358] (control characters and size
bound), [:381][t381] (private modes and planted symlink),
[:342][t342] (swallowed-write counter), [:408][t408] (exit flush);
[rust-mode-transform.test.ts:244][t244] asserts `rust pass:` and
`rust module stages:` lines exist per pass. None found asserting that
secret-bearing text is redacted before write; no such redaction exists on
this path.
Open questions:
- Provider error bodies and model output reach the log unredacted; the CLI
  redacts on export. Is that the intended boundary, or must the plugin
  redact before write? (needs human input)

### todowrite-deny-then-read-failure-is-exercised

Type: reachability
Check: `sometimes` - across a campaign, at least one pass must satisfy both
preconditions independently: `cachedToolPermissionDenied(sessionId,
"todowrite") === true` at entry to `resolveCombinedTodowriteVerdict`, and
the live read for that same pass rejects or times out. `sometimes` because
this is situation coverage for the window the first property guards; the
marker asserts the preconditions, not the outcome.
Guarantee: The fail-closed fallback is reached with a cached deny at least
once, so the first property is not vacuously satisfied.
Fault/timing angle: The two preconditions come from different passes: an
earlier successful read must have stored `true`, and a later read must fail.
Required faults and enabling state: An SDK fake that answers `deny` once and
then rejects or hangs; a session whose map verdict is frozen and callable.
Reachability: test-only - requires an injected SDK fault; production
occurrence is plausible but unobservable without a marker.
Existing check: none found. The two existing hung-read tests start with an
empty cache.
Open questions: None.

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [rust-mode-transform.test.ts:377][t377] | provisional availability sends `tool_present: false` and `todo_tool_present: false` | unaudited |
| [rust-mode-transform.test.ts:440][t440] | agent `deny` rule through the SDK yields `todo_tool_present: false`; `app.agents` called once | unaudited |
| [rust-mode-transform.test.ts:466][t466] | hung `app.agents()` with an empty cache yields `todo_tool_present: true` after about 2 s | unaudited |
| [hook-handlers.test.ts:119][t119] | hung permission read in the capture hook forwards the snapshot after about 2 s | unaudited |
| [ctx-reduce-availability.test.ts:238-343][tperm] | `permissionDisabled` last-match semantics, session overlay after agent rules, wildcard escaping, agent input changes the answer | unaudited |
| [ctx-reduce-availability.test.ts:26-122][tavaildb] | DB-derived frozen verdicts: fail-open freeze, tie by id, malformed JSON row | unaudited |
| [read-session-db.test.ts:97-742][tmidturn] | example states for `isMidTurnFromOpenCodeDb` (streaming, `tool-calls`, compaction summary, same-millisecond, malformed rows and parts, marker-only parts) | unaudited |
| [read-session-db.test.ts:920-951][tismidturn] | `isMidTurn`: idle DB, missing DB, unreadable DB returns mid-turn | unaudited |
| [read-session-db.test.ts:953-1021][tdbpath] | `OPENCODE_DB` override selects the database; empty override ignored | unaudited |
| [read-session-raw.test.ts:173, 249][tordinal] | ordinal keyset page reads wider than one part chunk; summary rows spend no ordinal | unaudited |
| [sqlite.test.ts:279-467][tsqlite] | adapter constructor option mapping, transaction shim, runtime selector errors | unaudited |
| [sqlite-bind-style.test.ts:32][tbind] | every `.run/.get/.all` uses spread positional binds | unaudited |
| [module-wire.test.ts:1308][t1308] | unpaged `bytes` equals a later `JSON.stringify` length | unaudited |
| [module-wire.test.ts:1371][t1371] | each paged `bytes` equals a later `JSON.stringify` length | unaudited |
| [module-wire.test.ts:1391][t1391] | pageable array field list matches the daemon's Rust literal | unaudited |
| [rust-mode-transform.test.ts:532, 570, 609][tpaged] | paged series re-pages after `need_full_sync`; restarts on attempt mismatch and reconnect | unaudited |
| [frame-channel.test.ts:181, 193][t181] | declared byte length equals written bytes for lone surrogates, including across a segment boundary | unaudited |
| [rust-mode-transform.test.ts:1441, 1577][tinplace] | in-place mutation of an older message forces a full send; recovery after repeated rejection | unaudited |
| [rust-mode-transform.test.ts:244][t244] | `rust pass:` and `rust module stages:` lines are emitted per pass (spy on `sessionLog`) | unaudited |
| [logger.test.ts:358][t358] | control characters removed; entry size bounded; no forged fourth line | unaudited |
| [logger.test.ts:381][t381] | default log is `0600` under `0700` dirs; planted symlink is not followed | unaudited |
| [logger.test.ts:342, 408][t342] | swallowed-write counter; exit flush without holding the process | unaudited |
| [event-handler.test.ts:272-524][tevent] | `message.updated` usage bookkeeping (no assertions on log lines) | unaudited |

None found:

- No test constructs a cached `todowrite` deny and then a failing read.
- No test covers `isMidTurnFromOpenCodeDb` with rows from a second session
  sharing the database.
- No test covers a statement cache across connection replacement (no cache
  exists).
- No test runs a lone-surrogate body through paging and the frame writer in
  one path.
- No test constructs a body at the 512 KiB boundary under both measures.
- No test asserts secret redaction of log lines on this path.
- No test asserts log line counts per pass or per `message.updated` event.

Suspiciously quiet: `transform-stage-logger.ts` has no test file;
`event-handler.test.ts` never asserts on the two `sessionLog` lines per
`message.updated` event.

## Contract-versus-code disagreements

1. Live read frequency. [ctx-reduce-availability.ts:17][doc17] says
   "Todowrite checks live permissions only at cache-busting boundaries" and
   [:57-58][doc57] says defer passes "reuse the cached permission verdict
   without a live permission read". The code at [:85-96][combinedseed]
   issues the live read on every pass whenever `deps.client` is set and uses
   the cache only as the fallback on failure. The host applies the
   bust-pass discipline ([injection.rs:202][injection]); the plugin does not
   know which pass is a bust pass.
2. Fail-closed claim. [rust-mode-transform.ts:1057][failclosed] says
   "Synthesis fails closed when host evidence is provisional or missing".
   For the permission read, a failure with no cached verdict defaults to
   `false` (not denied) at [:85][combinedseed], so the pass sends
   `todo_tool_present: true`. [t466] and [t119] pin this outcome under names
   that say "cached verdict" while constructing no cached verdict.
3. Lone surrogate byte count. [frame-channel.ts:186-190][utf8len] says
   `Buffer.byteLength` "may count a lone surrogate as two bytes". Bun 1.3.14
   and Node v24.18.0 both return 3 at authoring. The replacement is harmless
   and the guarantee stands; the stated rationale is not reproduced.
4. Page cap measure. [module-wire.ts:9][pagemax] says the facade accepts
   pages up to 512 KiB; the host enforces 512 KiB on the `serde_json`
   re-serialization ([lib.rs:9324-9330][hostpagecheck]) while the plugin measures
   `JSON.stringify` output. Both say 512 KiB; the measured text differs.
5. Audit framing (not a source contract). The audit describes "synchronous
   file flushes" per line and a regex scan of the body. The logger batches
   50 lines or 500 ms and flushes synchronously per batch
   ([logger.ts:136-162][flush]); no regex scan of the serialized body was
   found before send. The per-turn line count of nine was not recounted.

Anchor corrections against HEAD: `resolveCombinedTodowriteVerdict` is
77-107, not 77-108. `buildTransformBody` is defined at 739-819; 1340 is a
call site. `prefixContentSnapshotsMatch` is defined at 349-361; 1142 is a
call site. `messageCacheSignature` is defined at 363-368; 1152, 1264, and
1306 are call sites. `resolveToolPermissionDenied` is 277-308, not 280-310.
`buildPagedModuleTransformPayloads` is 635-640 for the unpaged path.
`encodeBody` is 1515-1520. `utf8ByteLength` is 191-193, `utf8FrameBody`
195-203, `writeUtf8` 205-229, `LONE_SURROGATE` 184. All other anchors match.

## Anchors

[run]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L821-L835
[avail]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1053-L1066
[combined]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L77-L107
[combinedseed]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L85-L96
[combinedcatch]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L97-L104
[activeagent]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L68-L75
[failclosed]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1056-L1057
[midturncall]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1091
[body]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L739-L819
[prefix]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L349-L361
[signature]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L363-L368
[cached]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L319-L324
[clearperm]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L326-L334
[permdenied]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L277-L308
[permdisabled]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L203-L213
[permmap]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L59
[permkey]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L71-L73
[doc17]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L15-L17
[doc57]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L57-L58
[capture]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L270-L292
[agentset]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.ts#L133-L135
[hookclient]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L138-L139
[hookclear]: ../../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L375
[timeout]: ../../../../../packages/opencode-plugin/src/shared/with-timeout.ts#L2
[ismidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L73-L82
[dbcache]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L37-L67
[midturndb]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L84-L138
[newer]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L152-L189
[sqlite]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L71-L79
[stmttype]: ../../../../../packages/opencode-plugin/src/shared/sqlite.ts#L304-L309
[paged]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[pagemax]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L9-L10
[pagecontract]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L629-L633
[numbers]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L57-L109
[bodyvalid]: ../../../../../packages/opencode-plugin/src/hooks/context/module-transport.ts#L494-L501
[transportcall]: ../../../../../packages/opencode-plugin/src/hooks/context/module-transport.ts#L824-L836
[request]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/client.ts#L534-L549
[encodebody]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/shared/host-client/client.ts#L1515-L1520
[utf8len]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L184-L193
[utf8body]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.ts#L195-L229
[sessionlog]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L183-L200
[sanitize]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L14-L34
[ensuredir]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L98-L109
[appendpriv]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L117-L134
[flush]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L136-L162
[stagelog]: ../../../../../packages/opencode-plugin/src/hooks/context/transform-stage-logger.ts#L3-L12
[evlog]: ../../../../../packages/opencode-plugin/src/hooks/context/event-handler.ts#L121-L240
[redaction]: ../../../../../packages/opencode-plugin/src/shared/redaction.ts#L1-L20
[injection]: ../../../../../crates/daemon/src/injection.rs#L195-L230
[hostcap]: ../../../../../crates/daemon/src/lib.rs#L15364-L15538
[hostpage]: ../../../../../crates/daemon/src/lib.rs#L742-L743
[hostpagecheck]: ../../../../../crates/daemon/src/lib.rs#L9370-L9376
[t377]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L377
[t440]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L440
[t466]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L466
[t532]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L532
[tpaged]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L532
[t244]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L244
[tinplace]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1441
[t119]: ../../../../../packages/opencode-plugin/src/hooks/context/hook-handlers.test.ts#L119
[t318]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L318
[tperm]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L238
[tavaildb]: ../../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.test.ts#L26
[tmidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L97
[tismidturn]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L920
[tdbpath]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-db.test.ts#L953
[tordinal]: ../../../../../packages/opencode-plugin/src/hooks/context/read-session-raw.test.ts#L173
[tsqlite]: ../../../../../packages/opencode-plugin/src/shared/sqlite.test.ts#L279
[tbind]: ../../../../../packages/opencode-plugin/src/shared/sqlite-bind-style.test.ts#L32
[t1308]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1308
[t1371]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1371
[t1391]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.test.ts#L1391
[t181]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L181
[t193]: ../../../../../packages/opencode-plugin/src/shared/host-client/frame-channel.test.ts#L193
[t358]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L358
[t381]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L381
[t342]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L342
[t408]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L408
[tevent]: ../../../../../packages/opencode-plugin/src/hooks/context/event-handler.test.ts#L272
