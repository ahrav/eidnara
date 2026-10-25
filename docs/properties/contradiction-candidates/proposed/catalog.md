# Proposed obligation catalog: P5 write-time contradiction candidates

Part of `docs/properties/contradiction-candidates/`. Companion part:
`../dependencies/` (discovered facts about implemented surfaces).

These records are proposed design obligations for behavior that is not built
at HEAD `3a3b916b7`. Every record carries `Status: active - PROPOSED`. A
record here is a specification input and a test target for the implementing
unit; it is not a discovered fact and no record claims the implementation
exists or satisfies it. Source: the
`/software-design:invariant-driven-domain-modeling` pass whose certificate is
`certificate.md`; its open items are listed in `open-questions.md`. The
records below are reproduced from that pass unchanged in substance.

Reachability labels name the intended class once the unit lands, per the
preamble reproduced below, and are re-verified at implementation time.

## Index

| Slug | Surface | Type | Semantics | Intended reachability |
| --- | --- | --- | --- | --- |
| cc-scorer-output-deterministic-under-contract-version | S-A | safety | always | explicit-config-only |
| cc-oversize-text-refused-whole-never-truncated | S-A | safety | always | explicit-config-only |
| cc-case-fold-applied-identically-to-both-sides | S-A | safety | always | explicit-config-only |
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | explicit-config-only |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | explicit-config-only |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | explicit-config-only |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | explicit-config-only |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | explicit-config-only |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | explicit-config-only |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | explicit-config-only |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | explicit-config-only |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | default-production |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | default-production |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | explicit-config-only |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | explicit-config-only |

15 records. Semantics distribution: 13 `always`, 2 `always(!X)`; all 15 are
safety. Intended reachability: 13 `explicit-config-only`, 2
`default-production`.

## Source preamble and records

Schema: `docs/properties/METHOD.md` at HEAD `3a3b916b7`. Every record is a
proposed design obligation for unbuilt behavior (`Status: active - PROPOSED`,
`Exercised: not yet`, `Existing check: none; proposed`). Reachability is the
intended class once the unit lands; the whole feature is behind
`contradiction_candidates.enabled`, default false, so every runtime record is
`explicit-config-only` except where noted. Each record adds a Falsifier line
and a Legal witness line after the schema fields. Slug prefix `cc-`.

Record count: 15. By surface: S-A 3, S-B 2, S-C 2, S-D 2, S-E 1, S-F 2,
S-G 1, S-H 1, R5 cross-cutting 1.

Vocabulary used below: "eligible decision" is the one shared predicate OQ-B
fixes (proposed: registry row live at the snapshot, `memory` domain, project
scope term accepted, disposition not in {`superseded`, `contradicted`,
`rejected`, `quarantined`}). "Scorer contract version V" names the constant
that embeds `ANALYSIS_CONTRACT_EPOCH` (`crates/retrieval/src/lexical/mod.rs:40`),
the fold function F, the formula, threshold, and K.

---

## S-A scorer

### cc-scorer-output-deterministic-under-contract-version

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U1 scorer module and its unit tests do not exist
Guarantee: For equal descriptor bytes, equal candidate set (as a set of
(decision object id, decision_id, summary bytes)), equal `LexicalBounds`,
and equal scorer contract version V, the scorer returns a byte-identical
ordered list of at most K (object id, decision_id, score, matched_atoms).
Check: `always` - call the scorer twice with the candidate list in two
different input orders and compare the outputs for equality, including tie
order; the semantics is `always` because the scorer is a pure function whose
every evaluation must satisfy the predicate.
Fault/timing angle: none; pure computation.
Required faults and enabling state: a candidate list with ties on score and
matched count; a list of more than K matches; two permutations of the same
list; a run on a second thread or process.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: a nondeterministic order defeats the rebuild obligation and makes the
"top K" a function of SQL row order.
Open questions:
- Tie-break order (proposed: score desc, matched desc, decision object id
  asc) and whether the formula is set-based over distinct folded atoms
  (OQ5-3) (needs human input).
Falsifier: the same inputs with the candidate list permuted yield a different
K-th element.
Legal witness: a 7-atom descriptor against three summaries scores (1.0, 0.43,
0.0) in both permutations with the same order.

