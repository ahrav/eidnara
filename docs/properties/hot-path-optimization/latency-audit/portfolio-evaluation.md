# Portfolio evaluation

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, evaluated 2026-09-10.

Independence: this evaluation was written without reading `_lenses/`. Its
inputs are `docs/properties/METHOD.md`, `docs/properties/README.md`, the
portfolio-evaluation mechanics in the property-discovery skill, the 36
records in `catalog.md`, `existing-checks.md`, `fault-map.md`, the 36
evidence files, the parent catalog's record headers and queued gaps, and the
source tree at HEAD. Every `file:line` reference below was read at HEAD
before it was written. No test, bench, or campaign ran. The five leads the
evidence writers left were treated as claims to verify; all five hold, and
each is recorded with its own citations in the disposition table.

The portfolio is a sound preservation contract in shape: 27 `always` and 9
`sometimes` records, every `sometimes` paired with a safety record it makes
non-vacuous, no `sometimes(X)` beside an `always(!X)`, no unbounded
"eventually", and no liveness record. The flaws found are of four kinds: a
handful of check clauses whose wording does not match HEAD; five records
whose reference is the HEAD code itself with no frozen artifact; three
witnesses whose precondition is the candidate's own mechanism or read; and
one risk class with no record, panic redaction across the thread boundary a
`spawn_blocking` relocation crosses, which the daemon already crosses today
for kernel routes.

## Harness fit

Semantics. Every `always` record asserts a condition evaluated on each pass
or each call, and every `sometimes` record asserts preconditions rather than
an outcome. The distribution stated at `catalog.md:109-112` is correct. Two
explicit-config-only records, C3 and W6, use `always` with no occurrence
witness; under a default campaign their populated paths never run, so the
checks pass vacuously. METHOD reserves `always-or-unreached` for exactly that
shape, or the campaign can add a `sometimes` witness. Either is a decision
about the campaign contract, so it is queued as a gap and surfaced as a bias
rather than applied.

Literal assertability. Most checks are assertable as written. Exceptions:

- W2's clause "after a relocation each field's start and stop instants still
  bracket the same work" is a review criterion, not a runtime assertion. Its
  first clause, "every field named by `format_pass_timing_line` ... exists in
  `TransformTimings`", is assertable but false at HEAD as a literal name
  equality: the line prints `post_attach_ms`
  (`crates/daemon/src/transform.rs:1254`, value at `:1331`) for the field
  `post_attach` (`:1162`). The plugin reads
  `post_attach` by field name (`rust-mode-transform.ts:1035`), so the two
  consumers already disagree on one key. The check needs a stated
  key-to-field map.
- A1's clause "at most `RETAINED_STRING_COPIES` owned copies of each string
  block retained by the typed decode" names no observation method. The
  decoded `TransformRequest` exposes no copy count; the oracle is structural
  (the `Value` node plus the two `original` retentions) or an allocation
  counter. The record should say which.
- W1 quantifies "for every stage the specification authorizes to change"
  under one constant marker. One `sometimes` marker fires when any stage is
  measured and cannot certify every stage. It needs one constant marker per
  stage, fixed when the specification enumerates them, as W10 already does
  for its four conditions. G3 has the same shape: five decrement paths, one
  slug.

Witness independence. Three `sometimes` records certify their setup from the
candidate itself:

- B5 asserts that `normalize_synthetic_todo_ingress` "produces a clone"
  (`transform.rs:2083-2100`). A shared-view design, the very change B2
  constrains, produces no clone, so the marker can never fire under the
  candidate. The independent precondition is the input: a non-synthetic
  message carrying a `synthetic_todo_` id.
- C5 asserts that "the `row_version` observed by its post-commit read
  differs from the `row_version` its transform committed". The post-commit
  read is the thing C1 tests; a single-load candidate returns the pre-commit
  snapshot and the marker fires for the wrong reason. The independent
  witness is the foreign actor's commit through a second store handle, its
  returned `row_version`, and its ordering before the pass's first
  post-commit read.
- G3 says "observes usage" without naming the source. Once a counter ships,
  "usage" is the candidate. The oracle must be the independent `st_size`
  walk (`crates/kernel/src/cas/ingest.rs:1144-1204`).

Reference without a frozen artifact. Five `always` records compare a
candidate with the HEAD implementation and name no artifact that survives
the change: A2 ("the current reader"), W6 ("`next_occurrence` at HEAD"), W9
("the reference predicate at `:4298-4316`"), W4 ("the whole-input scan"),
and P2 (a prose reference with no file). Once the optimization replaces the
code, each oracle disappears. W5 shows the working pattern: a frozen copy in
`crates/daemon/tests/historian_truncate_differential.rs:13-58`, with three
proptests against it. The caveman and selection differentials in
`crates/daemon/tests/` follow the same pattern. This is systematic enough to
record as a bias as well as five refinements.

Two fault angles are wrong at HEAD:

- T4 says fast egress lets "the endpoint thread serialize before the handler
  future is polled to completion". For a unary response the frame is queued
  from `settle` after the joined future returns
  (`crates/host-runtime/src/dispatch.rs:956-997`, the `settle` call at `:995`,
  the emit at `:409-420`), so the closure always outlives the handler once
  queued. The stated angle describes a stream item sent through
  `StreamSink::send` (`:556-582`) while the handler is still running. The
  construction problem for the unary case is observing a short window, not
  opening it.
- W10's `-updates` marker asserts `m1.memory_update_count > 40`. The field
  has one writer, the constant `0` at `crates/daemon/src/m1_compose.rs:231`,
  and one reader, the predicate at `transform.rs:4310`. The marker cannot
  fire at HEAD and W9's required state "`memory_update_count` at 40 and 41"
  is not constructible through the SOFT arm. The catalog's Exercised and
  Required-faults fields present both as reachable.

