# Hot-path optimization preservation supplement

## Scope and provenance

This supplement supplies reusable `/property-discovery-and-catalog` input for
a specification. It does not authorize implementation or create tickets.

- The system is `/local/home/ahrav/scratch/eidnara`.
- The baseline is `913234433ae36a80a6e22c6aac14c7f9aab74386`.
- This is a working-tree supplement against that source baseline, not an
  artifact contained in the baseline commit.
- The source verification date is 2026-09-10.
- The supplied audit identifies five separate prior read-only discovery
  surfaces. Their summaries are retained under `_lenses/`; they are not test
  evidence or additional independent corroboration.
- The supplied scope includes that audit and in-repository code, documents,
  tests, and history. External plans and incident reports were not supplied.
  The owner approved this five-area scope on 2026-09-10 and confirmed that no
  additional external plan or incident evidence needs to govern it. This does
  not assert that no external documents exist.
- The task reports five prior independent evaluations of relevant existing
  catalogs. Those evaluations are not a fresh evaluation of this supplement.
  The separate review, local dispositions, and independent recheck are recorded
  in [portfolio-evaluation.md](portfolio-evaluation.md). Applying corrections
  is not another independent evaluation.
- Historical source citations and exercise claims are not carried forward.
  New source anchors are checked against this HEAD. No tests, campaigns,
  benchmarks, or query-plan experiments run as part of this work.

The records constrain preservation across selection pushdown, execution
placement, callback batching, history-budget calculation, and prepared-field
ownership changes. They do not claim an optimization exists or is faster.
Generic lifecycle, fencing, snapshot, redaction, and tokenizer obligations
remain in their canonical catalogs below.

The five-area specification is
[Spec: behavior-preserving hot-path optimization][spec-five-area].
Its approval covers this catalog's K, E, S, H, and R groups, not additional
concurrent supplements. [The publication receipt][receipt] records exact-body
verification and the local artifact boundary. That specification is closed
and superseded by [Spec: Hot-path latency (HP1)][spec-hp1], which names this
catalog and its [`latency-audit`](latency-audit/catalog.md) area as the
authority for its constraints.

## Reachability and boundaries

| Records | Class | Evidence and limit |
| --- | --- | --- |
| K1-K4 | default-production | [The transform read][pass-read] uses [the reader][memory-read]; [memory is enabled by default][memory-default]. Pressure requires a large valid input, not a feature flag. |
| E1-E3 | default-production | [General routed dispatch][dispatch] and [route close][close] are production paths. This label covers their existing lifecycle only. |
| S1-S2 | default-production | [Read callbacks][read-callback] and [fenced writes][write-callback] serve [ordinary memory-store operations][prepared-execute]. |
| H1-H4 | default-production | [HARD composition][hard-compose] reaches [the history renderer][history-render] and [wrapped retry][outer-retry]. Pressure requires workload construction. |
| R1-R3 | default-production | [Core preparation][core-prep] and [transaction preparation][transaction-prep] use [prepared fields][prepare-field] on durable writes. |

FUTURE execution topology is unresolved. The transform runs synchronously
inside its async handler at this HEAD. The existing ingest reservations
illustrate ownership across blocking work; they do not prove an off-worker
transform lifecycle. Any proposed worker arrangement needs its own completion
and accounting evidence before claiming E1-E3 are exercised. Worker-specific
test readiness is **BLOCKED** pending that design; existing request reachability
is not evidence that an unbuilt worker is ready for implementation.

History records cover HARD/refold rendering. Ordinary SOFT keeps existing m0
but can replace m1 and other rendered units; pressure refold can rematerialize
m0. Pure Defer/SoftPlus replays its retained prefix unchanged. The
[core transitions][soft-core] and [daemon SOFT inputs][soft-inputs] establish
these distinct boundaries. The tokenizer catalog's blanket test-only label is stale:
[daemon/Cargo.toml:21-32][tokenizer-dependency] declares a normal dependency.
This supplement does not edit that catalog or import its reachability labels.

## Fixed reference identity

K1 and H1 use commit `913234433ae36a80a6e22c6aac14c7f9aab74386` as the
semantic reference, not the candidate implementation. K1 freezes
[read_visible][read-visible], [admission selection/folding][admission-reference],
and [canonical conversion][memory-read], including their ordering and caps.
H1 freezes [history rendering][history-render], [wrapped retry][outer-retry],
[tokenization][tokenizer-reference], and [cache behavior][cache-reference].
The JSON bodies and SHA expectations in [render-golden.json][render-json],
[render-tight-golden.json][tight-json], [decay-store-shape.json][shape-json], and
[decay-store-differential.json][differential-json] have that exact source commit.
Their recorded provenance is retained, not regenerated from the candidate.