### cc-oversize-text-refused-whole-never-truncated

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U1 scorer module does not exist
Guarantee: A descriptor over `max_input_bytes` or `max_atoms` produces zero
candidate rows and one counted refusal; a summary over the bounds excludes
that decision from the descriptor's candidate set with a count; no analysis of
a prefix of either text contributes to any score.
Check: `always` - for a descriptor of `max_input_bytes + 1` bytes the scorer
returns the refusal and the pass writes zero rows for that evidence id; for a
summary of `max_atoms + 1` atoms the decision is absent from the output while
the other decisions are scored; the semantics is `always` because every
evaluation of the analyzer carries this rule (`analysis.rs:78-95` refuses
before retention).
Fault/timing angle: none.
Required faults and enabling state: a descriptor one byte over the bound; a
descriptor with `max_atoms + 1` atoms; a summary over each bound alongside a
summary under them.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (the analyzer's own refusal is tested in
`crates/retrieval/tests/lexical_analysis.rs`, which does not cover this
caller)
Impact: a truncated analysis scores the wrong text and can produce a false
candidate the agent then disputes.
Open questions:
- The `LexicalBounds` constants for the descriptor (`MAX_SOURCE_UNIT_BYTES`
  is 64 KiB, `harness_sources.rs:612-616`) and for a summary (needs human
  input).
Falsifier: a 65 KiB descriptor yields rows whose atoms come from its first
64 KiB.
Legal witness: a 64 KiB descriptor is scored; a 64 KiB + 1 byte descriptor
counts `descriptor_refused_oversize` and writes nothing.

### cc-case-fold-applied-identically-to-both-sides

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U1 scorer module does not exist
Guarantee: One named fold function F, part of V, is applied to every atom of
the descriptor and of every summary before comparison; two texts that differ
only under F score 1.0 against each other.
Check: `always` - score(`"HTTPServer binds Port"`, `"httpserver BINDS port"`)
equals 1.0 and equals score of the text against itself; the semantics is
`always` because the fold is part of the pure function's contract.
Fault/timing angle: none.
Required faults and enabling state: mixed-case pairs; a non-ASCII pair whose
lowercase mapping changes byte length (for example `İ`); a pair that differs
only in diacritics (expected: not folded unless F says so).
Confidence: low - proposed; no implementation evidence (`analysis.rs:6`
states the analyzer leaves folding to the engine)
Existing check: none; proposed
Impact: a case difference hides a real overlap or, with an asymmetric fold,
makes score(a, b) differ from score(b, a).
Open questions:
- F definition: Unicode default lowercase (`str::to_lowercase`) only, or also
  diacritic folding to match FTS5 unicode61 (needs human input).
Falsifier: `"Foo"` against `"foo"` scores below 1.0, or F is applied to the
descriptor only.
Legal witness: `"Retry Budget"` against `"retry budget"` scores 1.0 with
matched_atoms 2.

## S-B trigger

### cc-pass-starts-only-after-publication-job-terminal

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 trigger does not exist
Guarantee: The candidate pass starts only after `CaptureJob::run`
(`source_capture.rs:653-676`) has produced its terminal `Count` and the
capture commit gate guard has been dropped (`source_capture.rs:800`); it runs
under an `EvalBudget` whose origin is the pass start and whose interrupt is the
daemon cancel, never the job's budget; the response bytes were handed to the
wire writer before the `AfterResponse` closure ran (`lib.rs:9157-9176`).
Check: `always` - with the `before_publish` and `between_screen_and_commit`
test hooks (`source_capture.rs:781, 839`) blocked, the candidate table has
zero rows and the pass counter `admitted` is zero; after the hooks release and
the job returns, the pass runs; the pass's budget object is not the job's; the
semantics is `always` because every publication must respect the ordering.
Fault/timing angle: the window between `publish_screened_between` returning
and `drop(gate)`; the window between the job's terminal and the pass start;
daemon cancel during either.
Required faults and enabling state: flag on; a publication with at least one
`Published` unit; the two test hooks held; a cancel raised while the job is
blocked in a hook.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: the pass would consume the 5 s publication budget or hold the gate,
delaying publication and session deletion (KTD2 stop condition).
Open questions:
- How the `Published` list and the descriptor text leave the gate: carried
  from the job or read back from the evidence object (OQ-C) (needs human
  input).
Falsifier: a candidate row exists while `between_screen_and_commit` is held.
Legal witness: hooks held, zero rows; hooks released, job `Completed`, pass
writes rows, publish latency counters `PublishMsLast` unchanged by the pass.

