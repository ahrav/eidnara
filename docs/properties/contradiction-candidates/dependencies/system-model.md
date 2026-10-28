# System model: implemented surfaces the P5 mechanism depends on

System: `/local/home/ahrav/scratch/eidnara`, HEAD `3a3b916b7`, 2026-10-24.
Working tree: one unrelated modification (`rust-toolchain.toml`, adds the
`rust-analyzer` component; channel unchanged at 1.98).

Scope: the eight implemented surfaces S1 to S8 named in the task. The new P5
mechanism (scorer, trigger, `contradiction_candidates` table, owner read,
`candidates` tool action) is unbuilt and belongs to the domain-modeling pass.
Nothing below catalogs it; it appears only as the consumer whose assumptions
each surface must satisfy.

External-evidence scope (user-confirmed, recorded verbatim): the plan set
(00-index, P4, P5 plans), tracker specs #1145 and #1159 (copies at
`/tmp/p5-spec/spec-p3-1145.md` and `spec-p4-1159.md`), repository code,
tests, `docs/host-wire-protocol.md`, `docs/memory-reviewer-operations.md`,
`docs/search-projection.md`, and `docs/properties/` at HEAD. No other tracker
issues, no incident history, no git-history lens.

Why each reference was consulted: `plan-p5.md` for the consumer's assumptions
(R1 to R7, KTD1 to KTD4); `context.md` for the verified-fact baseline, which
this pass re-verified line by line; `spec-p4-1159.md` for the `dispute`
confirm path that P5 U4 hands a candidate to (evidence must be live, which
makes S6 retention a dependency); `spec-p3-1145.md` for the `review.*`
family P5 U4's owner read must follow; the wire doc and operations doc for
the contractual obligations S5 and S7 carry; `docs/properties/` for existing
records to link rather than duplicate.

Method: `docs/properties/METHOD.md` and the property-discovery skill. Every
`file:line` below was read at HEAD by this pass. Where `context.md` cited a
line that drifted, the corrected line is given and the drift noted.

## Lens: architecture and data flow

S1 retrieval analyzer. `retrieval::lexical::analyze(text, bounds)`
(`crates/retrieval/src/lexical/analysis.rs:68-70`) wraps
`analyze_segments` (`:78-105`). A pure function over `&str` returning
`Analysis { atoms, parts }` (`:33-36`) or `LexicalRefusal` (`:22-28`). It is
called on the production query path by `crates/daemon/src/query_route.rs:1055`
and `:1495` with `lexical_bounds(limits)` (`:1061-1066`: `query_bytes`,
`probes`), and on the index side by `crates/retrieval/src/lexical/index.rs:50`.
The analyzer fixes atom boundaries; FTS5 (`TOKENIZER` at
`crates/retrieval/src/lexical/mod.rs:30`, `unicode61 remove_diacritics 2`)
folds case and diacritics. The daemon depends on `retrieval` at
`crates/daemon/Cargo.toml:45` (plan cited `:41`; drifted).

S2 scope-filtered Kernel read. `KernelStore::visible_domain_decisions_as_of_in_scope`
(`crates/kernel/src/admission.rs:3168-3181`) calls `visible_selected_as_of`
(`:3183-3211`) with `ServedSelection::DomainDecisions(domain_id)`. That reads
one snapshot through `served_rows` (`:3460-3476`), which runs
`served_classes_selected` (`:3669-3747`) over the SQL in `served_classes_sql`
(`:3530-3645`). The domain/kind predicate is `o.domain_id=:domain_id AND
o.object_kind='decision'` (`:3557-3559`). The scope prefilter is the `WHERE`
tail at `:3625-3643`. Rows are then sorted by `object_id` (`:3203`). No row
limit and no budget parameter exist on this API. The daemon consumer is
`read_visible` in `crates/daemon/src/kernel_routes/read.rs:163-258`: SQL
prefilter with `project.scope_term()` (`:178`,
`crates/daemon/src/kernel_routes/project.rs:106-111`), then `ScopeFilter`
(`project.rs:186-222`) judges each kept row, then `NewestRows` heap of
`MAX_READ_ROWS = 8192` (`read.rs:26`, `:128-161`, `:191-196`), then payload
sizes before payloads (`:198-234`), then `decisions_for_objects_as_of` for the
retained ids (`:235-250`).

