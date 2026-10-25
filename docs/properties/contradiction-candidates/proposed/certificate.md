Output certificate of `/software-design:invariant-driven-domain-modeling` for the P5
new behavior (write-time contradiction candidates) at HEAD `3a3b916b7`.

# P5 write-time contradiction candidates: invariant-driven domain model certificate

Method: `/software-design:invariant-driven-domain-modeling`, applied to the
unbuilt behavior in `/tmp/p5-spec/plan-p5.md` (KTD1-KTD4, R1-R7, U1-U4, OQ5-1..3
preserved as authoritative). Every statement below is a proposed design
obligation. Nothing here is implementation evidence or proof. Repository facts
are cited at HEAD `3a3b916b7` and were re-read for this certificate where they
bear on a rule; the rest are taken from `/tmp/p5-spec/context.md`.

Altitude gate. Deployable: the eidnara daemon. Owned semantic state for this
feature: the `contradiction_candidates` derived table in the memory-store
(daemon-owned, `crates/memory-store/baseline.sql`, single baseline, Batch M1).
Authority read, never written: the Kernel (decisions in domain `memory`,
`canonical_memory.rs:19`; source descriptors `srcdesc:*` and their evidence
objects in domain `harness-capture`, `source_capture.rs:53`, `schema.rs:93`).
Independently failing participants: the Kernel store (may be Starting or
Unavailable), the memory-store (may be unreadable), the publication job (its own
5 s budget, `source_capture.rs:3`), the daemon runtime (shutdown cancel). No
ownership-changing routing is needed: the plan already placed the table in the
memory-store (KTD3) and the confirm path in P4's `kernel.commit` (index D8 of
#1159). Representation, transaction participation, and in-process topology are
handed off at the end.

---

```text
SCOPE AND AUTHORITY
```

Owned state: `contradiction_candidates` rows. Row = (project digest,
descriptor evidence object id, decision object id) key with values
(decision_id scored, score, matched_atoms, publish_tip, created_at_ms). The
row is derived (index D6) and authorizes nothing.

Actors and decision authority:
- Post-publish pass (writer). Decides which rows exist for one publication's
  descriptors. Authority over the table only.
- Sweep (writer). Decides which rows are dropped. Authority over the table only.
- Owner observational read (reader; `review.*` family, wire doc section 7.8).
- `eidnara_memory` `candidates` action (reader; the agent).
- The agent confirming via P4 `dispute` (a Kernel command through
  `kernel.commit`; outside this table; the Kernel admission path is the
  authority for the disposition).
- Kernel maintenance sweep for `harness_capture` retention (external writer of
  the authority the table is derived from; `schema.rs:135`).
- Operator (flag `contradiction_candidates.enabled`, route-frozen consent
  snapshot like `/source_capture/enabled`, `config.rs:102,154,993`).

Crossings: daemon -> Kernel reads (`visible_domain_decisions_as_of_in_scope`,
`admission.rs:3087-3098`; `decisions_for_objects_as_of_within_budget`,
`slice/read.rs:236-241`; `decision_payload_sizes_as_of`, `slice/read.rs:330`);
daemon -> memory-store writes (`PreparedWrite` family, precedent
`memory_capture.rs:484-491`); daemon -> plugin wire (routed read, tool action);
plugin -> daemon `kernel.commit` `disposition` with `evidence_id` (P4).

Routed decisions: domain-to-storage mapping, transaction participation,
read-modify-write protection, and restart reconstruction go to
`/software-design:persistence-boundary-and-transaction-design`; which task owns
the pass and the in-flight bound goes to
`/software-design:in-process-concurrency-topology-design`; wire shapes of the
`candidates` action and the owner read go to
`/software-architecture:api-design-and-evolution` within section 7.8's rules.

```text
BEHAVIOR
```

Commands (authoritative over the table):
- C-PASS `score_publication(project, published[], tip)`: for each published
  descriptor (evidence id, object id, exact bytes), compute candidates against
  the eligible `memory` decisions of the project at `tip`; upsert at most K rows
  per descriptor and M per pass.
- C-SWEEP `sweep_candidates(project?, page)`: drop rows whose decision or
  descriptor is no longer eligible at the sweep's Kernel snapshot; at most one
  page per call.
- C-REBUILD (operator or test; same code as C-PASS over live descriptors):
  truncate and rebuild. Named so the determinism obligation has a subject.