### cc-passes-in-flight-bounded-and-overflow-counted

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 trigger does not exist
Guarantee: At most P candidate passes exist at once (P a named constant,
proposed 1 per project binding); an offer that finds the bound full is refused
and counted, never queued without bound; daemon cancel ends every running
pass within its budget.
Check: `always` - after N rapid publications with the pass blocked on a test
hook, running passes are at most P and `refused_busy` equals N - P; the
semantics is `always` because every offer is subject to the bound.
Fault/timing angle: a burst of publications faster than one pass completes;
a pass blocked on a slow Kernel read.
Required faults and enabling state: flag on; a test hook that parks the pass;
N > P publications in one session.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: unbounded passes would hold Kernel readers and memory proportional to
publication rate.
Open questions:
- P and whether the bound is per project or per daemon (needs human input).
Falsifier: 1,000 rapid publications create 1,000 concurrent passes.
Legal witness: P = 1, three publications while parked: one pass, two refusals
counted.

## S-C candidate set bounding

### cc-candidate-set-bounded-with-counted-truncation

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U1/U2 read path does not exist
Guarantee: The pass's candidate set is built by the project scope term
prefilter (`admission.rs:3060-3066`, `project.rs:106-111`) and the
`ScopeFilter` judge, then cut to at most N decisions in a deterministic
newest-first order with a counted truncation when more were eligible; summary
bytes are bounded before any payload is loaded
(`decision_payload_sizes_as_of`, `slice/read.rs:330`, or
`decision_page_within_budget`'s `max_payload_bytes`, `:126-133`); the pass
never loads more than N summaries or more than the byte bound.
Check: `always` - with N + 1 eligible decisions the pass scores exactly N, the
oldest is the one omitted, and `candidate_set_truncated` increments by one; a
decision with an oversize payload is skipped before load and counted; the
semantics is `always` because every pass must stay inside the bound.
Fault/timing angle: a project whose `memory` decisions exceed N; a decision
with a very large summary.
Required faults and enabling state: flag on; N + 1 eligible decisions in one
project and some in another project (must be excluded by the prefilter); one
decision whose payload exceeds the byte bound.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`kernel.read` uses the same pattern at
`read.rs:170-230` for a different read)
Impact: pass cost grows with project size; a large project stalls publication
follow-up and holds Kernel readers.
Open questions:
- N; whether the SQL walk of `visible_domain_decisions_as_of_in_scope`
  (no budget, no limit, `admission.rs:3087-3098`) is acceptable or a budgeted
  Kernel variant is required (OQ5-1) (needs human input).
Falsifier: a project with 100,000 `memory` decisions makes the pass load
100,000 summaries.
Legal witness: N = 256, 300 eligible decisions: 256 scored, 44 omitted,
truncation counted once.

### cc-partial-result-is-descriptor-granular-prefix

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 budget handling does not exist
Guarantee: When the pass's budget is exhausted or interrupted, the rows
written equal exactly the rows a complete pass would have written for a
prefix of the publication's descriptors in publish order; a descriptor whose
candidate set was not fully scored contributes no rows; the exhaustion is
counted once.
Check: `always` - with a deadline injected during descriptor i of n, rows
exist for descriptors 1..i-1 only, each with its full top-K, and `deadline`
increments by one; the semantics is `always` because every interrupted pass
must leave this shape.
Fault/timing angle: deadline or cancel between two descriptors; deadline
inside the summary read or the scoring loop of one descriptor; crash between
two descriptor writes.
Required faults and enabling state: flag on; a batch of several `Published`
units; a budget that expires mid-pass (test clock or interrupt flag).
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: a half-scored descriptor's rows are not its top-K; a reader sees a
false ranking and the rebuild obligation cannot be stated.
Open questions:
- Whether a pass writes per descriptor or once at the end (persistence
  handoff); per descriptor is the proposal.
Falsifier: a deadline inside descriptor 3 of 5 leaves two of descriptor 3's
five rows.
Legal witness: deadline during descriptor 3: rows for descriptors 1 and 2
complete, none for 3, 4, 5; `deadline` = 1.

## S-D row semantics