One check clause is inexact: B4 says "excluded parts hash `"excluded\0" ++
block.bytes`". That holds for the pre-match exclusions
(`crates/daemon/src/tail_hygiene.rs:513-526`) and the kind-level exclusions
(`:601-604`). The empty-or-sentinel branches for text, tool result, and
media hash the derived content string instead (`:542-543`, `:569-570`,
`:584-585`, through `excluded_part` at `:280-289`). The evidence file has
the correction; the catalog does not.

One rationale over-claims: W7 says `prepare_historian_fire` "calls it every
pass". The call at `crates/daemon/src/lib.rs:5051` runs only after the state
load succeeds (`:5013-5025`), when no `pending_rewrite` is set (`:5036`),
and when no live historian completion is pending (`:5042`); the handler
skips `prepare_historian_fire` entirely for subagent passes (`:8234`). The
check itself is a conditional on the call and stays `always`; the rationale
needs the qualification.

Routing for `/testing:test-strategy`, by cheapest form that can observe the
invariant:

| Form | Records |
| --- | --- |
| Pure differential against a frozen reference or formula | A2, B4, C4, P2, P3, W4, W5, W6, W9 |
| One process, real store or files, no concurrency | B1, B3, C1, C2, G1, W3, W7 |
| Driven host, SDK fake, or in-crate ring | A1, A3, P1, P4, P5, T1, T2, T3 |
| Interleaving, crash point, or abort seam | C3, C5, G2, G3, T4, W8, W10, W11 |
| Measurement manifest, not a test harness | W1, W2 |

## Coverage balance

The six probes named in the brief, then the balance across groups.

(a) Panic boundary under `spawn_blocking`. Not covered, and reachable at
HEAD. The redaction guard is a thread-local poll-depth counter
(`crates/host-runtime/src/panic_boundary.rs:11-13`); the panic hook prints
the redacted string only when `callback_is_polling()` is true on the
panicking thread (`:30-34`, `:40-47`), and otherwise forwards to the
previous hook with the full payload (`:46`). The host wraps the handler
future in `redact_sync` and `redact` (`dispatch.rs:928-934`), which sets the
depth on the runtime worker that polls it. A `spawn_blocking` worker thread
has depth zero. The daemon already runs kernel work that way:
`kernel_routes::blocking` (`crates/daemon/src/kernel_routes/mod.rs:462-468`)
wraps `tokio::task::spawn_blocking` at ten call sites, including the ingest
finish that calls `store.ingest_artifact` (`kernel_routes/ingest.rs:577`,
inside the closure at `:797`) and the page decode at `:722`; `MemoryStore::open`
runs the same way (`lib.rs:3808`). A panic inside any of those closures
prints unredacted on the worker thread; the `JoinError` is then mapped to
`StoreUnavailable` (`mod.rs:467`), so the request terminal is preserved and
only the diagnostic leaks. The host-runtime record
`every-callback-invocation-is-inside-the-redaction-guard`
(`docs/properties/host-runtime/catalog.md:1318-1345`) enumerates host call
sites and does not reach daemon-spawned threads; the parent E-group covers
charges and lifecycle, not diagnostics. A transform relocation widens this
from kernel routes to every pass. Gap.

The same boundary crosses one more thread-local: the token-cache counters
(`crates/daemon/src/token_cache.rs:57`), which W2 already names. The third
`thread_local!` in the inspected crates is test-only
(`transform.rs:487-490`). A relocation record should carry the inventory,
not one item of it.

(b) `trace_pass_received` folded with `commit_transform`, CAS fails. C2's
oracle, `receive_count` increases by exactly one per request, is the right
one and already covers the exhausted-retry case through the reject clause.
It does not name the conflict-then-retry case: under a fold, a CAS conflict
rolls the breadcrumb back with the row, the retry loop
(`transform.rs:1940-1979`) reruns `apply_once`, and the second commit bumps
once. The Required-faults list should include a first-attempt CAS conflict.
Separately, C2 and the fault map (`fault-map.md:20`) treat "an injected
failure in the `pass_trace` upsert" as available at the store. No such seam
exists: the four `fail_next_*_for_test` seams
(`crates/memory-store/src/lib.rs:5900`, `:5914`, `:5921`, `:5928`) cover the
side channel, the authority route read, and two dreamer task steps.
Refinement to both.

(c) Direct frame for non-transform routes. T3's check is generic over
`DirectFrame` and holds for any route. The reachability text narrows the
future to "transform responses" (`catalog.md:64-66`). The owned path
serializes a `MeasuredSource::Json` response twice at HEAD: `measure_json`
counts (`crates/daemon/src/dispatch.rs:132-148`) and `serde_json::to_writer`
writes (`:237-249`); a direct path for those routes moves the write to the
endpoint thread with an owned `Value` in a `'static` closure
(`host-runtime/src/handler.rs:466-472`), which is exactly T3's charge
clause. No new record; the reachability wording should not exclude the
routes the specification is most likely to move first. Refinement.

(d) Plugin statement cache and a replaced database file. P2 binds every
statement to the `Database` in `cachedReadOnlyDb` and requires `get`/`all`
to see committed state, but nothing binds `cachedReadOnlyDb` to the live
file. The cache is keyed on path alone
(`packages/opencode-plugin/src/hooks/context/read-session-db.ts:53-63`); a
file replaced at the same path (rename over, or delete and recreate) leaves
the open handle on the old inode and every later `isMidTurn` answers from
it. `openCodeDbExists()` is re-checked per call (`:75`), so a deleted file
returns idle while the stale handle stays open for the next replacement. P2
does not cover it. Whether OpenCode ever replaces the file is not in this
repository. Gap, reachability unresolved.

(e) Unbounded "eventually". None. The catalog has no liveness record; W7's
"by the next pass", W8's "on the next pass", G2's "at every quiescent point
after recovery", and C3's backoff restate bounds the code fixes. The
statement at `catalog.md:110-112` is accurate.

(f) `sometimes` without an independent witness. B5, C5, and G3, as analysed
under harness fit. A3, P5, T4, W1, W10, and W11 assert inputs or ordering
observed outside the candidate.

Across the seven groups:

- Ingress (A). Dense on decode and charge order. No record on the paged
  lane's assembled-whole footprint beyond A1's open question; that is a
  design decision, correctly left open.
- Deep-copy elimination (B). Dense. B1 through B5 together state the
  ownership-independence contract from three observers. The sidecar
  `mid_pins` question is an honest unresolved.
- Cache state (C). Dense on freshness and trace. The Emergency95 rerun that
  C1's floor comparison and C2's double-count clause depend on has no
  witness of its own; C5 fires only when a foreign write also lands. The
  parent's queued gap 4 (pass-class witnesses) is the right home; extend it
  with "an Emergency95 pass reran and committed twice".
- Plugin (P). P1 through P5 are the only plugin records in `docs/properties`.
  Coverage of the pre-send path is proportionate; (d) is the one hole.
- Ring and direct frame (T). T1 correctly refuses to invent a residency
  bound. T3 and T4 are sound once T4's angle is corrected. A note only:
  `punch_batch_bytes` is `(arena_bytes / 4).max(system_page_size())`
  (`crates/shm-transport/src/backend/ring.rs:2159-2161`); T1 states
  `arena_bytes / 4`, which is equal at every arena the profile admits.
- CAS accounting (G). Sound. G2 keeps the walk as the oracle, which is the
  right call. G3 needs per-path markers and the independent source named.
- Cross-cutting (W). W1 and W2 gate every "faster" claim and are the most
  valuable records in the area. W8 and W11 treat `spawn_blocking` as a
  future topology; see the wildcard section.

Balance across semantics: no `always-or-unreached`, `reachable`, or
`unreachable`. The absence of `reachable` and `unreachable` is fine for a
preservation contract. The absence of `always-or-unreached` for C3 and W6 is
the vacuity issue above.

## Implementability

Anchor verification against HEAD. Every anchor below was read; none is off.