Queries:
- Q-OWNER `candidates_for_decision(project, decision object id, after, limit)`.
- Q-TOOL `eidnara_memory candidates {object_id}` (same answer contract, agent
  consumer, `allowedActions` admitted).

External command this feature feeds but does not own:
- P4 `dispute {object_id, event, evidence_id}`; the candidate row supplies the
  two ids; the Kernel and the daemon's C6/C7 checks decide.

Legal witness (end to end). Flag on. A pass publishes descriptor D with
evidence E (`Published.evidence_object_id`, `harness_sources.rs:521-532`)
scoped `project:<digest>`. The job reaches terminal `Completed`, the commit gate
is released (`source_capture.rs:800`), the pass starts under its own budget,
reads the project's eligible `memory` decisions at `tip`, scores D's folded
atoms against each summary, writes one row (digest, E, O1) with score 0.62,
matched 8, tip, created. Later the agent calls `candidates {object_id: O1}`,
receives `[ {evidence_id: E, decision_id, score: 0.62, matched_atoms: 8,
publish_tip, created_at_ms} ]`, and files `dispute {object_id: O1, event:
mark_disputed, evidence_id: E}`; the Kernel writes `Disputed`. The table is
unchanged by the dispute; the next sweep keeps the row (Disputed is not a
drop condition under R4).

Forbidden outcomes (from the plan, restated as named outcomes):
F1 any disposition changed by the pass or sweep; F2 any Kernel event or
commit-log row written by the pass or sweep; F3 any non-empty
`CausalInputs.signals` produced by this feature (`memory_reviewer_jobs.rs:79-82`);
F4 the pass executing inside the publication job's budget, under the capture
commit gate, or before the response bytes were handed to the wire writer; F5 a
model call; F6 any pass, write, or table read while the route's frozen flag is
off; F7 more than K rows for one descriptor in one pass, or more than M rows in
one pass; F8 work per pass that grows with project size without a counted cut;
F9 a candidate row used to admit or authorize any command; F10 descriptor text,
summary text, or matched atoms leaving the daemon through the read surfaces.

```text
RULES
```

Each rule: temporal kind; observation dependencies (Axis 2); validity point;
status. Falsifiers are collected in FALSIFIERS.

R-A1 Scorer determinism. Given identical (descriptor bytes, candidate list as
an unordered set of (object id, decision id, summary bytes), `LexicalBounds`,
threshold, K, scorer contract version which embeds
`ANALYSIS_CONTRACT_EPOCH` = `identifier-analysis.v1`, `lexical/mod.rs:40`), the
scorer returns the same ordered list. Kind: state predicate on a pure
function's output (intrinsic/local only). Validity: return. Settled (R3).

R-A2 Refuse whole, never truncate. A descriptor whose bytes exceed
`max_input_bytes` or whose atoms exceed `max_atoms` produces zero rows and a
counted refusal; a summary that exceeds the bounds excludes that decision from
that descriptor's candidate set with a count. No analysis of a prefix ever
contributes (`analysis.rs:78-95` refuses before retention). Kind: state
predicate (intrinsic). Validity: return. Settled (R3, U1 step 1).

R-A3 Case fold rule. One named fold function F is applied to every atom on both
sides before comparison; F is part of the scorer contract version. Texts that
differ only under F score identically. The analyzer itself does not fold
(`analysis.rs:6`), so the daemon owns F. Kind: state predicate (intrinsic).
Validity: return. Settled in direction (R3), F's definition is OQ-A below.

R-B1 Trigger after terminal. The pass starts only after `CaptureJob::run`
(`source_capture.rs:653-676`) has returned its terminal `Count` and the capture
commit gate has been dropped (`source_capture.rs:800`). Kind: transition/history
safety (ordering). Observation: current runtime state (job terminal) only.
Validity: pass start. Settled (R1, KTD2).

R-B2 Own budget, never the response path. The pass runs under an `EvalBudget`
(`applicability/checkout.rs:146-154`) whose origin is the pass start, linked
to the daemon cancel, never to the job's budget; the response bytes were
handed to the wire writer before `AfterResponse` ran (`lib.rs:9157-9176`,
`source_capture.rs:3`). Kind: transition safety. Validity: pass start. Settled.

R-B3 Bounded in flight. At most P passes exist at once (P a named constant;
proposal 1 per project binding); an offer that finds the bound full is refused
and counted, never queued without bound. Kind: state predicate on runtime
(current authoritative runtime state). Validity: offer. Proposed (R1 implies).