### cc-row-key-unique-and-caps-enforced

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 table does not exist
Guarantee: The table key is (project digest, descriptor evidence object id,
decision object id); a write for an existing key replaces `decision_id`,
`score`, `matched_atoms`, `publish_tip`, `created_at_ms`; one pass writes at
most K rows per evidence id and at most M rows in total, stopping at M with a
count; the row carries no column that any command reads for admission.
Check: `always` - after any sequence of passes, `SELECT COUNT(*) GROUP BY
key` is 1 for every key; after one pass with a descriptor matching K + 1
decisions the evidence id has K rows; after one pass whose descriptors would
match more than M, exactly M rows are written and `pass_cap_reached`
increments; the semantics is `always` because the predicate holds at every
pass commit.
Fault/timing angle: a replayed publication (`Published.replayed`,
`harness_sources.rs:530`) re-scoring the same evidence id; two passes for one
project interleaving (bounded by P).
Required faults and enabling state: flag on; a descriptor overlapping K + 1
decisions; a batch whose total matches exceed M; the same unit published
twice (replay).
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: duplicate rows inflate candidate counts; uncapped rows make the
table grow with project size per pass.
Open questions:
- K and M (OQ5-3); whether a replayed descriptor is re-scored or skipped
  (both satisfy the record) (needs human input).
Falsifier: two rows with equal (project, evidence id, decision object id), or
K + 1 rows for one evidence id after one pass.
Legal witness: a descriptor matching 12 decisions with K = 5 yields 5 rows;
publishing it again yields the same 5 rows with a newer `publish_tip`.

### cc-table-rebuildable-from-evidence-bytes-and-live-decisions

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 rebuild path does not exist
Guarantee: For one Kernel snapshot, one scorer contract version V, and no
budget exhaustion, truncating the table and rebuilding it from the project's
live descriptors yields the same set of (key, decision_id, score,
matched_atoms); this holds because the pass scores the evidence object's
exact bytes, so a rebuild can read the same bytes back.
Check: `always` - run a pass, snapshot the rows, truncate, run the rebuild at
the same `tip`, compare the sets excluding `created_at_ms`; the semantics is
`always` because the derivation is deterministic by construction.
Fault/timing angle: a decision revised between pass and rebuild (changes the
expected set; excluded by fixing the snapshot); a descriptor whose evidence
was retired (excluded from the rebuild input and swept from the table).
Required faults and enabling state: flag on; a pass that wrote rows; a
rebuild entry point (operator or test) that enumerates live descriptors of
the project and reads evidence bytes under a budget.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: without this the table is a log, not a derivation, and the derived
classification (index D6) is false.
Open questions:
- Whether the pass carries text from the job or reads the evidence object
  (OQ-C); a carried copy must be byte-equal to the evidence (needs human
  input).
- Where V is recorded so a version change triggers drop-and-rebuild.
Falsifier: truncate + rebuild at the same snapshot yields a different set
because the pass scored a normalized or redacted copy of the text.
Legal witness: 9 rows before truncate; 9 equal rows after rebuild at the
same `tip`.

## S-E sweep

### cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 sweep does not exist
Guarantee: One sweep call examines at most one page of rows and drops exactly
those whose decision object is invalidated at the sweep's Kernel snapshot,
whose disposition is `superseded` or `contradicted` (or otherwise not
eligible under OQ-B), or whose descriptor object or evidence row is
invalidated; every other row is kept; a repeat at the same tip drops zero; the
sweep writes no Kernel row; a Kernel that is not readable yields zero drops
and an unavailable result.
Check: `always` - seed rows for decisions in states {active, disputed, stale,
superseded, contradicted, invalidated} and for one retired descriptor; one
sweep drops the superseded, contradicted, invalidated, and retired-descriptor
rows and keeps active, disputed, stale; a second sweep drops zero; the Kernel
commit log row count is equal before and after; with the Kernel closed the
sweep returns unavailable and the table is unchanged; the semantics is
`always` because every sweep step must satisfy the predicate.
Fault/timing angle: a pass writing a row between the sweep's judgment and its
delete (the row survives to the next sweep); the Kernel retention sweep
retiring evidence concurrently; more rows than one page.
Required faults and enabling state: flag on; rows in each state; a Kernel
store closed during the sweep; a page overflow.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (precedent sweeps: `expire_memory_reviewer_work`
`memory_reviewer_jobs.rs:1629`, `prune_terminal_memory_captures`
`memory_capture.rs:484-491`)
Impact: over-dropping loses candidates for live decisions; under-dropping
serves rows for dead decisions; a Kernel write would violate R5.
Open questions:
- Whether `stale` and `disputed` rows are kept (proposed yes: both are
  confirmable targets under P4 C2 lateral moves) (needs human input).