S3 budgeted Kernel reads. `decision_page_within_budget`
(`crates/kernel/src/slice/read.rs:126-220`): keyset page over the whole
`decisions` table (`:139-146`, no domain or scope predicate), `max_rows`
examined live or invalidated, byte cut that never stops before the first live
row (`:161-170`), `lock_reader_within(&budget.acquire_limit())` (`:134`).
`decisions_for_objects_as_of_within_budget` (`:236-243`) is the same acquire
plus `decisions_for_objects_on`. `decision_payload_sizes_as_of` (`:330-345`)
reads `length(decision_payload)` only, no budget. `claim_facts_at_within_budget`
(`crates/kernel/src/claim_facts.rs:240-257`): `limit.run(..)` then
`claim_facts_inner` (`:301-333`), which calls `check_claim_bounds` (`:336-358`)
before `reader_with_limit`. `ClaimFactBounds` (`:41-44`), `TooManyClaims`
(`:49`). `ClaimFacts` (`:146-156`) carries `object`, `decision:
ClaimDecisionFacts` (`:90-97`, ids only), admission facts, `served:
ServedStanding` (`:107-115`), occurrences, exclusions, causality. `EvalBudget`
(`crates/kernel/src/applicability/checkout.rs:145-150`; `linked` `:162-168`,
`is_exhausted` `:203-218`, `acquire_limit` `:235-238`).

S4 source capture. `crates/daemon/src/source_capture.rs:1-7` module header.
`CAPTURE_BUDGET = 5 s` (`:36`), `CAPTURE_DOMAIN_ID = "harness-capture"`
(`:54`), `counts!` macro (`:58-75`) and vocabulary (`:77-115`). Supervisor
`slot: Arc<Semaphore>` (`:235`) created with `Semaphore::new(1)` (`:267`).
`ActiveSlot` (`:473-478`) decrements `Active` on drop. `offer` (`:493`).
`into_job` (`:722-765`): `try_acquire_owned` or `RefusedQueueFull`; budget
origin `Instant::now() + CAPTURE_BUDGET` at acceptance (`:621-624`).
`CaptureJob::run` (`:665-691`): `spawn_blocking(self.publish())`, cancel sets
the interrupt and awaits the join. `publish` (`:697-880`): cancel check,
kernel store, budget, units, memo, commit gate mutex (`:811`),
`publish_locked` (`:883-975`): delete epoch, scope, budget,
`publish_screened_between` for the window (`:907-915`), then
`capture_documents` in its own batch (`:905-...`). `capture_after_response`
(`crates/daemon/src/lib.rs:9246-9263`). The request owner runs the
`AfterResponse` after `settle_prepared` (`lib.rs:13660-13664`;
`crates/daemon/src/dispatch.rs:24-35`, `:445-460`). `publish_batch`
(`crates/daemon/src/harness_sources.rs:1292-1298`), `Published` (`:549-562`),
`MAX_SOURCE_UNIT_BYTES`/`MAX_SOURCE_BATCH_UNITS`/`MAX_SOURCE_BATCH_BYTES`
(`:614-618`, resolving to 64 KiB, 32, 64 KiB through
`crates/memory-store/src/memory_capture.rs:19` and
`crates/daemon/src/memory_capture.rs:36-37`). Screening bounds applied at
`harness_sources.rs:1557`, `:1602-1603`.