R-C1 Candidate set bounded. The id read uses the project scope term prefilter
(`admission.rs:3060-3066`, `project.rs:106-111`) and the `ScopeFilter` judge,
then keeps at most N decisions by a deterministic newest-first order
(`read.rs:170-200` pattern), counting a truncation when more were eligible;
summary bytes are bounded by `decision_payload_sizes_as_of` before any payload
is loaded, or by `decision_page_within_budget`'s `max_payload_bytes`
(`slice/read.rs:126-133`). Kind: state predicate on the pass's read set
(derived/replicated observation of Kernel at `tip`). Validity: before scoring.
Settled in intent (OQ5-1 names the cap as spec-time).

R-C2 Partial result is a descriptor-granular prefix. When the budget is
exhausted or interrupted, the rows written equal the rows the full pass would
have written for a prefix of the descriptors in publish order; a descriptor
whose candidate set was not fully scored writes no rows; the exhaustion is
counted. Kind: transition safety. Validity: pass end. Proposed (U2 test
"partial results written, exhaustion counted" sharpened).

R-D1 Row key and caps. Key = (project digest, evidence object id, decision
object id); a write for an existing key replaces the values. At most K rows
per evidence id per pass; at most M rows per pass. Kind: state predicate
(current authoritative table state). Validity: pass commit. Settled (R4) with
the key choice proposed (see IDENTITY).

R-D2 Rebuildable. For a fixed Kernel snapshot, fixed scorer contract version,
and no budget exhaustion, truncating the table and running C-REBUILD over the
project's live descriptors yields the same set of (key, decision_id, score,
matched_atoms). Requires that the pass's descriptor text is the evidence
object's exact bytes, so a rebuild can read them back. Kind: state predicate
over a derivation. Validity: rebuild end. Settled (R4, U3 test 3) with the
text-source obligation added.

R-E1 Sweep drops only dead targets. A row is dropped when, at the sweep's
Kernel snapshot, its decision object is invalidated, or its current disposition
is `Superseded` or `Contradicted` (`admission.rs:4146-4147, 4157`), or its
descriptor object or evidence row is invalidated; every other row is kept.
Kind: transition safety (what a sweep step may remove). Observation: derived
observation of the Kernel at a snapshot. Validity: sweep commit. Settled (R4)
with the eligibility predicate shared with the pass (OQ-B).

R-E2 Sweep bounded and idempotent, Kernel untouched. One page per call; a
repeat at the same Kernel tip drops zero rows; the sweep issues no Kernel write;
a Kernel that is not readable yields zero drops and an `unavailable` result.
Kind: state predicate plus progress bound. Validity: sweep return. Proposed.

R-F1 Read serves only live-decision rows, ids and scores only. The read is
bound to the route's project; it checks the decision's standing in the Kernel
at read time; a decision that is not eligible answers a terminal, never rows;
rows are a bounded page ordered (score desc, created desc, evidence id asc);
fields are ids, numbers, and times only. Kind: state predicate on the answer
(current authoritative Kernel state plus derived table). Validity: response.
Settled (R6) with the live check and no-text rule proposed.

R-F2 Unavailable is distinct from empty. Flag off -> `disabled`; memory-store
unreadable -> `store_unavailable`; Kernel not Ready -> the kernel routes' state;
no rows for an eligible decision -> an empty page. Kind: state predicate on the
answer. Validity: response. Settled by the `review.*` family contract
(`host-wire-protocol.md:790-794`).

R-G1 Flag off means no pass, no write, no table read. With the route's frozen
`contradiction_candidates.enabled` false, no pass is offered, no row is written,
and neither read surface touches the table. Default false; enabled only by
explicit config. Kind: state predicate (`always(!X)`) over mutable
configuration frozen at route bind. Validity: every pass offer and every read.
Settled (R7, KTD4).

R-H1 Confirm linkage. A row's evidence object id is exactly the
`Published.evidence_object_id` P4 `dispute` accepts as `evidence_id`, and the
row's decision object id is exactly the `object_id` `dispute` takes. The list
does not check evidence liveness; a confirm whose evidence has been retired is
refused `not_found` by P4 C6 at commit. The confirm path never reads the
candidate table. Kind: state predicate (intrinsic to the row) plus a
transition rule on the confirm path owned by P4. Validity: row write; confirm
commit. Settled (R6, P4 C6/C7).