- Page size and who schedules the sweep (daemon-called, as the precedents).
Falsifier: a sweep drops a row whose decision is `disputed` and live, or a
sweep with the Kernel unavailable drops every row.
Legal witness: six seeded rows, one sweep drops three, a repeat drops zero,
commit log unchanged.

## S-F read surfaces

### cc-read-serves-only-live-decision-rows-ids-and-scores-only

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U4 read route and tool action do not exist
Guarantee: The owner read and the `candidates` action answer only for the
route's bound project; they check the decision's standing in the Kernel at
read time and answer a terminal (proposed `not_live`) with no rows when the
decision is not eligible; otherwise they answer a bounded page of rows
ordered (score desc, created desc, evidence id asc) whose fields are
`evidence_id`, `decision_id`, `score`, `matched_atoms`, `publish_tip`,
`created_at_ms`, and nothing else: no descriptor text, no summary, no atom
list.
Check: `always` - a read for a `superseded` decision answers the terminal
even when rows exist; a read for an active decision with 70 rows and limit 64
answers 64 rows and a `next`; the response JSON key set equals the six names;
a read bound to project Q for a decision of project P answers the kernel
routes' scope refusal; the semantics is `always` because every response must
satisfy the predicate.
Fault/timing angle: the decision superseded between the pass and the read; a
row written between the standing read and the table read (legal omission).
Required faults and enabling state: flag on; rows for an active and a
superseded decision; more rows than the page bound; two project bindings.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`review.list` paging contract at
`docs/host-wire-protocol.md:792` is the model)
Impact: rows for a dead decision invite a dispute the Kernel will refuse;
text in the answer is a new disclosure path for Sensitive harness capture
(`source_capture.rs:710-716`) to the model.
Open questions:
- Terminal name and the section 7.8 method literal (API handoff); whether
  `matched_atoms` as a count is acceptable where the atom list is not (yes
  proposed: a count is not text).
Falsifier: the read answers rows for a `superseded` decision, or a row
includes `summary`.
Legal witness: active decision with 3 rows answers 3 rows and `next: null`;
the same call after the decision is superseded answers `not_live`.

### cc-read-distinguishes-disabled-unavailable-and-empty

Type: safety
Reachability: default-production (the terminal branches are reachable with
the flag at its default)
Status: active - PROPOSED
Exercised: not yet - U4 read route and tool action do not exist
Guarantee: Flag off answers `disabled` without touching the table; a
memory-store that cannot be read answers `store_unavailable`; a Kernel that is
not Ready answers the kernel routes' state; an eligible decision with no rows
answers an empty page; no terminal is ever rendered as an empty page.
Check: `always` - the four conditions answer four distinct shapes; the
semantics is `always` because every response must be one of them.
Fault/timing angle: Kernel Starting at read; memory-store file unreadable;
flag toggled between route bind and read (the frozen snapshot decides).
Required faults and enabling state: a route with the flag off; a route with
the flag on and the store removed; a Kernel in Starting; an eligible decision
with zero rows.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`review.list` terminal vocabulary at
`docs/host-wire-protocol.md:790`)
Impact: an agent reading an empty page as "no contradiction" when the store
was unavailable.
Open questions: None.
Falsifier: Kernel Starting answers `{items: [], next: null}`.
Legal witness: flag off: `disabled`; flag on, store missing:
`store_unavailable`; flag on, no rows: `{items: [], next: null}`.

## S-G reachability

### cc-flag-off-no-pass-no-write-no-table-read