| Anchor | Status | Note |
| --- | --- | --- |
| `crates/host-runtime/src/dispatch.rs:956-997` | verified | joined arm; `settle` called at `:995` after the handler future returns |
| `crates/host-runtime/src/dispatch.rs:409-420` | verified | `emit_reserved_frame` inside `settle` for a unary `Response` |
| `crates/host-runtime/src/dispatch.rs:556-582` | verified | `StreamSink::send` emits while the handler runs; the only pre-completion path |
| `crates/host-runtime/src/dispatch.rs:928-934` | verified | handler future wrapped in `redact_sync` and `redact` on the runtime worker |
| `crates/host-runtime/src/dispatch.rs:517-554` | verified | `reserve_direct` charges `exact_len + HEADER_LEN` and awaits the egress budget |
| `crates/daemon/src/m1_compose.rs:231` | verified | `memory_update_count: 0`; field declared at `:96`; sole reader `transform.rs:4310` |
| `crates/daemon/src/tail_hygiene.rs:542-543`, `:569-570`, `:584-585` | verified | empty or sentinel content excluded with derived content, not `block.bytes` |
| `crates/daemon/src/tail_hygiene.rs:513-526`, `:601-604`, `:280-289` | verified | pre-match and kind-level exclusions hash `block.bytes`; `excluded_part` helper |
| `crates/daemon/src/lib.rs:5051` | verified | `effective_config` after early returns at `:5013-5025`, `:5036`, `:5042` |
| `crates/daemon/src/lib.rs:8234` | verified | `is_subagent` skips `prepare_historian_fire` |
| `crates/daemon/src/lib.rs:4556-4565` | verified | `#[cfg(test)] fixed_config` short-circuit at `:4557-4560` |
| `crates/daemon/src/transform.rs:1254`, `:1331`, `:1162` | verified | key `post_attach_ms`, value `timings.post_attach`, field `post_attach` |
| `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1013-1042` | verified | 22 stage keys by field name; `post_attach` at `:1035` |
| `crates/daemon/src/lib.rs:11805-11827` | verified | cap, footprint, `try_reserve_resident`, `Value` parse, dispatch |
| `crates/daemon/src/lib.rs:15458-15470` | verified | `invalid_params` and `queue_full` helpers |
| `crates/daemon/src/lib.rs:8036-8048`, `:8066` | verified | prompt freeze, route-channel insert, `ticket.accept()` |
| `crates/host-runtime/src/handler.rs:474-491` | verified | `try_reserve_resident` is `scratch.try_charge`; capacity doc |
| `crates/host-runtime/src/handler.rs:465-472`, `:387-400` | verified | `output_from_writer` needs `Send + 'static`; Direct arm passes the charge unshrunk |
| `crates/host-runtime/tests/support/mod.rs:441-455` | verified | `direct_fill` arm; no other `direct_fill` string in the tree |
| `crates/daemon/src/transform.rs:4298-4316` | verified | two direct `tokenizer::estimate_tokens` calls and the three disjuncts |
| `crates/daemon/src/transform.rs:7160`, `:8558` | verified | direct tokenizer calls for mint and nudge `token_count` |
| `crates/daemon/src/transform.rs:7883-7884`, `:2083-2100`, `:2951` | verified | `Arc::make_mut`, the clone-producing normalizer, the shadow |
| `crates/shm-transport/src/backend/ring.rs:2129-2134`, `:2158-2161`, `:48` | verified | punch decision; batch is `(arena_bytes / 4).max(page)`; divisor 4 |
| `crates/shm-transport/src/lease.rs:328-348` | verified | zero-fill at `:331`; both `LengthMismatch` arms |
| `crates/host-runtime/src/ring_transport.rs:57-58`, `:749-786`, `:788-800` | verified | constant comment says virtual bytes; `publish_one` and `publish_direct` |
| `crates/host-runtime/src/frame_channel.rs:166-200`, `:248-275` | verified | `DirectFrame` owns a boxed `'static` serializer; `send_before` |
| `crates/kernel/src/cas/ingest.rs:333-342`, `:591-608` | verified | lock then `check_budget`; walk, presence, projected cap |
| `crates/daemon/src/token_cache.rs:135-137`, `:57` | verified | `u32` bypass; thread-local counters |
| `crates/daemon/src/lib.rs:2243-2257` | verified | the declared-bytes sum, 14 terms |
| `crates/daemon/src/config.rs:266-288` | verified | doc on repeated warnings; clone at `:288` |
| `crates/memory-store/src/lib.rs:6482-6496` | verified | `trace_pass_received` and its "never contends" doc |
| `crates/memory-store/src/lib.rs:5900`, `:5914`, `:5921`, `:5928` | verified | the only `fail_next_*_for_test` seams; none for `pass_trace` |
| `crates/daemon/src/smart_note_evaluation.rs:163-192` | verified | doc at `:163-165`, `fn next_occurrence` at `:166` |
| `crates/daemon/src/historian_chunk.rs:692`, `:742-748` | verified | truncation call and function head |
| `crates/daemon/src/lib.rs:8131`, `:8194-8201`, `:8202`, `:8209-8214`, `:8224-8232` | verified | trace calls, commit call, roots insert, `#[cfg(test)]` hook |
| `crates/daemon/src/lib.rs:8263`, `:8289`, `:8315`, `:8387-8394`, `:8398-8403` | verified | the three Emergency95 awaits; projection-cache store; guidance removal |
| `crates/daemon/src/lib.rs:2906-2911` | verified | hook field is `#[cfg(test)]`, not `test-support` |
| `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:77-107` | verified | seed at `:85`, catch at `:97-104` |
| `packages/opencode-plugin/src/hooks/context/read-session-db.ts:37-67`, `:73-82` | verified | path-keyed connection cache; `isMidTurn` |
| `packages/opencode-plugin/src/shared/with-timeout.ts:2`; `module-wire.ts:635-640`; `Cargo.toml:45` | verified | 2000 ms; paging measure; `raw_value` only |
| `docs/host-wire-protocol.md:308`, `:314`, `:440`, `:666` | verified | §6.3 cap codes; writer rule; §7.5.1 strict parse; §7.7 |
| `crates/daemon/tests/direct_host.rs:48-70`, `:285-290` | verified | real-host fixture drives a unary transform; `request_too_large` is the control channel |
| `crates/daemon/src/transform.rs:12378-12383`, `:27395-27400`, `:27269-27270`, `:24170-24172` | verified | ignored fixtures; replayed-pair test; one-helper source scan |
| `crates/host-runtime/src/ring_transport.rs:1857-1858`; `hot_path.rs:197-200`; `ci.yml:506-510` | verified | deadline test; e2e bench; bench smoke step |
| `crates/kernel/tests/cas_fault_injection.rs:34`; `kernel/src/cas/mod.rs:36`; `gc.rs:33` | verified | crash point and both fault enums exist |

New anchors this evaluation adds, all read at HEAD:
`crates/host-runtime/src/panic_boundary.rs:11-13`, `:30-34`, `:36-50`,
`:52-55`, `:60-72`; `crates/daemon/src/kernel_routes/mod.rs:462-468`;
`crates/daemon/src/kernel_routes/ingest.rs:513-517`, `:716-722`, `:577`,
`:797`; `crates/daemon/src/lib.rs:3808`, `:3859`;
`crates/host-runtime/src/routing.rs:591`;
`crates/daemon/src/dispatch.rs:132-148`,
`:237-249`; `crates/daemon/src/lib.rs:12014-12070`;
`packages/opencode-plugin/src/hooks/context/read-session-db.test.ts:8`, `:22`.

Constructibility per group, against the test infrastructure found:

- A1 and A3. The catalog and fault map say only "a real host or a new seam"
  drives `Handler::handle`. A real host fixture exists:
  `crates/daemon/tests/support/direct_host.rs` (`FixtureProcess`), used by
  `direct_host.rs:48-70` to send a unary transform over the ring. Oversize
  bodies fit the 64 MiB arena; a footprint above 176 MiB needs about 2.9
  million scalar nodes, which is a body of a few megabytes under the 32 MiB
  cap with a transform-class `kind`. A1 is constructible today. A3's barrier
  needs a way to hold one request inside dispatch; the fixture handler has
  no such arm, so A3 needs one.
- B1 through B5. Constructible in `crates/daemon/src` unit tests; the
  differential gates are live under `cfg!(test)`. B5's delta turn needs a
  `model_chain` and the `OpencodeAiSdk` profile, both fixture inputs.
- C1, C2, C4. Constructible with a real store. C2's trace-failure fault has
  no seam (above). C5 and W11 depend on `between_transform_and_prepare`,
  which is `#[cfg(test)]` (`lib.rs:2910-2911`) and not exported under
  `test-support`, so both markers must live in the `crates/daemon` unit-test
  crate, as the evidence states. C3's crash between mark and delete needs a
  child process; the kernel's `cas_fault_injection.rs` crash-barrier pattern
  is the in-repo precedent.
- P1 through P5. SDK fakes exist in `rust-mode-transform.test.ts` (`:440-441`
  installs a deny through `app.agents`; `:466-467` hangs it). P5 is a
  two-run sequence on one session id; constructible. P2's connection
  replacement is drivable because `closeReadOnlySessionDb` is exported and
  already called in test setup (`read-session-db.test.ts:8`, `:22`); file
  replacement at the same path is not exercised anywhere.
- T1 and T2. In-crate; `resident_arena_pages` (`ring.rs:1894-1899`) is the
  probe. T3 and T4 are constructible in `crates/host-runtime/tests` through
  the `direct_fill` arm, which no test sends. In the daemon they wait for a
  sender.
- G1 through G3. `ArtifactIngestFault`, `ArtifactGcFault`, and
  `INGEST_CRASH_POINT` exist; G3 needs only the independent walk read before
  and after, plus five marker names.
- W7. `Handler::effective_config` returns `fixed_config` when set
  (`lib.rs:4557-4560`; set at `:3859`). Unit tests built that way bypass the
  cache entirely and cannot exercise W7. The record should say so.
- W9 and W10. The `-updates` arm is not constructible through the SOFT arm at
  HEAD; the other three are workload inputs.
- W1 and W2. A manifest form and a size class; no harness.

## Wildcard

The catalog frames execution topology as unresolved and `spawn_blocking` as a
future relocation (`catalog.md:62-64`; W8 and W11). The daemon already runs
that topology for kernel routes: `kernel_routes::blocking`
(`crates/daemon/src/kernel_routes/mod.rs:462-468`) at ten call sites, with a
stated ownership convention. The ingest page path moves its staging
reservation into the worker's result so the charge is released when the work
ends, not when the handler future is dropped, because "a dropped
`spawn_blocking` handle does not stop its worker"
(`kernel_routes/ingest.rs:513-517`, `:716-722`). That is a concrete answer to
the parent's E1 question and to W8's "what owns and joins any proposed
off-worker transform work". The area does not cite it. Two consequences:

1. The panic-redaction gap in (a) is not a relocation-era risk; it is a
   default-production condition at HEAD for every kernel route, including
   the ingest G1 and G2 constrain. The "execution topology unresolved"
   framing hid a live boundary.
2. A transform relocation shares Tokio's blocking pool with those ten
   sites, with `MemoryStore::open` (`lib.rs:3808`), and with the routing
   bound installer (`host-runtime/src/routing.rs:591`). No record bounds
   pool occupancy or names the tenants. W1 would measure the effect; nothing
   states the contract.

A second framing question: every `always` record preserves values and
bytes. None preserves what the process says about itself when something
goes wrong. Panic-hook output, stderr diagnostics, and the timing line's
meaning under relocation (W2) are the only self-reports on this path. W2
covers one; the redaction gap is another; a relocation inventory of
thread-local state would cover the class.

## Finding disposition

| # | Finding | Class | Records | Proposed action |
| --- | --- | --- | --- | --- |
| 1 | T4's fault angle describes the stream case; for a unary response the frame is queued from `settle` after the handler future completes (`dispatch.rs:956-997`, `:409-420`) | refinement | T4 | Rewrite Fault/timing angle and Required faults; see below |
| 2 | W10's `-updates` marker cannot fire and W9's update-count boundary is not constructible; `memory_update_count` is the constant `0` (`m1_compose.rs:231`) with no other writer | refinement | W10, W9 | Rewrite Exercised and Required faults; add the writer question as needs human input |
| 3 | B4's "excluded parts hash `block.bytes`" holds for pre-match and kind-level exclusions only; empty and sentinel branches hash derived content (`tail_hygiene.rs:542-543`, `:569-570`, `:584-585`) | refinement | B4 | Rewrite the excluded clause of Check |
| 4 | W7's rationale says `prepare_historian_fire` calls `effective_config` every pass; the call at `lib.rs:5051` is skipped on subagent (`:8234`), load-failure (`:5013-5025`), pending-rewrite (`:5036`), and busy (`:5042`) passes | refinement | W7 | Rewrite the rationale sentence of Check |
| 5 | W2's first clause is false as a literal name equality at HEAD: `post_attach_ms` (`transform.rs:1254`, `:1331`) versus field `post_attach` (`:1162`) | refinement | W2 | State the key-to-field map in Check |
| 6 | W2's "bracket the same work" clause is a review criterion with no runtime oracle | refinement | W2 | Name the observation method in Check |
| 7 | Five records compare against HEAD code with no frozen artifact: A2, W6, W9, W4, P2 | refinement | A2, W6, W9, W4, P2 | Name a frozen reference per record, on the `historian_truncate_differential.rs` pattern |
| 8 | B5's precondition "produces a clone" is the candidate's mechanism; a shared-view design never fires it | refinement | B5 | Assert the input condition only |
| 9 | C5's witness is the pass's own post-commit read, which is what C1 tests | refinement | C5 | Witness the foreign commit through a second handle and its ordering |
| 10 | G3 does not name the usage source and uses one marker for five paths | refinement | G3 | Name the independent walk and five constant markers |
| 11 | W1 quantifies over every stage under one marker | refinement | W1 | One constant marker per authorized stage |
| 12 | C3 and W6 are explicit-config-only `always` records with no occurrence witness; vacuous under a default campaign | gap | C3, W6 | Two `sometimes` witnesses, or `always-or-unreached` with a recorded exemption; the choice is a campaign-contract decision |
| 13 | Panic redaction is thread-local (`panic_boundary.rs:11-13`, `:40-47`); `kernel_routes::blocking` (`mod.rs:462-468`) already runs kernel work, including `ingest_artifact` (`ingest.rs:577`, `:797`), on worker threads with no guard; a transform relocation widens it to every pass | gap | new; touches G1, G2, W8, W11 | One safety record and one witness; see gaps queued |
| 14 | C2's Required faults lack a first-attempt CAS conflict under a fold, and both C2 and `fault-map.md:20` assume a `pass_trace` failure seam that does not exist (`memory-store/src/lib.rs:5900-5928`) | refinement | C2, fault-map | Add the conflict case; state the missing seam |
| 15 | T3 and T4 reachability text narrows the future to transform responses; T3's check is generic and the `MeasuredSource::Json` owned path serializes twice today (`daemon/src/dispatch.rs:132-148`, `:237-249`) | refinement | reachability text | Widen the wording |
| 16 | P2 does not cover a replaced database file at the same path; the connection cache is path-keyed (`read-session-db.ts:53-63`) | gap | P2 | Discovery on whether `isMidTurn` must observe replacement; reachability needs OpenCode's write behavior |
| 17 | A relocation crosses two production thread-locals (`panic_boundary.rs:11`, `token_cache.rs:57`); W2 names one | gap | W2, W8 | Fold into the record for finding 13 as an inventory clause |
| 18 | No record bounds blocking-pool tenancy shared by a relocated transform, `kernel_routes::blocking`, `MemoryStore::open`, and `routing.rs:591` | gap | new | Bounded record with the pool's tenants named |
| 19 | A1 says only `dispatch_value` is reachable; the real-host fixture (`tests/support/direct_host.rs`, used at `direct_host.rs:48-70`) reaches `Handler::handle` | refinement | A1 | Name the fixture in Exercised |
| 20 | W7 cannot be exercised by unit tests built with `fixed_config` (`lib.rs:4557-4560`, set at `:3859`) | refinement | W7 | Add the constraint to Existing check |
| 21 | A1's copy-count clause names no observation method | refinement | A1 | State the structural or allocator oracle in Check |
| 22 | Reference-is-HEAD-code orientation across five records while W5 and two other differentials use frozen copies | bias | A2, W6, W9, W4, P2 | Human decides frozen copy versus recorded corpus per record, and who owns the frozen files |
| 23 | Topology framed as hypothetical while `kernel_routes::blocking` is a live precedent with an ownership convention | bias | W8, W11, parent E1, E3 | Human decides whether the transform adopts the precedent |
| 24 | Preservation measured on values and bytes only; diagnostics, thread identity, and pool tenancy are not treated as preserved surfaces | bias | area-wide | Human decides whether the latency specification owns them |