R-R5 No authority effects. Across any pass or sweep, the Kernel commit log and
admission rows are unchanged, and every `CausalInputs` this feature could
touch has empty `signals`. Kind: transition safety (`always(!X)`). Validity:
pass and sweep end. Settled (R5, stop conditions).

```text
IDENTITY AND VALUES
```

Entity identity: NOT EARNED. The domain does not track one candidate "through
change". A row is fully determined by its key and the derivation inputs; a
re-score replaces it; a sweep drops it; a rebuild recreates an equal row. No
merge, split, alias, or resurrection policy is needed beyond "the same key may
be written again". Row address and `created_at_ms` are not identity.

Key choice (proposed, resolves the S-D question): key = (project digest,
evidence object id, decision object id). Rationale: these are the three
identifiers the confirm path and the project binding need; `decision_id`
(the version) is a value, so a decision revision changes the stored
`decision_id` and score under the same key instead of accumulating one row
per version. Per-publish-tip identity is rejected: it would let one
descriptor-decision pair accumulate rows across replayed publications
(`Published.replayed`, `harness_sources.rs:530`) with no consumer that
distinguishes them.

Value semantics: EARNED for the score tuple. (score, matched_atoms) is
meaning-by-attributes under the scorer contract version; equality is exact
on the stored representation; the version is part of the meaning (two scores
from different contract versions are not comparable). Representation of
score (integer basis points vs float) is routed to the representation skill;
the obligation here is exact, deterministic equality.

Value semantics: EARNED for the folded atom set. Equality after F; order and
repetition handled by the scoring formula (OQ5-3). This value never leaves the
daemon (F10).

Named pure operation: EARNED. `score(descriptor_bytes, candidates, bounds,
threshold, K, version) -> ordered candidates` is a significant process with
no natural state owner. It is a function, not a service class.

Atomic consistency boundary: EARNED, small. Members: the rows for one
descriptor within one pass. Rule: R-C2 (a descriptor's rows are present
together or absent together) and R-D1's K cap (K is only checkable over the
descriptor's complete row set). The pass-level M cap is a sequential counter
across descriptors, not a joint-commit rule. Aggregate label: not applied;
there is no root through which commands enter.

Repository access role: NOT EARNED. Callers need "rows for decision O in
project P, paged" and "upsert/drop by key". Those are table operations.

```text
TRANSITIONS
```

NOT EARNED for rows. A row is present or absent; its values are replaced
wholesale. No history-dependent legality attaches to a row.

EARNED for the pass lifecycle, as an ordering rule rather than a status enum:

| Current state | Command / actor | Guard | Next state | Illegal |
| --- | --- | --- | --- | --- |
| Response handed to wire writer | `AfterResponse` closure | handoff accepted | Job running (slot held, gate taken inside) | pass started here |
| Job running | `CaptureJob::run` | - | Job terminal (`Count`), gate dropped | pass started under gate |
| Job terminal with `Published` list | daemon | flag on at the route; in-flight < P | Pass running (own budget) | pass with job budget |
| Job terminal, flag off | daemon | - | nothing | any table access |
| Pass running | budget exhausted / cancel | - | Pass ended, prefix written, exhaustion counted | rows for a half-scored descriptor |
| Pass running | completes | - | Pass ended, rows written, counts | > K or > M rows |

```text
PRESERVATION
```

Enforcement sets (proposed writer inventories; unverified until built).

E1 (R-D1 key, caps, R-C2 descriptor atomicity). Winning authority: the
memory-store table. Observed decision state: the descriptor's complete scored
candidate list, K, M, the running pass counter. Mechanism: one memory-store
write per descriptor (or per pass) through a `PreparedWrite` family with the
key as a uniqueness constraint; a descriptor's rows are written in one write.
Ordinary writers: C-PASS, C-REBUILD. Exceptional: C-SWEEP (delete only), test
fixtures, operator truncate. Bypass: direct SQL (operator), accepted as
rebuildable derived state. Commit point: the memory-store write commit.
Abort: nothing for that descriptor; later descriptors may still be written
(prefix). Unknown result: process crash between descriptors leaves a prefix,
which is a legal partial state under R-C2.

E2 (R-B1, R-B2 ordering and budget). Authority: the daemon runtime. Mechanism:
the pass is offered from the code that observes the job's terminal `Count`
and after the gate guard is dropped; the pass constructs its own `EvalBudget`
from the daemon cancel. Writers: only the one trigger site. Bypass that must
not exist: calling the scorer from `publish_locked` or passing `self.budget`.
Verification obligation: a test hook at `before_publish` /
`between_screen_and_commit` (`source_capture.rs:781, 839`) showing zero
candidate rows exist while the gate is held.