S5 memory store baseline. `BASELINE = include_str!("../baseline.sql")`
(`crates/memory-store/src/lib.rs:335`), `baseline_digest()` (`:338-340`) =
`storage::baseline_digest(BASELINE)` (`crates/storage/src/lib.rs:2631`:
SHA-256 of the storage baseline, a newline, the consumer's). `MemoryStore::open`
(`lib.rs:8157-8158`) goes through `open_sqlite` (`storage/src/lib.rs:2032`).
Activation gate: `identity_terms` (`crates/daemon/src/memory_reviewer/activation.rs:174-200`)
includes `("memory store baseline", live.memstore_baseline_digest)`;
`evaluate` (`:218-240`) returns `Closed::IdentityMismatch(field)` on the first
differing term; the live value is `memory_store::baseline_digest()`
(`worker.rs:167`).

S6 harness-capture retention. `HARNESS_CAPTURE_RETENTION_CLASS =
"harness_capture"` (`crates/kernel/src/cas/mod.rs:155`); evidence ingest sets
`retain_until = observed_at + HARNESS_CAPTURE_RETENTION_MS` (30 days,
`harness_sources.rs:607-608`, `:991`). Sweep:
`KernelStore::expire_harness_captures` (`crates/kernel/src/local_file.rs:245-247`)
with `HARNESS_CAPTURES` (`:64-70`, owner kind `source_descriptor`, 64 captures
per commit); `expire_captures` (`:250-310`); `retire_capture` (`:487-501`:
owners first via `live_capture_observations`, then `cited_elsewhere`, then
`retire_evidence`); `EXPIRED_UNPINNED_SQL` (`:514-522`); candidate query
(`:549-565`); `EVIDENCE_CITED_SQL` (`crates/kernel/src/slice/write.rs:42-54`:
live observations, decisions, decision_events of live decisions, asserted
edges). Descriptor retirement is `retire_capture_observation`
(`write.rs:569-577`) -> `retire_slice_object` (`:627-666`) which sets
`invalidated_commit_seq`. Caller: `kernel_slice` in
`crates/daemon/src/memory_reviewer/lifecycle.rs:313-338`, from
`sweep_and_sample` (`:363`), from `run` (`:471-539`), spawned once the store
is installed (`lib.rs:4000-4008`), with `SAMPLE_INTERVAL = 30 s` and
`SAMPLE_RETRY_INTERVAL = 5 s` (`lifecycle.rs:16-18`).

S7 routed reads. `docs/host-wire-protocol.md:795-799` and `:819-827`
(`:827` is `review.use_outcome`, added by #1157).
`crates/daemon/src/memory_reviewer/wire.rs:1-26`; strict request structs
(`:33-60`, `deny_unknown_fields`); `bind_review` (`:203-220`) calls
`kernel_request` (`crates/daemon/src/kernel_routes/mod.rs:446-455`) which
binds scope (`kernel_route_scope` `:415-441`, project check at `:428-432`)
before `parse_request_body` (`:463-474`, removes `ENVELOPE_KEYS` `:461`:
`method`, `kind`, `session_id`, `project_root`). `authority_project`
(`wire.rs:131-139`) maps NotModule to `disabled`, store error to
`store_unavailable`. Observational harness `cli`
(`crates/daemon/src/lib.rs:274-281`), excluded from background views
(`:413-417`). `kernel.read` header `read.rs:1-4`.

S8 tool surface (re-verified at the #1164 branch tip; #1161 added
`dispute`). `EIDNARA_MEMORY_ACTIONS =
["create","get","revise","archive","merge","dispute"]`
(`packages/opencode-plugin/src/tools/eidnara-memory/types.ts:6-13`);
`EIDNARA_MEMORY_DEFAULT_ACTIONS` excludes `dispute` (`:17-19`); read set
`{get}` (`:24`); `isEidnaraMemoryMutation` (`:26-29`); `allowedActions?`
(`:57`). `tools.ts:103-106` allowlist default; `:125-137` execute guards
(approve/enforce, context-researcher agent, closed list and allowlist);
`:152-154` mutation needs a tool-call id.
`packages/opencode-plugin/src/plugin/tool-registry.ts:58` passes
`EIDNARA_MEMORY_DEFAULT_ACTIONS`. Pi plugin
`packages/pi-plugin/src/tools/eidnara-memory.ts:125-185` has the same
allowlist default, the allowlist check, and the mutation identity check;
exclusion of the context_researcher is by registration
(`packages/pi-plugin/src/tools/index.ts:30-32`, `:69`;
`packages/pi-plugin/src/subagent-entry.ts:63` `memoryToolEnabled: false`).

## Lens: state and persistence

- Kernel decisions, observations (source descriptors are observations of kind
  `source_descriptor`, `crates/kernel/src/source_descriptor.rs:30`),
  `evidence_meta` with `retention_class`, `retain_until`,
  `invalidated_commit_seq` (`crates/kernel/src/schema.rs:136`). Append-only
  registry; retirement sets `invalidated_commit_seq`, never deletes.
- Memory store: one baseline file, `format_marker` holds the digest; a
  mismatch refuses open and leaves bytes untouched
  (`crates/memory-store/tests/baseline.rs:492-514`).
- Capture memo is process-local and bounded (`source_capture.rs:5`); Kernel
  receipts are the durable record.
- Review receipts live in the memory store ledger; `review.list` lists from
  the row alone (`wire.rs:1`, module header).

## Lens: concurrency model

- One publication slot per daemon (`Semaphore::new(1)`); the job runs on
  `spawn_blocking`; a `commit_gate: Mutex<()>` serializes capture commits
  with session deletion (`source_capture.rs:813-879`, panic caught and
  resumed after the guard drops).
- The lifecycle loop is one task; `kernel_slice` runs on `spawn_blocking`
  and checks `cancelled()` between sweeps.
- Kernel reader pool: `lock_reader_within` polls `try_lock` across readers
  under an `AcquireLimit` (`crates/kernel/src/open.rs:565-593`);
  `LimitedConnection` installs a SQLite progress handler and zero busy
  timeout only when the limit has control (`:1693-1722`).
- `EvalBudget::linked` gives a child a private interrupt the parent still
  raises (`checkout.rs:162-168`).

## Lens: claimed safety guarantees (claims under test)

C1 `analysis.rs:76-77`: byte bound checked before any atom is scanned; the
atom after `max_atoms` is refused before retention.
C2 `analysis.rs:6`: "Parts keep their original bytes; the engine folds case
and diacritics."
C3 `crates/retrieval/AGENTS.md`: a change to any analysis rule needs a new
epoch and an updated pinned digest; mutations pure; every bound caller-supplied.
C4 `admission.rs:3162-3164`: domain and kind matched in SQL before any row is
read; a corrupt row outside the domain never fails this read.
C5 `admission.rs:3138-3144`: the scope restriction is a superset of the
algebra's verdict.
C6 `read.rs:26`, `:189`: the row bound applies after the filter; `:197`:
payload sizes read before any payload.
C7 `slice/read.rs:121`: a single decision larger than the bound is returned
alone; the wait for a pooled reader stops at the budget.
C8 `claim_facts.rs:232-234`: reader acquisition and every statement stop at
the deadline or on cancellation.
C9 `source_capture.rs:3`: "The response bytes never wait on the kernel or the
job"; single publication slot; at most one screened batch under a 5 s budget.
C10 `source_capture.rs:7`: counters carry a closed vocabulary, never unit text.
C11 `storage/AGENTS.md`: single baseline; reject mismatches.
`memory-reviewer-operations.md:86-88`, `:235-241`: a store at another
baseline refuses to open; the activation gate binds `memstore_baseline_digest`.
C12 `local_file.rs:240`: owners first, evidence second; cited evidence kept;
descriptors retired.
C13 `host-wire-protocol.md:799`, `:819`: strict bodies, project checked before
the body is read, `disabled`/`store_unavailable` terminals.
C14 `types.ts:6-29`, `tools.ts:103-106`: closed action list; `get` is the
only read; an omitted allowlist admits the default actions (all but
`dispute`), an empty one admits none.

## Lens: claimed liveness guarantees

L1 `local_file.rs:240-247`, `lifecycle.rs:16-18`: an expired, unpinned,
uncited harness capture is retired by the maintenance sweep, at most 64 per
call, with the loop repeating at 5 s while a pass advances and 30 s otherwise.
L2 `source_capture.rs:5`: "A window refused at a full slot and never offered
again is lost to capture; the next pass that carries it is captured." (A
no-guarantee statement: loss is accepted.)

## Lens: existing test strategy

See `existing-checks.md`. The analyzer has hand-authored goldens and a
pinned digest. The Kernel read has a superset-prefilter test and
domain-selection equivalence tests. Capture has a large integration suite
(`source_capture_tests.rs`) including a latency measurement and a
response-identity test. The sweep has in-module unit tests for pin recheck
and page rollback plus one lifecycle integration test. The wire family has
one strict-body test per method. The tool surface has an allowlist suite.

## Lens: failure and degradation

- Capture: every exit is a `Count` terminal; `RefusedQueueFull` drops the
  window; `Deadline` leaves units pending in the memo for the next pass.
- Kernel reads: `KernelError::Deadline` from the acquire limit;
  `CorruptCanonicalRow` fails closed when a decision row is missing.
- Sweep: a refused capture rolls its commit back and the page is committed
  again without it (`local_file.rs:304-306`).

## Lens: unproven assumptions and contract-versus-code disagreements (leads)

D1 Plan R2 says decision summaries come "through `claim_facts_at_within_budget`".
`ClaimFacts` (`claim_facts.rs:146-156`) and `ClaimDecisionFacts` (`:90-97`)
carry no `summary` or `rationale`. Summary text at HEAD comes only from
`DecisionRow.payload` via `decisions_for_objects_as_of[_within_budget]`
(`slice/read.rs:221-243`) or `decision_page_within_budget`
(`:126`, `PagedDecision.decision`). Plan versus code; the code is the fact.

D2 Plan OQ5-1 treats `decision_page_within_budget` as a budgeted
alternative. It pages the whole `decisions` table in `object_id` order with
no domain or scope predicate (`slice/read.rs:139-146`), so for P5 it would
examine every project's and every domain's decisions. Its budget bounds only
the reader acquisition wait (`:134`, `lock_reader_within`), not the statement:
`LimitedConnection` and its progress handler are used by `claim_facts`
(`reader_with_limit`, `claim_facts.rs:313`) but not here. The row scan is
bounded by `max_rows` instead. The doc comment at `:121` is accurate about
the wait; the plan's reading of "budgeted" is broader than the code.

D3 `source_capture.rs:3` says the job "publishes at most one screened batch".
`publish_locked` publishes the window batch (`:908-916`) and then
`capture_documents` publishes the pass's configured documents "in their own
batch after the window's" (`:976-980`, `publish_screened` at `:1055`). Two
screened batches per job when documents are pending. The per-batch bounds
(32 units, 64 KiB) hold for each.

D4 `crates/retrieval/AGENTS.md` ties the pinned digest to analysis-rule
changes. The digest preimage (`identity.rs:36-65`) also includes
`char::UNICODE_VERSION`, `rusqlite::version()`, and `SQLITE_SOURCE_ID`, so a
toolchain or SQLite bump changes `AnalysisIdentity::current()` and fails
`identity_pins_the_manifest` (`tests/lexical_analysis.rs:389-412`) with no
rule change and no epoch change. Documented intent (`identity.rs:35`) is that
this is deliberate. A P5 consumer that pins the epoch string alone pins less
than the identity does.

D5 The Kernel scope prefilter keeps rows whose `scope_id` is NULL
(`admission.rs:3637`), and the daemon `ScopeFilter::matches` returns `false`
for `None` (`project.rs:212-214`). The superset claim holds; a P5 consumer
that uses the Kernel API without the daemon filter would see unscoped rows
the daemon never serves.

D6 `host-wire-protocol.md:797` names the envelope as `method`, `session_id`,
`project_root`; `ENVELOPE_KEYS` (`kernel_routes/mod.rs:461`) also strips
`kind`. A body field named `kind` is therefore not `invalid_params`. The doc
is silent on `kind`; recorded as a lead, not a defect.

D7 At `3a3b916b7` the pi plugin's `eidnara_memory` tool had no
`allowedActions`. #1161 resolved this: both plugins default to
`EIDNARA_MEMORY_DEFAULT_ACTIONS` (pi `eidnara-memory.ts:129-134`, opencode
`packages/opencode-plugin/src/plugin/tool-registry.ts:58`), so the P4/P5
"allowlist as rollout lever" assumption holds in both.

D8 `context.md` cited `capture_after_response` at `lib.rs:9220-9239`
(verified `:9220-9237`) and the plan cited `:9207-9221` (drifted). Plan cited
`Cargo.toml:41`; `retrieval` is at `:45`.

## Lens: wildcard

W1 `EVIDENCE_CITED_SQL` counts `decision_events` of live decisions. A P4
`dispute` event that cites a descriptor's evidence id keeps that evidence
alive past its 30-day `retain_until` for as long as the decision is live; the
descriptor (owner observation) is still retired. A P5 candidate confirmed
through `dispute` therefore changes the retention outcome of the evidence it
cites. Cross-mechanism coupling worth a record.

W2 `expired_captures_sql` reads every retained or pinned row ahead of the
page on every call (`local_file.rs:523` doc comment), so the sweep's read cost
grows with retained history even though its writes are bounded. The in-module
test `expiry_candidate_cost_does_not_grow_with_retired_history` (`:1138`)
checks the retired case.

W3 The capture lifecycle loop runs whenever the memory store is installed,
independent of `source_capture.enabled` and of the MemoryReviewer activation
gate. The sweep code path is default-production; the branch that retires a
row needs `harness_capture` rows, which only explicit capture config produces.

## Lens: product context

The P5 consumer reads descriptor text (S4 `SourceUnit.text`, in hand at
publish), decisions (S2/S3), tokenizes (S1), stores derived rows in the memory
store (S5 baseline rotation), depends on evidence being live when a candidate
is confirmed (S6), and exposes results through S7 and S8. A failure in S1 or
S3 bounds is unbounded daemon work after every pass; a failure in S2's scope
filter is cross-project leakage; a failure in S6 is a confirm refused
`not_found` (P4 C6/C7) or evidence kept longer than its class promises.