Type: safety
Reachability: default-production (the flag's default is off; this record
describes the default build's behavior)
Status: active - PROPOSED
Exercised: not yet - U2 flag and U3 table do not exist
Guarantee: With the route's frozen `contradiction_candidates.enabled` false
(default), no candidate pass is offered after any publication, no row is
written, and neither read surface opens the table; the flag is enabled only by
explicit configuration at the `/contradiction_candidates/enabled` path and is
frozen in the route's consent snapshot like `/source_capture/enabled`
(`config.rs:102,154,993`).
Check: `always(!X)` - X is "a pass counter increments, a row exists, or a
table read statement executes for a route whose frozen flag is false"; this
is a forbidden state with no dedicated detection point, so `always(!X)`
rather than `unreachable`; assert by publishing with the flag off and
checking counters, row count, and a statement-trace hook.
Fault/timing angle: a config reload that turns the flag on after the route
bound (the route keeps false); a daemon restart with the flag on (new routes
see true).
Required faults and enabling state: a default config; publications with
source capture on; a `candidates` call and an owner read under the default.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: an unmeasured matcher reaches users (KTD4) and a surprise write load
appears in deployments that did not opt in.
Open questions:
- Whether the TS plugin hides the `candidates` action from the tool schema
  when the daemon reports the flag off, since the schema itself is a visible
  behavior change (OQ-E) (needs human input).
Falsifier: flag off, a `candidates` call opens the table and answers an empty
page instead of `disabled`.
Legal witness: default config, five publications: zero pass admissions, zero
rows, `candidates` answers `disabled`.

## S-H confirm linkage

### cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 table and U4 surfaces do not exist
Guarantee: A row's `evidence_id` is byte-identical to the
`Published.evidence_object_id` of the descriptor's publication
(`harness_sources.rs:521-532`) and its decision object id is the `object_id`
P4 `dispute` takes, so `dispute {object_id, event, evidence_id}` copied from a
row is well formed; the list performs no evidence-liveness check; a confirm
whose evidence has been retired is refused `not_found` by P4 C6 at commit;
the confirm path never reads the candidate table.
Check: `always` - for every row the evidence id equals the publication
receipt's evidence object id and names an `evidence_meta` row at write time;
a `dispute` built from a row with live evidence lands `Disputed`; after the
evidence is retired the same row is still listed and the same `dispute` is
refused `not_found`; the semantics is `always` because every row and every
confirm must satisfy it.
Fault/timing angle: the `harness_capture` 30-day retention sweep retiring the
evidence between list and confirm (`schema.rs:135`); a session delete retiring
the descriptor.
Required faults and enabling state: flag on; P4 D8 `dispute` landed; a row
for a live descriptor; evidence retired by the Kernel sweep or
`retire_evidence`; a confirm after retirement.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (P4 proposed record
`cited-evidence-live-at-dispute-commit` owns the confirm-side check)
Impact: a row carrying the descriptor `object_id` instead of the evidence id
makes every confirm fail; a list-side liveness check would hide the refusal
the confirm must own and cost a Kernel read per row.
Open questions:
- Under P4 OQ4-3's recommended visibility definition, a retired descriptor
  leaves its evidence uncited and the confirm is `not_found` even if the
  evidence row is live; the record's expected refusal name is the same.
Falsifier: a row whose `evidence_id` is the `srcdesc:*` object id; or the
list omits rows with retired evidence.
Legal witness: row (E, O1); `dispute {object_id: O1, event: mark_disputed,
evidence_id: E}` lands `Disputed`; after retiring E the row is listed and the
confirm is `not_found`.

## Cross-cutting R5

### cc-pass-and-sweep-write-no-kernel-event-and-no-signal

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2/U3 do not exist
Guarantee: Across any pass and any sweep the Kernel commit log, admission
rows, and object registry are unchanged, no decision's disposition or
maturity changes, no MemoryReviewer job is reserved, and every
`CausalInputs.signals` constructed in the daemon remains empty
(`memory_reviewer_jobs.rs:79-82`; producers at `selection.rs:314`,
`handoff.rs:374`, `lifecycle.rs:521`).
Check: `always(!X)` - X is "a Kernel write, a reviewer reservation, or a
non-empty `signals` attributable to the candidate feature"; a forbidden state
with no dedicated detection point; assert by comparing commit-log and
reviewer-job row counts before and after a pass and a sweep, and by a
source-level check that no code path from the candidate module reaches a
Kernel write family or a `CausalInputs` constructor with a non-empty vector.
Fault/timing angle: none; the obligation is structural.
Required faults and enabling state: flag on; a pass that writes rows; a sweep
that drops rows; a candidate whose score would tempt an automatic dispute.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: the plan's three stop conditions; a signal would reserve a
receipt-charged reviewer job against the Q38 ceiling (2,048 per project per
incarnation, `docs/memory-reviewer-operations.md:247`).
Open questions: None.
Falsifier: a non-empty `signals` vector built from candidate rows, or a
commit-log row attributed to the pass.
Legal witness: commit-log count 412 before the pass, 412 after; reviewer job
count unchanged; rows written 5.