E3 (R-E1, R-E2 sweep). Authority for eligibility: the Kernel at the sweep's
snapshot. Mechanism: read standing for the page's distinct decision object
ids and descriptor/evidence ids under a budget, then one memory-store delete
of exactly the rows judged dead, keyed. Writers: C-SWEEP only. Race: a pass
writing a row for a decision that died between the pass's `tip` read and its
write; tolerated because the read surface (R-F1) re-checks standing and the
next sweep drops the row.

E4 (R-G1 flag). Authority: the route's frozen consent snapshot. Mechanism: the
trigger site and both read handlers consult the frozen flag before any table
access. Writers: config load, route bind. Bypass that must not exist: a read
handler that opens the table before checking the flag.

E5 (R-R5 no authority). Mechanism: the pass and sweep hold only read handles
to the Kernel; the memory-store write families they use carry no Kernel
write; every `CausalInputs` constructor site keeps `Vec::new()`
(`selection.rs:314`, `handoff.rs:374`, `lifecycle.rs:521`). Verification: the
Kernel commit log row count is equal before and after a pass and a sweep.

E6 (R-F1 live check, no text). Mechanism: the read handler performs a Kernel
standing read for the one decision before reading the table, then selects
rows by (project, decision object id) with a page bound, projecting only id,
number, and time columns. Bypass that must not exist: a tool result composer
that fetches evidence bytes to "help" the agent.

Closure test answers (all proposed): ordinary writers reach the table only
through E1 and E3; operator SQL is an accepted bypass because the state is
derived; stale observation (E3 race) is tolerated by design and named;
external effects: none (no Kernel write, no model call); cancellation leaves a
legal prefix; progress: the pass is bounded by N, K, M, the budget, and P.

```text
COMMAND OUTCOMES
```

C-PASS (per descriptor; the pass aggregates):
- Definitely applied / success: rows for the descriptor committed; counts
  `descriptors_analyzed`, `candidates_written`. Caller action: none.
- Definitely not applied / domain rejection: descriptor oversize (R-A2),
  counted `descriptor_refused_oversize`; descriptor not `Published` (deferred,
  too large, refused). Caller action: none (stop).
- Definitely not applied / policy: flag off, in-flight bound full
  (`refused_busy`), shutting down. Caller action: none; no retry (derived
  state; the next publication or a rebuild recovers).
- Definitely not applied for the remainder / unavailable: Kernel not readable
  for the id or summary read (`kernel_unavailable`). Caller action: none; sweep
  and later passes are the recovery.
- Definitely not applied for the remainder / cancelled or deadline: budget
  exhausted or daemon cancel (`deadline`, `discarded`); prefix stands (R-C2).
  Caller action: none.
- Unknown: a memory-store write that fails after commit was requested. The
  prefix is a legal state either way; no reconciliation needed because a
  rebuild or re-publication is idempotent under the key. Caller action: count
  `store_write_failed`, stop.
- Defect (owned impossibility): more than K rows for a key prefix, or a row
  for a half-scored descriptor. Effect certainty: separate; the row set is
  wrong and the sweep does not repair it; a rebuild does.

C-SWEEP:
- Definitely applied: n rows dropped, m examined; a repeat at the same tip
  drops 0. Caller action: call again while the page was full.
- Definitely not applied / unavailable: Kernel not readable; memory-store not
  writable. Caller action: retry on the next schedule.
- Unknown: write failure after the delete was issued. Safe: a repeat is
  idempotent. Caller action: retry on the next schedule.

Q-OWNER / Q-TOOL (queries; no state change; they are not commands):
- Terminal `disabled` (flag off, memory-store absent, authority not MODULE).
- Terminal `store_unavailable`.
- Kernel state per the kernel routes when the standing read fails.
- Terminal `not_live` (proposed name; the decision is not eligible at read).
- Page `{items: [...], next}`; empty items = no candidates recorded for an
  eligible decision.
Caller action for the agent: never infer "no contradiction" from an empty
page; never treat a terminal as zero.

```text
QUERY CONTRACTS
```

Q-OWNER and Q-TOOL share one contract:
- Question: which published sources overlap decision O of the bound project,
  and with what evidence id can one be confirmed.