Choosing a test-only adapter or separate reference executable is packaging
work for `/testing:test-strategy`, not an unresolved acceptance rule. A copied
candidate helper cannot serve as the independent oracle. Corrupt excluded-row
error behavior remains an owner gate before M1 selection pushdown; no error
suppression is approved. M1 and M5 below name the supplied optimization surfaces
(selection and prepared-field allocation), not tickets or prompt units.

## Index

| ID | Record | Type | Check |
| --- | --- | --- | --- |
| K1 | [canonical-memory-result-preserves-current-selection-pipeline][k1] | safety | always |
| K2 | [canonical-memory-irrelevant-candidates-do-not-consume-caps][k2] | safety | always |
| K3 | [canonical-memory-selection-pressure-is-exercised][k3] | reachability | sometimes |
| K4 | [canonical-memory-byte-pressure-is-exercised][k4] | reachability | sometimes |
| E1 | [route-cleanup-waits-for-request-owned-physical-work][e1] | safety | always |
| E2 | [request-work-accounting-covers-retained-resources][e2] | safety | always |
| E3 | [request-close-overlaps-live-work][e3] | reachability | sometimes |
| S1 | [guarded-callbacks-enforce-current-authority][s1] | safety | always |
| S2 | [callback-batching-preserves-observation-boundaries][s2] | safety | always |
| H1 | [history-budget-selection-preserves-reference-bytes][h1] | safety | always |
| H2 | [history-budget-boundaries-remain-distinct][h2] | safety | always |
| H3 | [history-budget-pressure-paths-are-exercised][h3] | reachability | sometimes |
| H4 | [history-outer-retry-pressure-is-exercised][h4] | reachability | sometimes |
| R1 | [prepared-field-output-and-audit-policy-agree][r1] | safety | always |
| R2 | [preparation-refusal-does-not-append-audit-state][r2] | safety | always |
| R3 | [redaction-audit-does-not-depend-on-retained-payload][r3] | safety | always |

The distribution is eleven `always` checks and five `sometimes` checks.
There is no new liveness claim with an invented deadline. A route-close budget
can lead to fatal refusal of cleanup, not proof that arbitrary work terminates.

## Canonical read