Counts: 16 refinements, 5 gaps, 3 biases, 24 findings.

## Refinements to apply

Each entry gives the record, the field, the exact current text as it appears
in `catalog.md` (wrapped as in the file), and the replacement. Link labels
used in replacements already exist in the catalog's link table; new line
references are written as inline code so the table is not touched.

### T4, Fault/timing angle

Current:

```
Fault/timing angle: With fast egress the endpoint thread serializes before
the handler future is polled to completion, so the closure never outlives the
handler and T3's charge clause is vacuous.
```

Replacement:

```
Fault/timing angle: For a unary response the frame is queued from `settle`
after the handler future has completed (`dispatch.rs:956-997` joins the
future and calls `settle` at `:995`; `:409-420` emits the frame), so the
closure outlives the handler whenever the frame is queued. With an idle ring
and a free endpoint thread the window between queueing and commit is short
and a marker can miss it. The endpoint thread can serialize before the
handler returns only for a stream item sent through `StreamSink::send`
(`:556-582`); the transform response is unary.
```

### T4, Required faults and enabling state

Current:

```
Required faults and enabling state: A slow egress or a held ring reservation
ahead of the direct frame; a handler that returns immediately after
[`output_from_writer`][from-writer]; observation of handler completion and of
commit as separate events.
```

Replacement:

```
Required faults and enabling state: A slow egress or a held ring reservation
ahead of the direct frame, to lengthen the window so the marker can observe
it; a handler that returns immediately after
[`output_from_writer`][from-writer]; observation of handler completion (the
joined future) and of the `publish_one` commit as separate events.
```

### W10, Exercised

Current:

```
Exercised: not yet - No witness records any threshold crossing.
```

Replacement:

```
Exercised: not yet - No witness records any threshold crossing; the
`-updates` marker cannot fire at HEAD because `memory_update_count` is the
constant `0` (`crates/daemon/src/m1_compose.rs:231`) and has no other writer.
```

### W10, Required faults and enabling state

Current:

```
Required faults and enabling state: A session with more than 40 memory
updates since the last fold; an m1 body whose exact token count exceeds a
fifth of a small positive budget; a frozen m0 of at least 500 tokens with an
m1 above 15 percent of it; direct token counts recorded before the candidate
runs.
```

Replacement:

```
Required faults and enabling state: A writer for `memory_update_count` that
the specification introduces (none exists at HEAD), then a session with more
than 40 memory updates since the last fold; an m1 body whose exact token
count exceeds a fifth of a small positive budget; a frozen m0 of at least 500
tokens with an m1 above 15 percent of it; direct token counts recorded before
the candidate runs.
```

### W10, Open questions

Current (the `Impact` line is included so the match is unique; keep it):

```
Impact: W9 passes without any boundary being approached.
Open questions: None.
```

Replacement:

```
Impact: W9 passes without any boundary being approached.
Open questions:
- Does the specification give `memory_update_count` a writer, so the
  `-updates` marker can fire, or is the first disjunct dropped from W9's
  reference and from this record? (needs human input)
```

### W9, Required faults and enabling state

Current:

```
Required faults and enabling state: SOFT passes with `memory_update_count` at
40 and 41; `m1_tokens` at the 0.20 budget boundary; `m0_tokens` at 499 and
500 with `m1_tokens` at the 0.15 boundary; an `m1.body` equal to the
placeholder; a store with no `m0` frozen unit.
```

Replacement:

```
Required faults and enabling state: SOFT passes with `memory_update_count` at
40 and 41 once a writer exists (at HEAD the field is the constant `0`, so this
arm is reachable only through a directly constructed `M1Composition`);
`m1_tokens` at the 0.20 budget boundary; `m0_tokens` at 499 and 500 with
`m1_tokens` at the 0.15 boundary; an `m1.body` equal to the placeholder; a
store with no `m0` frozen unit.
```

### W9, Check (reference clause)

Current:

```
Check: `always` - For every SOFT pass, `pressure_refold` equals the reference
predicate at [`:4306-4324`][soft-predicate] evaluated with the uncached
`tokenizer::estimate_tokens` on the frozen m0 payload and on the composed
```

Replacement:

```
Check: `always` - For every SOFT pass, `pressure_refold` equals a frozen copy
of the predicate at [`:4306-4324`][soft-predicate], kept as a test-only
reference function, evaluated with the uncached
`tokenizer::estimate_tokens` on the frozen m0 payload and on the composed
```

### B4, Check (excluded clause)

Current:

```
excluded parts hash `"excluded\0" ++ block.bytes`; the token cache is keyed
```

Replacement:

```
excluded parts hash `"excluded\0" ++ block.bytes` on the pre-match branch
(`tail_hygiene.rs:513-526`) and the kind-level branch (`:601-604`), and
`"excluded\0" ++ content` with the derived empty or drop-sentinel content on
the text, tool-result, and media branches (`:542-543`, `:569-570`,
`:584-585`, through `excluded_part` at `:280-289`); the token cache is keyed
```

### W7, Check (rationale sentence)

Current:

```
and bind-frozen `SessionBinding.config` stays frozen. `always` because
[`prepare_historian_fire`][call-fire] calls it every pass.
```

Replacement:

```
and bind-frozen `SessionBinding.config` stays frozen. `always` because
[`prepare_historian_fire`][call-fire] calls it on every non-subagent pass
whose state load succeeds, has no `pending_rewrite`, and has no live
historian completion pending (`lib.rs:8234`, `:5013-5051`), and
[`bind`][call-bind] calls it on every route bind; the check is on each call,
not on each pass.
```

### W7, Existing check

Current:

```
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the mtime test and the three privilege tests; all unaudited.
```

Replacement:

```
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the mtime test and the three privilege tests; all unaudited. Unit tests
built with `fixed_config` (`lib.rs:3859`, returned at `:4557-4560`) bypass the
cache and cannot exercise this record.
```

### W2, Check (first clause and bracket clause)

Current:

```
Check: `always` - Every field named by [`format_pass_timing_line`][fmt] and
by the plugin's [`rust module stages:` line][ts-stages] exists in
[`TransformTimings`][tt]; after a relocation each field's start and stop
instants still bracket the same work; a field for a removed or merged stage
```

Replacement:

```
Check: `always` - Every key printed by [`format_pass_timing_line`][fmt] and
every key the plugin's [`rust module stages:` line][ts-stages] reads resolves
to a field of [`TransformTimings`][tt] under a pinned key-to-field map, which
at HEAD is the identity except `post_attach_ms` for `post_attach`
(`transform.rs:1254`, `:1331`, field at `:1162`); each field's start and stop
instants are stated in a per-field table beside the struct and a relocation
changes the table in the same change (a review gate, not a runtime
assertion); a field for a removed or merged stage
```

### A2, Check (first sentence)

Current:

```
Check: `always` - Over a fixed corpus of body shapes assert three oracles
against the current reader. Routing: `route(body)` equals the
```

Replacement:

```
Check: `always` - Over a fixed corpus of body shapes assert three oracles
against a frozen reference: a test-only copy of HEAD's routing read, probe,
and `Value` decode kept under `crates/daemon/tests/` in the form of
[`historian_truncate_differential.rs`][diff-ref], or a recorded corpus of
expected route, code, and decoded request per body. Routing: `route(body)`
equals the
```

### W6, Check (reference clause)

Current:

```
transitions, a replacement returns the same `Option<i64>` as
[`next_occurrence`][stepper] at HEAD, including `None` for
```

Replacement:

```
transitions, a replacement returns the same `Option<i64>` as a frozen
test-only copy of [`next_occurrence`][stepper] as it reads at HEAD (the
function itself is replaced by the change and cannot remain the oracle),
including `None` for
```

### W4, Check (reference clause)

Current:

```
`semantic_digest` as the whole-input scan; if any input can differ,
```

Replacement:

```
`semantic_digest` as the whole-input scan, retained as a reference
evaluation mode or as a frozen copy of [`evaluate`][eval] at HEAD so the
comparison survives the change; if any input can differ,
```

### P2, Check (reference clause)

Current:

```
Check: `always` - For every database state, the collapsed [`isMidTurn`][ismidturn]
returns the same boolean as this reference: let A be the latest message by
```

Replacement:

```
Check: `always` - For every database state, the collapsed [`isMidTurn`][ismidturn]
returns the same boolean as a frozen reference function kept in the test file
(a copy of HEAD's `isMidTurnFromOpenCodeDb`) whose semantics are: let A be the
latest message by
```

### B5, Check (clone clause)

Current:

```
reattaches, at least one non-synthetic message carries a
[`synthetic_todo_`][todo-prefix] call or result id so
[`normalize_synthetic_todo_ingress`][normalize] produces a clone, and
```

Replacement:

```
reattaches, at least one non-synthetic message carries a
[`synthetic_todo_`][todo-prefix] call or result id (the input condition under
which HEAD's [`normalize_synthetic_todo_ingress`][normalize] clones; the
marker asserts the input, not the clone, so it fires under a shared-view
design as well), and
```

### C5, Check

Current:

```
Check: `sometimes` - For some pass, the `row_version` observed by its
post-commit read differs from the `row_version` its transform committed,
because another actor (historian publish, wrapup recut, or state sync)
committed in between. `sometimes` rather than `reachable` because the rerun
```

Replacement:

```
Check: `sometimes` - For some pass, a foreign commit by another actor
(historian publish, wrapup recut, or state sync) through a second store
handle returns a `row_version` greater than the one the pass's transform
committed, and that commit returns before the pass's first post-commit
`cache_state` read begins; both versions and the ordering are recorded from
the store and the actor, never from the pass's own read, which is what C1
tests. `sometimes` rather than `reachable` because the rerun
```

### G3, Check (first sentence)

Current:

```
Check: `sometimes` - A campaign observes usage before and after each path
that removes object bytes, and each occurs at least once with a non-zero byte
delta: [`cleanup_failed_reference`][cleanup] after a
```

Replacement:

```
Check: `sometimes` - A campaign records the independent `st_size` sum over
`objects` ([`regular_file_bytes`][walk]) before and after each path that
removes object bytes, with the writer lock released at both readings, under
one constant marker per path
(`artifact-byte-decrement-paths-are-exercised-cleanup`, `-recovery`,
`-reclaim`, `-purge`, `-retry`), and each marker fires at least once with a
non-zero byte delta: [`cleanup_failed_reference`][cleanup] after a
```

### W1, Check (first sentence)

Current:

```
Check: `sometimes` - For every stage the specification authorizes to change,
a recorded measurement run reaches that stage with an input in the
```

Replacement:

```
Check: `sometimes` - Under one constant marker per stage the specification
authorizes to change (the marker list is fixed when the specification
enumerates the stages; no name is built at run time), a recorded measurement
run reaches that stage with an input in the
```

### C2, Required faults and enabling state

Current:

```
Required faults and enabling state: A transform that rejects (ordinal
violation); a stable pass; an Emergency95 pass that reruns and commits twice;
a fresh session whose first pass commits; an injected failure in the
`pass_trace` upsert during a committing pass.
```

Replacement:

```
Required faults and enabling state: A transform that rejects (ordinal
violation); a stable pass; an Emergency95 pass that reruns and commits twice;
a CAS conflict on the first commit attempt so the [retry loop][cas-retry]
reruns `apply_once` and commits once (one breadcrumb, not zero or two); a
fresh session whose first pass commits; an injected failure in the
`pass_trace` upsert during a committing pass (no store seam exists at HEAD;
the four `fail_next_*_for_test` seams at `memory-store/src/lib.rs:5900-5928`
cover the side channel, the authority route read, and dreamer tasks only).
```

### fault-map.md, "Trace and drain faults" row (line 20)

Current:

```
| Trace and drain faults | A `pass_trace` upsert failure and an outbox delivery failure are injectable at the store; two drainers exist in production (pass and publish task). | No fault point exists for a crash between the mark commit and the delete commit; SIGKILL at that point needs a child process. |
```

Replacement:

```
| Trace and drain faults | An outbox delivery failure is injectable at the store (`fail_next_historian_side_channel_for_test`); two drainers exist in production (pass and publish task). | No seam injects a `pass_trace` upsert failure (the store's four `fail_next_*_for_test` seams at `memory-store/src/lib.rs:5900-5928` do not cover it); no fault point exists for a crash between the mark commit and the delete commit; SIGKILL at that point needs a child process. |
```