- Source: the memory-store table (rows) plus the Kernel (decision standing at
  read). Winning authority on standing: the Kernel. Winning authority on
  "there was an overlap": none; the row is advisory.
- Provenance: each row carries `publish_tip` and `created_at_ms`; the answer
  may carry the standing read's `known_as_of`.
- Consistency: the table read and the standing read are two snapshots; the
  standing read is taken first; a row written between them may be missing,
  which is a legal omission.
- Completeness: paged, bounded (proposal: 1..64 like `review.list`), ordered
  (score desc, created desc, evidence id asc), `next` as in `review.list`.
  Rows whose evidence has since been retired are included (R-H1).
- Result meaning: empty page = no recorded candidate for an eligible decision;
  terminal = no answer. Stale: the score reflects the decision version
  `decision_id` on the row, which may differ from the current version.
- Consumers: owner, the agent. Prohibited decisions: admitting or skipping any
  Kernel command, changing any disposition, selecting reviewer work.
- Incidental effects: none (no cache fill, no access touch). Telemetry counts
  only.
- Failure behavior: fail closed to a terminal; never fall back to "empty".

The pass's own reads (Kernel id read and summary read) are internal queries
with this contract: snapshot `tip` chosen once per pass; completeness
"at most N, truncation counted"; the `ScopeFilter` judge applied after the
SQL prefilter; refusal on `FutureSnapshot`, `Deadline`, or a corrupt row ends
the pass as unavailable for the remainder.

```text
PERSISTENCE AND CONCURRENCY
```

Semantic requirements handed to the persistence and topology skills:
- The rows of one descriptor share one outcome (R-C2). Partial rows for a
  descriptor are forbidden. A prefix of descriptors is permitted.