### canonical-memory-result-preserves-current-selection-pipeline

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No baseline-versus-candidate selection comparison runs.
Guarantee: Selection optimization preserves the canonical memory result for
the same valid store state, requested snapshot, project, and budget.
Check: `always` - Compare the ordered selected rows, rendered memory bytes,
revision, truncation flag, known-as-of value, and available/withheld outcome
with the [fixed baseline pipeline](#fixed-reference-identity); every evaluated
result must agree. The oracle must not reuse candidate selection helpers.
Fault/timing angle: A commit, sensitivity change, or scope change crosses the
read boundary while query filtering or materialization changes.
Required faults and enabling state: Construct admission and lineage histories,
uncertain scopes, tie-ordered rows, excluded categories, and payload pressure;
hold the baseline and candidate to the same governing snapshot and lag sample.
Confidence: high - [Evidence](evidence/canonical-memory-result-preserves-current-selection-pipeline.md).
The read, selection, trimming, and pass-level pinning are source-verified.
Existing check: [Canonical checks](existing-checks.md#canonical-read) cover
composition and budgets; their status is unaudited.
Impact: Memory bytes, revision-driven materialization, or withholding can drift.
Open questions:
- Must skipping a corrupt excluded row preserve the baseline error? This is
  an owner gate before M1 pushdown across decoding; suppression is not approved.
  (needs human input)

### canonical-memory-irrelevant-candidates-do-not-consume-caps

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No paired-store irrelevance comparison runs.
Guarantee: Rows rejected by the existing pre-cap selection cannot displace
eligible memory decisions or independently set canonical truncation.
Check: `always` - With relevant rows, scope/admission facts, and freshness held
equal, vary valid other-domain, non-decision, hidden, and out-of-project rows;
selected IDs, order, content, revision, and truncation must remain equal.
Compare snapshot metadata to each store's own pinned tip, not across tips.
Fault/timing angle: An early SQL limit or changed predicate admits noise into
the bounded set or removes candidates the baseline keeps.
Required faults and enabling state: Seed unrelated rows ahead of relevant rows
in both lexical and serving rank, including enough rows or bytes for pressure.
Positive-category and Labeled filters remain after caps; they are not noise
eligible for this equivalence.
Confidence: high - [Evidence](evidence/canonical-memory-irrelevant-candidates-do-not-consume-caps.md).
The pre-cap domain/kind and scope predicates are source-verified.
Existing check: [Canonical checks](existing-checks.md#canonical-read) include
generic row and byte caps; canonical noise pressure remains unaudited.
Impact: Busy unrelated domains can erase usable project memory.
Open questions:
- How will paired fixtures isolate unrelated rows from shared lineage and
  sensitivity facts? The fixture must not change legitimate authorization.

### canonical-memory-selection-pressure-is-exercised

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - No independent pressure witness is recorded.
Guarantee: A preservation campaign constructs row pressure independently of
byte pressure to distinguish pre-cap filtering from premature row limiting.
Check: `sometimes` - More than 8192 admitted visible candidates place an
eligible memory target beyond rank 8192 in unfiltered serving order
(`created_commit_seq DESC, object_id ASC`), but within the eligible cap; the
unfiltered payload total stays below 8 MiB, so bytes cannot mask row pressure.
This asserts seeded preconditions, not the candidate's output or truncation.
Fault/timing angle: Small fixtures conceal cap-order regressions.
Required faults and enabling state: Record counts, serving rank, small stored
payload lengths, and eligibility independently. Excluded rows precede the target.
K4 separately requires byte pressure; neither marker substitutes for the other.
Confidence: high - [Evidence](evidence/canonical-memory-selection-pressure-is-exercised.md).
The 8192-row cap and payload-prefix cutoff are source-verified.
Existing check: [Generic cap tests](existing-checks.md#canonical-read) exist;
their status is unaudited and no new witness is carried from them.
Impact: A green comparison may never challenge the optimization's boundary.
Open questions: None.

### canonical-memory-byte-pressure-is-exercised

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - No independent byte-pressure witness is recorded.
Guarantee: A preservation campaign constructs byte pressure independently of
row pressure to distinguish pre-cap filtering from premature payload limiting.
Check: `sometimes` - At most 8192 admitted visible candidates place excluded
decision payloads totaling more than 8 MiB before an eligible target in serving
order, while the relevant retained prefix including the target fits 8 MiB;
independent seed measurements establish this without requiring output truncation.
Fault/timing angle: Small payloads conceal incorrect byte-limit placement.
Required faults and enabling state: Use valid other-domain decisions with
bounded fields; measure stored `decision_payload` BLOB bytes, not summary
characters or the full serialized response. The row count cannot exceed its
cap. Record response JSON bytes separately if checking serialization limits.
Confidence: high - [Evidence](evidence/canonical-memory-byte-pressure-is-exercised.md).
The payload-size query, prefix cutoff, and separate row cap are verified.
Existing check: [Generic byte-cap test](existing-checks.md#canonical-read) checks
a nonempty truncated newest prefix; its status is unaudited.
Impact: Row-only pressure can leave a byte-limiting regression unchallenged.
Open questions: None.

## Execution lifecycle

### route-cleanup-waits-for-request-owned-physical-work

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No physical-work completion/cleanup trace is collected.
Guarantee: The current callback completion gate precedes route cleanup and
reuse, and execution relocation must extend that gate to owned physical work.
Check: `always` - At route-gone entry and cleanup-gated reuse, an independent
ledger contains no live request-owned work that can access that route's state;
an unquiesced timeout follows the fatal/refusal path instead of cleanup.
Fault/timing angle: Cancellation or outer-future drop precedes actual worker
completion, including a worker that has not observed abort.
Required faults and enabling state: Hold request work at an observable live
barrier, start route or generation close, and independently observe completion,
route-gone, and reuse. Classify durable outcomes through existing CAS/receipts.
Confidence: medium - [Evidence](evidence/route-cleanup-waits-for-request-owned-physical-work.md).
The current handler completion fence is verified; worker-specific test readiness
is BLOCKED until ownership and completion design are supplied.
Existing check: [Lifecycle checks](existing-checks.md#execution-lifecycle) cover
settlement and cleanup cases; their status is unaudited.
Impact: Cleanup can race live work or permit stale work to affect reused state.
Open questions:
- What owns and joins any proposed off-worker transform work? FUTURE topology
  remains unresolved. (needs human input)

### request-work-accounting-covers-retained-resources

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No resource-class lifetime ledger is compared.
Guarantee: Execution-placement optimization retains each resource charge until
the resource covered by that charge is released or explicitly transferred.
Check: `always` - For each resource class `c`, assert
`observed_live_units[c] <= reserved_units[c] <= configured_capacity[c]` using
the [current resource ledger](evidence/request-work-accounting-covers-retained-resources.md#evidence-trail);
each reservation returns once or transfers ownership without double release.
Pending permits cover settlement; physical-work and byte charges cover their
own lifetimes, so equality between these counters is not required.
Fault/timing angle: Cancellation drops a waiter while work or its result still
retains input, decoded bytes, staging bytes, or a task slot.
Required faults and enabling state: Exercise normal completion, refusal,
cancelled waiting, queued work, and retry ownership transfer with live payloads.
Confidence: high - [Evidence](evidence/request-work-accounting-covers-retained-resources.md).
Separate host permits and ingest reservation ownership are source-verified.
Existing check: [Accounting checks](existing-checks.md#execution-lifecycle)
exercise caps and reservation release; their status is unaudited.
Impact: Early release undercounts work; missed release strands capacity.
Open questions:
- Which existing or explicit new capacity class covers proposed physical work,
  and what exact bytes does it charge? (needs human input)

### request-close-overlaps-live-work

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - No close/live-work overlap witness is recorded.
Guarantee: A lifecycle-preservation campaign initiates close during a live
current callback and extends this witness to physical work if execution moves.
Check: `sometimes` - For the same route identity, observe work start, then close
initiation before the held work's completion signal; the predicate does not
require premature cleanup, a stale write, or a duplicate terminal.
Fault/timing angle: A close after all work finishes cannot expose ownership loss.
Required faults and enabling state: Use a live-work barrier and separate close
and completion observations; include cancellation and route retirement schedules.
Confidence: medium - [Evidence](evidence/request-close-overlaps-live-work.md).
The existing general request lifecycle is reachable; worker-specific test
readiness is BLOCKED pending execution topology and completion observations.
Existing check: [Lifecycle checks](existing-checks.md#execution-lifecycle) are
unaudited; no transform-worker overlap check is identified in this scope.
Impact: Completion-gate checks can pass without any vulnerable overlap.
Open questions:
- Which physical start/completion events can a future worker expose without
  equating waiter cancellation with completion? (needs human input)

## Guarded store

### guarded-callbacks-enforce-current-authority

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The [storage denial matrix and statement-reuse
probe](evidence/guarded-callbacks-enforce-current-authority.md#mode-gate-evidence)
run in `cargo test -p storage`: a warm fenced statement is not re-prepared
across two callbacks; a foreign `CREATE TABLE` forces a re-prepare, and a
temp-shadow statement cached before it is refused afterwards; a statement
prepared under the unrestricted mode is refused in a guarded callback once the
cache is flushed; a panicking read or fenced callback returns the connection to
the unrestricted mode and rolls its partial write back; and baseline text with
a pragma write, `ATTACH`, `BEGIN`, `SAVEPOINT`, fence-row insert, or
format-marker delete is refused by the store connection's gate. No
baseline-versus-candidate trace over interleaved facade callers runs.
Guarantee: Cached statements and reduced callback setup preserve each call's
current read/write, schema, fencing, and applicable facade authority.
Check: `always` - Across identical authority-transition traces, baseline and
candidate agree on accepted results, refusals, durable effects, and restored
connection state; facade effects use the current caller/domain/route scope on
the executing thread, while nonfacade calls may legitimately have no scope.
Fault/timing angle: Maintenance, temp shadows, rename, stale statements, errors,
or panic separate authorization from execution.
Required faults and enabling state: Alternate maintenance/read/fenced calls,
reuse SQL after authority changes, and interleave facade callers while the
connection is contended; inspect effects after failure and after unwind.
Confidence: high - [Evidence](evidence/guarded-callbacks-enforce-current-authority.md).
Callback installation, release, shadow checks, and facade scope sites are read.
Existing check: [Guarded-store checks](existing-checks.md#guarded-store) are
unaudited and cover cached statements, shadows, and restoration.
Impact: Setup elision can authorize stale privileges or target shadow objects.
Open questions:
- The read path's `query_only` toggle expires every cached statement on the
  connection, and it is the read callback's only write barrier for main and
  temp alike; `deny_scope_escapes` allows DML on every non-infrastructure
  table. What replaces it as the read path's write barrier if the toggle is
  dropped? Owner: the connection-open unit (#430). (needs human input)

### callback-batching-preserves-observation-boundaries

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The mode-gated authorizer introduces no batching; the
[snapshot, freshness, and rollback checks](evidence/callback-batching-preserves-observation-boundaries.md#mode-gate-evidence)
pass unchanged against it. No batched-versus-baseline observation trace runs.
Guarantee: Callback batching preserves existing snapshot freshness and atomic
write boundaries rather than merging unrelated operations into one snapshot.
Check: `always` - Under the same ordered writer schedule, reads within an
existing read callback retain one snapshot, a later independent callback sees
intervening committed data, and each existing fenced operation commits or rolls
back its original effect set without absorbing a neighboring operation.
Fault/timing angle: An external commit or callback error falls between operations
that an optimization tries to combine.
Required faults and enabling state: Commit through a second connection between
reads, then between callbacks; fail one fenced operation beside a successful
independent operation and compare values and durable effect sets.
Confidence: high - [Evidence](evidence/callback-batching-preserves-observation-boundaries.md).
Deferred read snapshots, immediate fenced writes, and the freshness test are read.
Existing check: [Snapshot and rollback checks](existing-checks.md#guarded-store)
are unaudited.
Impact: A batching optimization can serve stale decisions or enlarge rollback.
Open questions:
- Which reads belong to one logical observation in the proposed batching plan?
  Existing independent boundaries cannot be silently removed. (needs human input)

## History rendering

### history-budget-selection-preserves-reference-bytes

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No candidate renderer is compared with a frozen reference.
Guarantee: History-budget optimization selects exactly the baseline's final
history bytes for fixed ordered inputs, budget, and estimator semantics.
Check: `always` - Compare final UTF-8 bytes with the [fixed baseline](#fixed-reference-identity)
oldest-first reference and its JSON/SHA goldens, using the actual tokenizer
for reference decisions and final cost; cache-backed counts must equal direct
counts for the exact joined text at each observed comparison.
Fault/timing angle: Estimates, cached fragments, or batched demotions choose a
different stopping point at a join seam or equal-tier representation.
Required faults and enabling state: Use Unicode, escaped headings, empty tiers,
legacy rows, equal adjacent tier bodies, and tier-5 removal in input order.
Internal estimates and batched steps are allowed if final bytes agree.
Confidence: high - [Evidence](evidence/history-budget-selection-preserves-reference-bytes.md).
The loop, join separator, real-tokenizer goldens, and cache adapter are verified.
Existing check: [History checks](existing-checks.md#history-render) compare
reference bytes and counts; their status is unaudited.
Impact: Different surviving history changes prompt content and frozen bytes.
Open questions: None.

### history-budget-boundaries-remain-distinct

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No inner/outer boundary comparison runs.
Guarantee: Optimization preserves the distinct inner body guard, outer wrapped
history retry policy, and frozen replay boundary.
Check: `always` - With the actual estimator and representable compartment/guard
sizes, finite positive inner budgets produce bodies whose cost fits; direct-API
nonpositive budgets disable that guard. Outer results match the reference's
wrapped-slice 105% threshold and at-most-three rerenders, even if still over
budget. Ordinary SOFT preserves m0 but may replace m1/rendered units; pressure
refold may rematerialize m0. Pure Defer/SoftPlus preserves the retained prefix.
These checks do not impose a whole-prompt token limit.
Fault/timing angle: A shared budget helper conflates body, wrapper, request
validation, or replay semantics.
Required faults and enabling state: Use positive, zero, and negative direct-API
budgets; use wrapper-dominated small budgets, ordinary SOFT with changed m1,
pressure refold, and pure Defer/SoftPlus after HARD. H4 supplies outer pressure.
Confidence: high - [Evidence](evidence/history-budget-boundaries-remain-distinct.md).
The inner guard and bounded outer retry are source-verified separately.
Existing check: [History checks](existing-checks.md#history-render) are
unaudited; a complete outer-boundary comparison is not identified.
Impact: Valid direct calls can change behavior or replay can be recomputed.
Open questions:
- Will request validation remain separate from the direct renderer contract?
  No broader handling of nonfinite inputs is specified here. (needs human input)

### history-budget-pressure-paths-are-exercised

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - No independent multi-demotion witness is recorded.
Guarantee: A history-preservation campaign supplies input for which the baseline
oldest-first guard needs more than one demotion.
Check: `sometimes` - The independent reference's initial joined body and body
after its first legal demotion both exceed the positive budget, and another
demotion remains possible; this witnesses pressure without demanding a defect
or requiring the optimized renderer to execute separate iterations.
Fault/timing angle: Generous budgets or single-step fixtures hide stopping errors.
Required faults and enabling state: Construct ordered, demotable compartments
and record actual whole-body costs before candidate execution.
Confidence: high - [Evidence](evidence/history-budget-pressure-paths-are-exercised.md).
The oldest-first loop and the limitations of existing pressure markers are read.
Existing check: [Tight history goldens](existing-checks.md#history-render) are
unaudited and do not supply this campaign's independent witness.
Impact: A parity suite may never test the optimization's repeated-pressure case.
Open questions:
- Which fixtures independently certify multiple reference demotions with the
  production estimator, including repeated equal-tier bytes?

### history-outer-retry-pressure-is-exercised

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - No independent wrapped-retry witness is recorded.
Guarantee: A history-preservation campaign reaches the outer wrapped-slice retry
condition independently of the inner body-demotion witness.
Check: `sometimes` - With finite positive budget `B`, the baseline's initial
wrapped history slice has actual token cost greater than `1.05 * B`, so its
reference retry predicate is true; this witnesses setup, not a return failure.
Fault/timing angle: Inner body fit can hide wrapper-driven rerender pressure.
Required faults and enabling state: Record the initial reference slice and
cost. The boundary matrix also includes three-attempt exhaustion, zero-budget
bypass, and threshold equality; an empty history with budget 0.5 retains the
nonempty wrapper at every attempt and is a source-derived exhaustion constructor.
Confidence: high - [Evidence](evidence/history-outer-retry-pressure-is-exercised.md).
The wrapper placeholder and three-attempt retry limit are source-verified.
Existing check: [Outer retry guard](existing-checks.md#history-render) is
unaudited; no independent outer-pressure campaign marker is identified.
Impact: Optimizing inner selection can leave the outer retry contract untested.
Open questions: None.

## Redaction ownership

### prepared-field-output-and-audit-policy-agree

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No six-policy comparison runs.
Guarantee: Prepared-field ownership changes preserve output and audit policy
across Content, NewIdentity, and ExistingIdentity in both scan layers.
Check: `always` - For each policy/layer pair, compare output or refusal and
normalized scan metadata with the baseline: Content substitutes, ExistingIdentity
preserves input with preserve actions, and detected NewIdentity refuses before
append; an applied clean NewIdentity records a zero-finding `field_scans` row
and owner copies, but no `scan_detections` row.
Fault/timing angle: Moving or draining redacted text drops detections, changes
identity bytes, or assigns the wrong persisted action.
Required faults and enabling state: Cover clean and detected input in Durable
and Transaction layers, with field IDs and multiple owners recorded independently.
Confidence: high - [Evidence](evidence/prepared-field-output-and-audit-policy-agree.md).
The policy dispatch, output choice, and receipt writer are source-verified.
Existing check: [Redaction checks](existing-checks.md#redaction-ownership) cover
preserve/substitute actions; their status is unaudited.
Impact: Stored values and audit claims can disagree despite successful writes.
Open questions:
- Which detector fixtures exercise every supported layer distinction without
  deriving expected metadata from the candidate preparation code?

### preparation-refusal-does-not-append-audit-state

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No preparation-state before/after comparison runs.
Guarantee: A refused field preparation appends no scan and leaves durable effects
and audit rows uncommitted when the enclosing write fails.
Check: `always` - Snapshot prepared scans before each call; on detected
NewIdentity or input/output bound refusal, the scan sequence is unchanged, and
an aborted enclosing write leaves its durable effect/audit projection unchanged.
Successful earlier preparations may remain in memory but must not be committed
as a side effect of the refusal.
Fault/timing angle: A move-first refactor appends metadata before validation.
Required faults and enabling state: Test input above 512 KiB, replacement growth
beyond 512 KiB, and secret NewIdentity after an earlier successful preparation.
Confidence: high - [Evidence](evidence/preparation-refusal-does-not-append-audit-state.md).
Both size checks and the identity refusal precede scan append in the source.
Existing check: [Bound and rollback checks](existing-checks.md#redaction-ownership)
are unaudited; in-memory no-append coverage is not established.
Impact: Refused inputs can leave misleading audit receipts or partial effects.
Open questions:
- How will the test observe scan append separately from durable rollback?

### redaction-audit-does-not-depend-on-retained-payload

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No metadata-only ownership comparison runs.
Guarantee: Equivalent prepared detections, policies, and owner relationships
yield the same audit projection independently of retained redacted text.
Check: `always` - Hold prepared detections, field IDs, policies, owners, and
detector revision/digest equal; normalized persisted audit projections must
agree whether redacted text is retained or discarded, with unchanged effect
association and commit/rollback. The existing privacy contract separately keeps
a unique long synthetic sentinel out of substituted content and payload-free
audit columns; preserved identity values are exempt.
Fault/timing angle: Ownership transfer accidentally consumes metadata or changes
the audit/effect transaction boundary.
Required faults and enabling state: Use multiple fields/owners, JSON-observed
scans, successful writes, and refusal/rollback; normalize only generated audit
IDs and timestamps while preserving their graph relationships.
Confidence: high - [Evidence](evidence/redaction-audit-does-not-depend-on-retained-payload.md).
Audit persistence reads metadata, and JSON scans already retain empty text.
Existing check: [Receipt checks](existing-checks.md#redaction-ownership) are
unaudited; metadata-only differential coverage is not identified.
Impact: Allocation reduction can silently detach receipts from stored effects.
Open questions:
- Which allocation measurement will establish the performance benefit
  separately from functional parity? (needs human input)

## Relationships and retained canonical obligations

- K1 preserves selection and metadata; K2 isolates pre-cap irrelevance; K3 and
  K4 require separate row and byte witnesses. They retain
  [canonical-read-staleness-is-distinguishable-from-emptiness][canonical-old]
  rather than redefining freshness or availability.
- E1 and E2 constrain work relocation, while E3 witnesses overlap. They retain
  [no-task-outlives-the-generation-it-serves][host-old],
  [a-retired-generation-emits-nothing-and-mutates-nothing][retired-old], and
  [req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame][terminal-old].
  Best-effort cancellation and unknown outcomes remain normative. Existing
  fenced CAS and per-operation reconciliation apply; neither a new epoch
  mechanism nor exactly one durable effect per transform is prescribed.
- S1 constrains authorization reuse and S2 constrains batching. They retain
  [read-callbacks-cannot-write][read-old],
  [callback-scope-is-restored-after-unwind][scope-old],
  [protected-transactions-pin-fence-durability][durability-old], and
  [fenced-write-is-atomic][atomic-old]. S2 uses the existing snapshot contract,
  not a requirement to wrap unrelated observations in one transaction.
- H1 and H2 constrain selection and boundaries; H3 supplies inner pressure and
  H4 independently supplies outer retry pressure.
  They retain [defer-pass-replays-frozen-bytes-verbatim][replay-old],
  [frozen-unit-order-is-preserved][order-old], and
  [tokenizer-encoding-matches-the-independent-oracle][tokenizer-old]. No global
  BPE monotonicity, byte-sum conservation, or timestamp-sorting law is assumed.
- R1-R3 refine ownership preservation without duplicating Group H's
  [durable-identity-decision-is-made-inside-the-write-transaction][identity-old],
  [preserved-identity-name-does-not-exempt-its-value][value-old], and
  [refused-durable-write-leaves-no-row-and-no-receipt][refusal-old]. The retained
  allocation is already redacted. Removing it is not itself a security fix,
  and String object identity is not a semantic contract.

These relationships identify shared mechanisms, not proven dominance.

## Per-record handoff

Every active record goes to `/testing:test-strategy` for its form and oracle.
The following notes define the evidence to request, not implementation tickets.

| Record | Handoff focus |
| --- | --- |
| [K1][k1] | Package the fixed baseline and resolve the excluded-row error owner gate before M1. |
| [K2][k2] | Build paired valid stores without changing shared authority facts. |
| [K3][k3] | Certify row count/rank with small payloads so bytes cannot mask it. |
| [K4][k4] | Certify stored-payload pressure with at most 8192 candidates. |
| [E1][e1] | Name the physical-work owner and completion-to-cleanup observations. |
| [E2][e2] | Define a ledger per existing resource class and ownership transfer. |
| [E3][e3] | Construct start/close/completion ordering with explicit barriers. |
| [S1][s1] | Compare effects across authority changes, maintenance, and unwind. |
| [S2][s2] | Preserve independent freshness and each original atomic boundary. |
| [H1][h1] | Package the fixed reference independently and add tokenizer seam cases. |
| [H2][h2] | Separate direct API, wrapped retry, request validation, and replay. |
| [H3][h3] | Record multi-demotion pressure from the reference, not the candidate. |
| [H4][h4] | Record wrapped-slice pressure and cover the outer boundary matrix. |
| [R1][r1] | Cover the six policies and normalize audit identity without losing links. |
| [R2][r2] | Observe in-memory append refusal and durable rollback separately. |
| [R3][r3] | Compare metadata/effect graphs; measure allocation savings separately. |

Timing schedules for E1-E3 and S1-S2 may need
`/testing:deterministic-simulation-testing` after seam selection. Existing test
adequacy goes to `/testing:invariant-test-review`; production guards go to
`/low-level-systems:defensive-assertions-and-invariant-guards`. An independent
review disposition is recorded in [the portfolio](portfolio-evaluation.md).
The independent recheck and five-area scope confirmation are complete. Explicit
implementation design and measurement gates remain open; issue 351 preserves
them without creating implementation tickets.

[k1]: #canonical-memory-result-preserves-current-selection-pipeline
[k2]: #canonical-memory-irrelevant-candidates-do-not-consume-caps
[k3]: #canonical-memory-selection-pressure-is-exercised
[k4]: #canonical-memory-byte-pressure-is-exercised
[e1]: #route-cleanup-waits-for-request-owned-physical-work
[e2]: #request-work-accounting-covers-retained-resources
[e3]: #request-close-overlaps-live-work
[s1]: #guarded-callbacks-enforce-current-authority
[s2]: #callback-batching-preserves-observation-boundaries
[h1]: #history-budget-selection-preserves-reference-bytes
[h2]: #history-budget-boundaries-remain-distinct
[h3]: #history-budget-pressure-paths-are-exercised
[h4]: #history-outer-retry-pressure-is-exercised
[r1]: #prepared-field-output-and-audit-policy-agree
[r2]: #preparation-refusal-does-not-append-audit-state
[r3]: #redaction-audit-does-not-depend-on-retained-payload
[pass-read]: ../../../crates/daemon/src/lib.rs#L8133-L8202
[memory-read]: ../../../crates/daemon/src/canonical_memory.rs#L141-L212
[memory-default]: ../../../crates/daemon/src/config.rs#L122
[dispatch]: ../../../crates/host-runtime/src/dispatch.rs#L823-L934
[close]: ../../../crates/host-runtime/src/dispatch.rs#L1237-L1268
[read-callback]: ../../../crates/storage/src/lib.rs#L229-L245
[write-callback]: ../../../crates/storage/src/lib.rs#L290-L316
[prepared-execute]: ../../../crates/memory-store/src/lib.rs#L2245-L2271
[hard-compose]: ../../../crates/daemon/src/transform.rs#L4031-L4058
[history-render]: ../../../crates/daemon/src/decay_render.rs#L296-L338
[core-prep]: ../../../crates/memory-store/src/lib.rs#L3428-L3459
[transaction-prep]: ../../../crates/memory-store/src/lib.rs#L3462-L3494
[prepare-field]: ../../../crates/memory-store/src/lib.rs#L2204-L2243
[tokenizer-dependency]: ../../../crates/daemon/Cargo.toml#L21-L32
[read-visible]: ../../../crates/daemon/src/kernel_routes/read.rs#L159-L248
[admission-reference]: ../../../crates/kernel/src/admission.rs#L3132-L3298
[outer-retry]: ../../../crates/daemon/src/m0_compose.rs#L178-L215
[tokenizer-reference]: ../../../crates/tokenizer/src/lib.rs#L123-L155
[cache-reference]: ../../../crates/daemon/src/token_cache.rs#L165-L180
[soft-core]: ../../../crates/cache-stability/src/lib.rs#L221-L287
[soft-inputs]: ../../../crates/daemon/src/transform.rs#L4455-L4507
[render-json]: ../../../crates/daemon/testdata/render-golden.json
[tight-json]: ../../../crates/daemon/testdata/render-tight-golden.json
[shape-json]: ../../../crates/daemon/testdata/decay-store-shape.json
[differential-json]: ../../../crates/daemon/testdata/decay-store-differential.json
[canonical-old]: ../daemon/transform/catalog.md#canonical-read-staleness-is-distinguishable-from-emptiness
[host-old]: ../host-runtime/catalog.md#no-task-outlives-the-generation-it-serves
[retired-old]: ../host-runtime/catalog.md#a-retired-generation-emits-nothing-and-mutates-nothing
[terminal-old]: ../host-runtime/catalog.md#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame
[read-old]: ../shared-primitives/catalog.md#read-callbacks-cannot-write
[scope-old]: ../shared-primitives/catalog.md#callback-scope-is-restored-after-unwind
[durability-old]: ../shared-primitives/catalog.md#protected-transactions-pin-fence-durability
[atomic-old]: ../shared-primitives/catalog.md#fenced-write-is-atomic
[replay-old]: ../shared-primitives/catalog.md#defer-pass-replays-frozen-bytes-verbatim
[order-old]: ../shared-primitives/catalog.md#frozen-unit-order-is-preserved
[tokenizer-old]: ../tokenizer/catalog.md#tokenizer-encoding-matches-the-independent-oracle
[identity-old]: ../memory-store/catalog.md#durable-identity-decision-is-made-inside-the-write-transaction
[value-old]: ../memory-store/catalog.md#preserved-identity-name-does-not-exempt-its-value
[refusal-old]: ../memory-store/catalog.md#refused-durable-write-leaves-no-row-and-no-receipt
[spec-five-area]: https://github.com/ahrav/eidnara/issues/351
[spec-hp1]: https://github.com/ahrav/eidnara/issues/350
[receipt]: specification-publication-receipt.md