### Reachability text, `catalog.md:64-66`

Current:

```
prove such a worker is ready. T3 and T4 are test-only at HEAD because the
direct path has no production sender; the specification would move them to
default-production for transform responses.
```

Replacement:

```
prove such a worker is ready. T3 and T4 are test-only at HEAD because the
direct path has no production sender; the specification would move them to
default-production for whichever routes it moves to the direct path,
transform responses first. T3's check applies unchanged to a
`MeasuredSource::Json` response, which the owned path serializes twice at
HEAD (`crates/daemon/src/dispatch.rs:132-148` measures, `:237-249` writes).
```

### A1, Exercised

Current:

```
Exercised: not yet - No handler-level refusal or charge-versus-decode
comparison runs; existing tests enter at `dispatch_value`.
```

Replacement:

```
Exercised: not yet - No handler-level refusal or charge-versus-decode
comparison runs; existing tests enter at `dispatch_value`. The real-host
fixture in `crates/daemon/tests/support/direct_host.rs` (`FixtureProcess`,
used by `direct_host.rs:48-70`) reaches `Handler::handle` over the ring and is
the seam for a handler-level test.
```

### A1, Check (copy-count clause)

Current:

```
string_bytes * RETAINED_STRING_COPIES + VALUE_ENVELOPE_BYTES`, with at most
[`RETAINED_STRING_COPIES`][copies] owned copies of each string block
retained by the typed decode. `always` because every request evaluates this
```

Replacement:

```
string_bytes * RETAINED_STRING_COPIES + VALUE_ENVELOPE_BYTES`, with at most
[`RETAINED_STRING_COPIES`][copies] owned copies of each string block
retained by the typed decode, observed structurally (the `Value` node, the
`WireMessage` `original`, and the `WireBlock` `original` for one known
block) or through a counting allocator, since the decoded type exposes no
copy count. `always` because every request evaluates this
```

## Gaps queued

1. Panic redaction and thread-local state across a worker-thread boundary
   (finding 13, with finding 17 folded in). One safety record, `always`: a
   panic inside handler work that runs on a `spawn_blocking` worker prints
   only the redacted diagnostic and never the payload, and every
   thread-local the relocated work reads (`panic_boundary.rs:11`,
   `token_cache.rs:57`) is either carried across or its consumer is moved
   with it. One `sometimes` witness: a panic was injected inside a
   `kernel_routes::blocking` closure (or the relocated transform) while a
   callback was in flight, observed as a worker-thread panic and a redacted
   stderr, recorded separately. Reachability is default-production at HEAD
   through `kernel_routes::blocking` (`mod.rs:462-468`; ten call sites,
   including `ingest.rs:577` inside `:797`). Existing check: the host-runtime
   record `every-callback-invocation-is-inside-the-redaction-guard` and
   `crates/host-runtime/tests/dispatch.rs:603` cover the runtime worker only.
   Dedupe against the host-runtime catalog before adding.
2. Occurrence witnesses for the explicit-config-only `always` records
   (finding 12). Either two `sometimes` records, "a drain delivered at least
   one row of each of the three kinds" beside C3 and "the scheduler evaluated
   a configured cron through `next_due`" beside W6, or reclassify C3 and W6
   as `always-or-unreached` with the exemption recorded. The choice depends
   on whether reaching those paths is part of the campaign contract; see
   bias 4.
3. Plugin connection cache versus a replaced database file (finding 16).
   Discovery on whether `isMidTurn` must observe a file replaced at the same
   path (inode change) and what the plugin does when `openCodeDbExists()`
   turns false and true again; the connection cache is path-keyed
   (`read-session-db.ts:53-63`). Reachability is unresolved until OpenCode's
   write behavior for its database file is known; record it as such.
4. Blocking-pool tenancy under relocation (finding 18). A bounded record
   naming the tenants a relocated transform shares the pool with
   (`kernel_routes::blocking` at ten sites, `MemoryStore::open` at
   `lib.rs:3808`, `routing.rs:591`) and the bound the specification chooses:
   a worker count, a queue depth, or a per-request deadline in the units the
   code fixes. Not a liveness claim with an invented deadline; state the
   bound the specification sets or mark it needs human input.
5. Emergency95 rerun witness. C1's floor comparison and C2's double-count
   clause depend on an Emergency95 pass that reran and committed twice; C5
   fires only when a foreign write also lands. Extend the parent's queued
   gap 4 (pass-class witnesses) with this case rather than adding a record
   here.

Re-evaluation is warranted if gap 1 lands, because it opens a category the
area does not have: a diagnostics-preservation record.

## Biases for a human

1. Reference is the HEAD code (finding 22). A2, W6, W9, W4, and P2 compare
   the candidate with the implementation the change replaces and name no
   artifact that outlives it. W5 and the caveman and selection differentials
   in `crates/daemon/tests/` use frozen copies. Judgment required: for each
   of the five, a frozen reference function or a recorded corpus of expected
   outputs, and which team owns the frozen files so they are not "fixed"
   when the candidate disagrees.
2. Topology framed as hypothetical (finding 23). The catalog says execution
   topology is unresolved and treats `spawn_blocking` as a future
   relocation. The daemon already runs `kernel_routes::blocking` at ten
   sites with a stated ownership convention: the resource rides in the
   worker's result and is released when the work ends
   (`kernel_routes/ingest.rs:513-517`, `:716-722`). Judgment required:
   whether the transform relocation adopts that convention, which answers
   the parent's E1 question and W8's open question for the transform, or
   rejects it and states why.
3. Values and bytes are the only preserved surface (finding 24). Every
   `always` record preserves an output, a stored byte, a count, or a
   classification. None preserves what the process reports about itself:
   panic-hook output, stderr diagnostics, thread identity of counters, or
   pool tenancy. Judgment required: whether the latency specification owns
   those surfaces, or hands them to the host-runtime catalog and records the
   handoff.
4. `always` for explicit-config-only records without a recorded exemption
   (finding 12). C3 and W6 pass vacuously under a default campaign.
   Judgment required: whether reaching the populated drain and the scheduler
   path is part of the campaign contract (then add the witnesses) or not
   (then use `always-or-unreached` and record the exemption).

## Disposition

Applied 2026-09-10 against the same baseline. Every line reference the
disposition added was read at HEAD before it was written. No test ran; no
source, test, or CI file changed. Files touched: `catalog.md`,
`existing-checks.md`, `fault-map.md`, this file, and `evidence/`.

### Refinements applied

All 24 entries under "Refinements to apply" were applied by exact substring
replacement. Each "Current" block matched exactly once in its target file;
surrounding text was not reflowed.

| Record | Field |
| --- | --- |
| T4 | Fault/timing angle; Required faults and enabling state |
| W10 | Exercised; Required faults and enabling state; Open questions |
| W9 | Required faults and enabling state; Check (reference clause) |
| B4 | Check (excluded clause) |
| W7 | Check (rationale sentence); Existing check |
| W2 | Check (first clause and bracket clause) |
| A2 | Check (first sentence) |
| W6 | Check (reference clause) |
| W4 | Check (reference clause) |
| P2 | Check (reference clause) |
| B5 | Check (clone clause) |
| C5 | Check |
| G3 | Check (first sentence) |
| W1 | Check (first sentence) |
| C2 | Required faults and enabling state |
| fault-map | "Trace and drain faults" row |
| reachability text | `catalog.md` paragraph after the reachability table |
| A1 | Exercised; Check (copy-count clause) |