- The key is unique; a second write replaces values (upsert semantics by
  meaning; mechanism is the persistence skill's).
- Writers: C-PASS and C-REBUILD write; C-SWEEP deletes; no other ordinary
  writer. Operator truncate is an accepted bypass of a derived table.
- Concurrency: a pass and a sweep may interleave. Required outcome: the sweep
  deletes only rows it judged; a row inserted after the sweep's judgment
  survives to the next sweep. No row may be half-updated.
- Two passes for the same project: prevented by the in-flight bound P=1 per
  project (proposal), or tolerated because both are upserts from snapshots;
  the later `tip` should win on conflict. Choose and verify in topology.
- Retry: none inside the pass. Recovery is the next publication, the sweep,
  or a rebuild. No retry storm is possible because the trigger is one offer
  per job terminal.
- Progress: bounded by N (ids), summary bytes bound, K, M, the budget, and P.
  Falsifier of progress: a project with more than N eligible decisions where
  the id read itself is unbounded (OQ5-1); the heap bound caps memory but the
  SQL walk is proportional to the project's `memory` decisions. State this as
  the one unbounded-in-project-size read and decide whether a budgeted
  Kernel variant is required.

```text
DERIVED STATE
```

Ledger for `contradiction_candidates`:
- Derivative and purpose: discovery/advisory list of descriptor-decision
  overlaps for inspection and manual confirm.
- Base authority: Kernel decisions (domain `memory`, project scope) and
  descriptors/evidence (domain `harness-capture`). Disagreement rule: the
  Kernel wins; a row contradicted by Kernel state is stale, never authoritative.
- Derivation: `score` under scorer contract version V (embeds
  `ANALYSIS_CONTRACT_EPOCH`, F, formula, threshold, K); inputs are the evidence
  object's exact bytes and `DecisionRow.payload.summary`
  (`slice/read.rs:19`, `slice/mod.rs:25-28`). Deterministic.
- Provenance: `publish_tip` (Kernel snapshot of the pass), `decision_id`
  (version scored), `created_at_ms`. Recommended: store V on the row or in a
  table-level marker so a version change can drop and rebuild.
- Refresh: on publication (pass), on sweep (deletion), on rebuild. No refresh
  on decision revision; the row keeps the old `decision_id` until a later
  pass for a new descriptor or a rebuild.
- Visible freshness: bounded by the sweep interval for deletions; unknown lag
  for revisions. Readers re-check decision standing (R-F1).
- Completeness: partial by design (N truncation, K, M, budget prefix, flag
  history). Never complete.
- Allowed consumers: owner read, agent `candidates` action.
- Prohibited decisions: any Kernel admission, any disposition, any reviewer
  selection, any `signals`, any authorization.
- Decision authority: none. The confirm path re-reads the Kernel (P4 C4, C6,
  C7) and never consults this table.
- Mismatch detection: the read's standing check; the sweep's standing check;
  a rebuild comparison in tests. Latency: next read or next sweep.
- Mismatch action: drop (sweep) or withhold (read terminal `not_live`).
- Rebuild/cutover: truncate; rebuild from live descriptors at one Kernel
  snapshot; readers can read during rebuild (rows appear per descriptor).
  Mixed old/new is acceptable because rows are advisory. Rollback: truncate.
- Failure/absence meaning: absent row = unscored, refused, truncated, flag
  was off, or swept. Never "no overlap".
- Retirement: drop the table in a later baseline rotation; no compatibility
  horizon because nothing authoritative references rows.

```text
STRUCTURE INPUTS
```

Hidden decisions (inputs to topology, not module decisions):
- H1 the scoring formula, threshold, F, K, M, N (index D7 scaffolding;
  OQ5-3). Protected clients: the pass, the rebuild, tests. Contract to
  preserve: R-A1..A3 and the version tag.
- H2 the eligibility predicate for a `memory` decision (shared by pass, sweep,
  read). Protected clients: all three. Contract: one definition (OQ-B).
- H3 the descriptor text source (carried from the job vs read back from the
  evidence object). Contract: bytes equal the evidence object's bytes.
- H4 the trigger site and the in-flight bound.

Labelled edges the model requires:
- pass --reads--> Kernel (snapshot `tip`); pass --writes--> table;
  sweep --reads--> Kernel; sweep --deletes--> table; read --reads--> Kernel
  (standing) and table; agent --commands--> `kernel.commit` (P4) with ids
  copied from a row; the Kernel retention sweep --invalidates--> evidence
  (external to this feature).
- No edge from the table to any Kernel command, reviewer job, or admission.

```text
VARIATION / ASSEMBLY
```

NOT EARNED as a variation point. One scorer, one store, one flag. The flag is
a lifecycle/consent input (route-frozen), not a strategy selection. Version V
is a compatibility tag for rebuild, not a plug-in seam.

```text
FALSIFIERS
```

(Smallest violating trace per rule; model-relative, not proof.)
- R-A1: the same inputs with the candidate list in a different order yield a
  different top-K order or a different tie winner.
- R-A2: a 65 KiB descriptor yields rows computed from its first 64 KiB.
- R-A3: "Foo" vs "foo" scores below 1.0; or F applied to the descriptor only.
- R-B1: a candidate row exists while the `between_screen_and_commit` hook is
  running (gate held).
- R-B2: the pass is constructed with the job's `budget`, or `AfterResponse`
  awaits the pass before returning.
- R-B3: 1,000 rapid publications create 1,000 concurrent passes.
- R-C1: a project with 100,000 `memory` decisions makes the pass load 100,000
  summaries.
- R-C2: a deadline inside descriptor 3 of 5 leaves 2 rows of descriptor 3's
  top-5.
- R-D1: two rows with the same (project, evidence, object id); K+1 rows for
  one evidence id from one pass.
- R-D2: truncate + rebuild at the same snapshot yields a different row set
  because the pass used carried text that differed from the evidence bytes
  (for example a redacted or normalized copy).
- R-E1: a sweep drops a row whose decision is `Disputed` and live; or keeps a
  row whose decision is `Superseded`.
- R-E2: a sweep with the Kernel unavailable drops every row; or a sweep writes
  a Kernel event.
- R-F1: the read answers rows for a `Superseded` decision; or a row includes
  `summary` or `matched_atoms_text`.
- R-F2: Kernel Starting answers `{items: []}`.
- R-G1: flag off, a `candidates` call opens the table and answers an empty
  page instead of `disabled`.
- R-H1: a row carries the descriptor `object_id` where `evidence_id` is
  expected and the confirm is refused `not_found` for a live source; or the
  list refuses rows with retired evidence (hiding a refusal the confirm must
  own).
- R-R5: a non-empty `signals` vector constructed from candidate rows; or a
  commit-log row attributed to the pass.

Analysis bounds: single daemon, one memory-store, one Kernel; no replication;
operator SQL excluded; writer inventory proposed, not verified.

```text
HANDOFF
```

- Persistence (`/software-design:persistence-boundary-and-transaction-design`):
  key uniqueness, per-descriptor write grouping, upsert semantics, sweep
  delete-by-judged-keys, `PreparedWrite` family, baseline DDL in Batch M1.
- Topology (`/software-design:in-process-concurrency-topology-design`): the
  trigger site after `CaptureJob::run`, the in-flight bound P, cancel wiring,
  whether the pass runs on the blocking pool.
- API (`/software-architecture:api-design-and-evolution`): the owner read's
  method literal in section 7.8, the `candidates` action body, terminal names
  (`not_live` is a proposal), page bounds.
- Representation (`/software-design:representation-and-model-boundary-design`):
  score encoding, version tag placement, time units.
- Rust design review: scorer module API, `LexicalBounds` constants, F.
- Tests (`/testing:invariant-test-review` after landing): the 15 proposed
  records in `idm-proposed-records.md`.
- Unknowns: OQ-A (F definition and FTS5 diacritic divergence), OQ-B (one
  eligibility predicate: which dispositions are candidates and swept), OQ-C
  (text source H3), OQ-D (N and whether a budgeted Kernel id read is needed:
  OQ5-1), OQ-E (whether the `candidates` action is hidden from the tool
  schema when the flag is off), OQ5-2, OQ5-3.

---

## Conflicts between the plan and HEAD 3a3b916b7

1. Summary source. R2 and KTD-era OQ decision say summaries come through
   `claim_facts_at_within_budget`. At HEAD `ClaimFacts` carries ids, admission
   facts, served standing, occurrences, and causality (`claim_facts.rs:146-156`,
   `ClaimDecisionFacts` `:90-97`) and no payload text. Summary text is
   `DecisionRow.payload.summary` (`slice/read.rs:11-22`), reachable through
   `decisions_for_objects_as_of_within_budget` (`:236-241`) after
   `decision_payload_sizes_as_of` (`:330`), or through
   `decision_page_within_budget` (`:126-133`). `claim_facts` remains useful only
   if its `ServedStanding` is preferred over `VisibleRow.visibility`
   (`admission.rs:193-200`) for eligibility; the plan's `TooManyClaims` bound
   then does not bound the summary read and R-C1 must bound it another way.
2. Unbounded id read. `visible_domain_decisions_as_of_in_scope` has no budget
   and no row limit (`admission.rs:3087-3098`). The plan's "bounded work per
   pass regardless of project size" is satisfiable only by a caller-side heap
   (memory) plus either a budgeted Kernel variant or acceptance that the SQL
   walk scales with the project's `memory` decisions (OQ5-1, OQ-D).
3. "Flag off: no behavior change anywhere" (Definition of Done) is not true at
   the storage layer. U3's table is a memory-store baseline change; the digest
   rotation closes the MemoryReviewer activation gate for every deployment
   until renewed (`docs/memory-reviewer-operations.md:34-43`), flag or no flag.
   Index D4 accepts this by batching. The DoD should say "no runtime behavior
   change". Also the TS tool schema gains the `candidates` action regardless of
   the daemon flag unless the plugin hides it (OQ-E).
4. Trigger location. The `Published` list (with `evidence_object_id`) and the
   unit text exist only inside `publish_locked` under the capture commit gate
   (`source_capture.rs:789-800, 826-840`), and `publish_screened_between`
   consumes the `SourceUnit` vector. The plan's "on publish settlement"
   requires the job to carry (evidence id, text or a handle to it) out past the
   gate and the terminal, or the pass to read evidence bytes back from the
   Kernel. Either choice is new plumbing the plan does not list under U2's
   files.
5. Line drift: the plan cites `lib.rs:9145-9158` for `AfterResponse`; at HEAD
   it is `:9157-9176` with the same semantics. The plan cites
   `crates/daemon/Cargo.toml:41` for the retrieval dependency; context records
   `:45`.
6. Eligibility predicate. The plan says "live" for the candidate set and
   "superseded, contradicted, or retired" for the sweep. `Disposition` also has
   `Stale`, `Disputed`, `Rejected`, `Quarantined` (`admission.rs:97-105`).
   `Rejected` and `Quarantined` decisions should not be candidates; the plan is
   silent. One shared predicate is required (OQ-B, needs human input on
   `Stale`/`Quarantined`).
7. Dispositions do not "retire" a decision object; retirement is
   `invalidated_commit_seq` on the registry row (`schema.rs:93`). The plan's
   "retired" and "superseded" are distinct Kernel facts; the sweep must check
   both.