Where a refinement changed a `Check:` or `Required faults and enabling
state` field, the record's evidence file gained a short paragraph at the end
of "What a test must construct" so the two do not disagree: A1, A2, B4, B5,
C2, C5, G3, P2, T4, W1, W2, W4, W6, W7, W9, W10 (16 files). The W7 and A1
additions restate the fixture constraints the refinements introduced.

### Refinements not applied

None.

### Gaps closed

| Gap | Closed by | Notes |
| --- | --- | --- |
| 1, finding 13 (panic redaction across the worker boundary), with finding 17 folded in | W12 `worker-thread-panics-stay-inside-the-redaction-boundary`, safety, `always`, default-production | Verified: the guard is the thread-local `CALLBACK_POLL_DEPTH` (`panic_boundary.rs:11-13`); the hook prints the redacted string only when the panicking thread's depth is non-zero and otherwise forwards the full panic to the previous hook (`:40-47`); `kernel_routes::blocking` (`mod.rs:462-468`) runs its closure on a `spawn_blocking` worker with no guard entry, so a panic there reaches the default hook. The terminal clause disagrees with HEAD for kernel routes: `blocking` maps the `JoinError` to a `store_unavailable` response by documented intent (`mod.rs:460-467`), while the host maps an in-handler panic to `internal_error` (`dispatch.rs:985-989`). Both sides are cited in the record and the choice is an open question (needs human input). The thread-local inventory (finding 17) is in the record's Fault/timing angle. |
| 2, finding 12 (C3 and W6 vacuous under a default campaign) | C6 `side-channel-row-is-due-during-a-drain` and W13 `cron-schedule-is-evaluated-for-a-configured-project`, both reachability, `sometimes`, explicit-config-only | Witnesses were chosen over `always-or-unreached` because both enabling states are constructible from fixtures already in the tree: `fail_next_historian_side_channel_for_test` is set-valued (`memory-store/src/lib.rs:5905-5908`), the memory-store fixture publishes all three kinds (`:18712-18750`), and the daemon status test drives a pass drain against a pending row (`daemon/src/lib.rs:35555-35612`); the scheduler tests build a `MODULE`-authority project with a real cron on a real store and tick a `ManualClock` past its instant (`dreamer_scheduler.rs:586-600`, `:680-704`). Bias 4 is therefore answered in favour of the witnesses; a human may still prefer the exemption. |
| 3, finding 16 (plugin cache versus a replaced database file) | P2 extended | Verified: the cache compares the path only (`read-session-db.ts:55`) and `openCodeDbExists` is re-checked per call (`:75`). P2's `Check:` and `Required faults and enabling state` now carry the file-identity clause; the evidence file has the construction and an investigation entry. Whether OpenCode replaces the file in place is not established by anything in this repository and is recorded as an open question (needs external input). Reachability stays unresolved. |

### Gaps remaining

1. The `sometimes` witness the evaluation paired with gap 1 (a panic
   injected inside a `kernel_routes::blocking` closure while a callback is
   in flight, recorded as a worker-thread panic and a redacted stderr). Not
   added: `blocking` takes an opaque closure and no store or route exposes a
   panic seam at HEAD, so the witness needs a test-only injection point the
   specification has not chosen. W12's Required faults name what it takes.
2. Gap 4, blocking-pool tenancy under relocation. Not added; it needs the
   bound the specification sets. Correction to the tenant list the
   evaluation gave: `routing.rs:591` is inside `#[cfg(test)] mod tests`
   (`crates/host-runtime/src/routing.rs:458-459`) and is not a production
   tenant. The production tenants at HEAD are the eight
   `kernel_routes::blocking` sites, `health.rs:224`, `mod.rs:358`, and
   `lib.rs:3808`.
3. Gap 5, the Emergency95 rerun witness, remains for the parent's queued
   gap 4 as the evaluation proposed.
4. P2's file-replacement reachability, pending OpenCode's write behavior.

### Corrections to the evaluation text

- "Ten call sites" for `kernel_routes::blocking` (Coverage balance (a) and
  Wildcard 1) is eight through `blocking` (`commit.rs:1013`, `:1038`,
  `egress.rs:201`, `eligibility.rs:419`, `ingest.rs:722`, `:797`,
  `read.rs:291`, `:311`) plus two direct `spawn_blocking` calls in
  `kernel_routes` (`health.rs:224`, `mod.rs:358`). The evaluation body is
  left as written; W12 carries the corrected inventory.
- `routing.rs:591` as a production pool tenant (Wildcard 2, gap 4): test
  code, as above.

### Biases left for a human

1. Reference is the HEAD code (finding 22). The five refinements name a
   frozen reference per record; who owns the frozen files, and frozen copy
   versus recorded corpus per record, remain the human's call.
2. Topology framed as hypothetical (finding 23). Unchanged. W12 now cites
   the `kernel_routes::blocking` precedent, but whether the transform adopts
   its ownership convention is still the parent's E1 question.
3. Values and bytes as the only preserved surface (finding 24). Partly
   addressed: W12 opens the diagnostics-preservation category the evaluation
   said would warrant re-evaluation. Thread identity of counters stays with
   W2; pool tenancy stays open under gap 4.
4. `always` for explicit-config-only records (finding 12). Answered here by
   adding witnesses (C6, W13) rather than exemptions, on constructibility
   evidence. The human decides whether reaching those paths is part of the
   campaign contract; if not, C3 and W6 move to `always-or-unreached` and
   C6 and W13 are dropped.

### Record set after disposition

39 records: 28 `always` and 11 `sometimes`; no `always-or-unreached`,
`reachable`, or `unreachable`; no liveness record. The W12 evidence file
exceeds the 60 to 120 line target (173 lines including 34 link definitions)
to keep every verified anchor; the C6 and W13 files are within it.

[call-bind]: ../../../../crates/daemon/src/lib.rs#L11801
[call-fire]: ../../../../crates/daemon/src/lib.rs#L5064
[cas-retry]: ../../../../crates/daemon/src/transform.rs#L1948-L1987
[cleanup]: ../../../../crates/kernel/src/cas/ingest.rs#L779-L849
[copies]: ../../../../crates/daemon/src/lib.rs#L15421-L15430
[diff-ref]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L13-L58
[eval]: ../../../../crates/secret-scanner/src/evaluator.rs#L35-L157
[fmt]: ../../../../crates/daemon/src/transform.rs#L1224-L1357
[from-writer]: ../../../../crates/host-runtime/src/handler.rs#L465-L472
[ismidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L73-L82
[normalize]: ../../../../crates/daemon/src/transform.rs#L2091-L2108
[soft-predicate]: ../../../../crates/daemon/src/transform.rs#L4306-L4324
[stepper]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L163-L192
[todo-prefix]: ../../../../crates/daemon/src/injection.rs#L187-L189
[ts-stages]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1013-L1042
[tt]: ../../../../crates/daemon/src/transform.rs#L1026-L1205
[walk]: ../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
