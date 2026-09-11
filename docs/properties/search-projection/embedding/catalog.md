# RP2.1 embedding property catalog

## Scope and provenance

- Repository: `/local/home/ahrav/scratch/eidnara`.
- Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`, verified on 2026-09-10.
- Method: `/property-discovery-and-catalog` and [METHOD.md](../../METHOD.md).
- External-evidence scope is answered by the user: the settled RP2.1 plan,
  linked RP2 index and local parents, and this repository. No incidents are
  supplied. No tracker or web investigation is part of this pass.
- The deliverable is reusable discovery evidence. Product code, tracker items,
  and test/build/benchmark execution are outside this pass.
- Export/rebuild and projection/source coverage belong to other discovery
  lanes. This catalog starts with a durable pending identity and ends at safe
  embedding completion, recovery, and reclamation.
- Every plan guarantee is a claim under test. `Status: active` means it belongs
  in the handoff, not that it is implemented or exercised.

### Source register

| Label | Local source and locator | Why consulted |
| --- | --- | --- |
| P1 | [RP2.1 plan](../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md), lines 35-41, 75-83, 108-121, 144-156 | Settled embedding ownership, exact counting, pending recovery, completion fences, and bounds. |
| P2 | [RP2 index](../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md), lines 35-58 | Shared tokenizer, JobTable, EvalBudget, and RP2.9 approval contracts. |
| P3 | [Stage 2 parent](../../../../../commons/docs/plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md), lines 33-74 | Parent authority, preservation of canonical state, and measurement ownership. |
| P4 | [N1 ownership parent](../../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md), lines 123-169 | Shared supervisor ownership and disabled RP2.1 hooks. Its earlier current-code survey is not imported as fact. |
| P7 | [RP2.7 plan](../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-7-fusion-routes-plan.md), lines 79-82, 104-119 | Shared absolute deadline, request cancellation, physical ownership, and explicit dense-lane degradation. |
| W | [Host wire contract](../../../host-wire-protocol.md), lines 434-502 | Existing four-operation Synapse contract, truncation, and process-local job semantics. |
| C | [Existing-coverage readback](_lenses/00-existing-coverage.md) | Exact-match reuse decisions and stale reachability/check claims corrected locally. |
| E | [Central portfolio evaluation](portfolio-evaluation.md), analyst session `ses_f7623dcccffe3Y09nW2wVoABif`, supplied through the user | Independent cross-catalog findings and their embedding dispositions. This writer verifies citations and applies refinements, not a second independent review. |

Plan files are local external evidence and are not pinned by the Eidnara SHA.
The locators above are checked against their local contents for this pass.
Code citations below are repository-relative and checked against the named HEAD.

### Reachability and implementation boundary

Existing paths have distinct classifications:

| Existing path | Classification and current evidence |
| --- | --- |
| Synapse composition and disabled fallback | `default-production`: `crates/daemon/src/bin/eidnara_host/serve.rs:1097-1127` composes it; missing artifacts use `new(None)` at `:1043`, `:1057`. |
| Certified inference, bundle validation, and live JobTable serving | `explicit-config-only`: the selected generation must include the bundle manifest and ORT artifact (`serve.rs:1027-1065`). The macOS branch is explicitly unsupported. |
| Claude accounting calls | `default-production`: `crates/daemon/Cargo.toml:32` and `crates/daemon/src/token_cache.rs:133` contradict the old tokenizer catalog's absent-caller premise. |
| Dreamer scheduler loop | `default-production` after store open: `crates/daemon/src/lib.rs:3646-3660`. |
| Due review-user-memories slots | `explicit-config-only`: scheduled MODULE projects come from `lib.rs:13979-14027`. Actual scheduled model dispatch has only test-installed inputs (`lib.rs:3084-3105`, `:14041-14044`). |
| Deterministic engine and manual-clock scenarios | `test-only`: `crates/host-runtime/tests/support/synapse.rs:40-122` and `crates/daemon/src/dreamer_scheduler.rs:422-483`. |

Each new record carries its own proposed-only reachability explanation. Searches
find no Rust `search.sqlite`, `untruncated_token_len`, `EmbedTokens`, or
`ClaudeTokens` implementation. The current count is explicitly truncated
(`crates/host-runtime/src/synapse/inference.rs:577-589`). The scheduler only runs
the review-user-memories task (`crates/daemon/src/dreamer_scheduler.rs:334-371`).
These observations support absence of the RP2.1 embedding path; they do not
classify the existing host or tokenizer as test-only.

### Reuse and evaluation state

The [readback](_lenses/00-existing-coverage.md) precedes additions. Existing
fingerprint, wire validation, admission, degraded-lane, sealed-runtime, Claude
tokenizer, and scheduled-task records remain canonical in their own catalogs.
Their historical `Exercised` or audit labels are not carried into this pass.

Scoped system-model and property lenses are retained in
[_lenses/10-system-model.md](_lenses/10-system-model.md) and
[_lenses/20-property-lenses.md](_lenses/20-property-lenses.md).
Both taxonomies end with [_lenses/99-wildcard.md](_lenses/99-wildcard.md).
The harness rejects nested subagents at its depth limit, so these are sequential
attention passes, not independent review. The central independent analyst
`ses_f7623dcccffe3Y09nW2wVoABif` subsequently reviewed the three catalogs. Its
user-supplied findings and this writer's dispositions are recorded in
[portfolio-evaluation.md](portfolio-evaluation.md). These edits are not an
independent re-review or execution evidence.

## Shared prerequisites and acceptance

The async query-embedding lane and shared EvalBudget adapter are cross-plan
prerequisites shared with RP2.7. RP2.1.U3 integrates exact preflight and query
priority with the existing JobTable; RP2.7.U3 owns the authorized query route,
budget derivation before queue wait, async embedding before bounded scans,
degradation, and final validation (P7 lines 142-147). The handoff must preserve
that dependency instead of scheduling the full query path solely in RP2.1.

The central owner must declare enabled acceptance scenarios and their required
dimensions before execution. The shared
[acceptance-situation record](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
requires witnesses for every declared enabled marker/scenario dimension across
all three maps. Optional or unreached inference is not successful embedding
acceptance. One marker firing in one dimension cannot satisfy the others.
Central choices of enabled scope and RP2.9 bounds remain prerequisites; no
duplicate reachability record is added here.

## Check vocabulary

These are mathematical observation names, not proposed API names:

- `K` is a durable embedding work identity. It preserves project/source
  occurrence, revision, representation/span, model, fingerprint, dimensions,
  table epoch where used, and exact input hash. Projection owns its encoding.
- `current(K)` means the authoritative occurrence still exists and its current
  identity agrees with every field of K. Its input hash is checked against the
  current bytes produced by the approved input mapping from authoritative
  state, not only the stored revision or a stale projected hash. A tombstone
  is not current work. A remediated field matters only if that mapping uses it.
- `Pending(K)` remains durable until a matching durable vector or an explicit
  obsolete disposition supersedes it. JobTable `ready` is not this disposition.
- `Complete(K)` is product completion, not a Synapse descriptor status.
- `V(K)` is a reopened, durable vector record with K and the validated vector.
- `L` is an approved, versioned RP2.9 limit set. It supplies input bytes/tokens,
  pending count/bytes, batch limits, per-slice work, retry attempts, recovery
  deadline, query waiting/service bounds, and cancellation observation bounds.
- `D` is the absolute deadline of the shared EvalBudget. Every clone shares its
  cancellation flag and D; a new relative timeout is not equivalent.
- `T_input` and `B_input` are the smaller of the approved L caps and the
  verified lane's token and byte caps, respectively. Approval cannot enlarge
  the model's supported window. Queries have no durable K or completion marker.

No numeric RP2.1 value is approved by this catalog. Existing Synapse defaults
are implementation facts, not RP2.9 acceptance values. A campaign lacking L
cannot report the bounded-liveness or resource checks as passed.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [embedding-count-authority-is-untruncated](#embedding-count-authority-is-untruncated) | safety | test-only | always | active | high |
| [embedding-input-is-rejected-before-inference](#embedding-input-is-rejected-before-inference) | safety | test-only | always | active | high |
| [embedding-pending-drives-one-job-table](#embedding-pending-drives-one-job-table) | safety | test-only | always | active | medium |
| [embedding-completion-is-identity-fenced](#embedding-completion-is-identity-fenced) | safety | test-only | always | active | high |
| [embedding-complete-requires-durable-vector](#embedding-complete-requires-durable-vector) | safety | test-only | always | active | high |
| [embedding-restart-retries-durable-pending](#embedding-restart-retries-durable-pending) | liveness | test-only | always (per admitted episode; RP2.9-blocked) | active | medium |
| [embedding-backfill-preserves-query-admission](#embedding-backfill-preserves-query-admission) | liveness | test-only | always (per admitted episode; RP2.9-blocked) | active | medium |
| [embedding-identity-gc-preserves-live-work](#embedding-identity-gc-preserves-live-work) | safety | test-only | always | active | medium |
| [embedding-supervisor-shares-budget-and-joins](#embedding-supervisor-shares-budget-and-joins) | safety | test-only | always | active | medium |
| [embedding-dispatch-scan-makes-bounded-progress](#embedding-dispatch-scan-makes-bounded-progress) | liveness | test-only | always | active | high |
| [embedding-dispatch-actions-respect-pass-budget](#embedding-dispatch-actions-respect-pass-budget) | safety | test-only | always | active | high |

An admitted episode has independently witnessed workload and service
preconditions. It is not selected because the implementation successfully
admits a query or completes recovery. Missing required episodes fail the shared
acceptance-situation check; unapproved RP2.9 bounds block liveness acceptance.

## Records

### embedding-count-authority-is-untruncated

Type: safety
Reachability: test-only - this is a proposed contract; no production RP2.1
untruncated count path exists. Backend's private count truncates at
`crates/host-runtime/src/synapse/inference.rs:577-589`.
Status: active
Exercised: not yet - the exact-count API, typed units, and model-specific token
oracle are missing.
Guarantee: Product embedding preflight counts the complete input in EmbedTokens
using the same verified tokenizer artifacts and fingerprint as inference.
Check: `always` - for each successful count of text t under lane f, the recorded
artifact digests equal f's verified TokenizerRefs, the result equals the length
of an independently pinned untruncated token sequence for t under that lane's
special-token policy, and its unit is EmbedTokens rather than ClaudeTokens;
check every call because a count is an authority-bearing value.
Fault/timing angle: A truncating counter hides an oversized suffix; a separate
artifact lookup or provider tokenizer produces a plausible but wrong count.
Required faults and enabling state: Same-prefix texts whose complete encodings
straddle the token limit, a tokenizer with special tokens, and replacement
artifact bytes available after verification; compare isolated and batch contexts.
Confidence: high - [evidence](evidence/embedding-count-authority-is-untruncated.md).
P1 line 83 establishes the claim; bundle and inference source establish the gap.
Existing check: The existing
[fingerprint record](../../host-runtime/catalog.md#synapse-bundle-fingerprint-covers-every-artifact)
is reused; its current checks are unaudited here. None checks the proposed count.
Impact: Incorrect admission silently embeds a prefix or mixes token authorities.
Open questions:

- Which immutable model-token fixture and special-token/padding contract will
  certify the count independently of the implementation? (needs human input)

### embedding-input-is-rejected-before-inference

Type: safety
Reachability: test-only - no production RP2.1 product preflight exists. Existing
`crates/host-runtime/src/synapse/mod.rs:344-399` checks bytes and rows, not exact
untruncated tokens.
Status: active
Exercised: not yet - no product path joins exact counting to the inference-call
boundary or observes missing dense coverage after rejection.
Guarantee: Product input exceeding either approved byte or exact embedding-token
limits, or lacking verified counting authority, never reaches inference.
Check: `always` - for each product submission, `bytes(t) > B_input`, unavailable
exact count/identity, or `EmbedTokens(t) > T_input` implies zero engine calls
for that attempt; stored-source attempts preserve lexical state, leave dense
coverage missing, and create no Complete(K); rejected queries return no dense
result; valid boundary input passes preflight unchanged;
these are per-attempt safety conditions, not a claim that an overloaded lane
must execute valid input immediately.
Fault/timing angle: Validation after dispatch, byte-only admission, or silent
truncation converts invalid input into apparent success.
Required faults and enabling state: Token boundary and boundary-plus-one with
bytes within cap, byte boundary and boundary-plus-one, count failure, and a
healthy counting engine observer; take counters after startup certification.
Confidence: high - [evidence](evidence/embedding-input-is-rejected-before-inference.md).
P1 lines 58, 115, and 149 require preflight; current source lacks the token gate.
Existing check: Reuse
[wire validation](../../host-runtime/catalog.md#synapse-requests-are-validated-before-any-inference).
`crates/host-runtime/tests/synapse_protocol.rs:642-735`, `:869-952` are
unaudited adjacent checks; no exact-token product check exists.
Impact: Lost suffix evidence, wasted inference, and falsely complete vectors.
Open questions:

- RP2.9 must approve byte/token caps and the product rejection/coverage
  disposition; it must not add a Synapse count method. (needs human input)

### embedding-pending-drives-one-job-table

Type: safety
Reachability: test-only - no production RP2.1 durable pending source or driver
exists; `crates/host-runtime/src/synapse/jobs.rs:162-196` is process-local state.
Status: active
Exercised: not yet - durable pending operations and admission trace are absent.
Guarantee: Every product batch dispatch is backed by durable pending work and
uses the existing JobTable admission and result-lifetime authority.
Check: `always` - each new batch worker has a preceding durable Pending(K) and
one `Admitted` result from the component's JobTable; `Existing` creates no
worker; refusal or lost response does not erase Pending(K); accepted pending
mutations and resident admissions stay within their separate approved limits;
check each transition because durable discovery is not a second runnable queue.
Fault/timing angle: Dispatch before local commit, concurrent sweeps of one row,
full admission, and response loss between admission and descriptor observation.
Required faults and enabling state: A committed pending row, two discovery
attempts for it, a full JobTable, and a lost admission response. Local transaction
creation is supplied by the projection lane.
Confidence: medium - [evidence](evidence/embedding-pending-drives-one-job-table.md).
P1 line 109 and P2 line 54 name the ownership; JobTable reuse is source-verified.
Existing check: `crates/host-runtime/src/synapse/jobs.rs:387-489` and
`crates/host-runtime/tests/synapse_protocol.rs:990-1045` cover local admission
and replay, unaudited. No durable-to-process handoff check exists.
Impact: Required work disappears on restart or duplicate routing/lease machinery
acquires conflicting ownership and resource accounting.
Open questions:

- What bounded pending scan and row/byte admission operations does projection
  expose, and how is an unchanged batch key reconstructed? (needs human input)

### embedding-completion-is-identity-fenced

Type: safety
Reachability: test-only - no production RP2.1 completion transaction exists.
JobTable publication at `crates/host-runtime/src/synapse/jobs.rs:502-557` has no
current occurrence or revision authority.
Status: active
Exercised: not yet - current-identity comparison and a held-completion seam are
missing from the product path.
Guarantee: A result can complete only the still-current occurrence, revision,
model, fingerprint, dimensions, epoch, and input hash for which it was requested.
Check: `always` - every product completion commit requires `current(K)` and
matching returned item ID/hash, model, fingerprint, epoch, vector dimension,
and valid finite unit vector; if any identity component differs or the source
is tombstoned, the result leaves the current vector and completion state
unchanged; when the approved input mapping uses a remediated field, compare K's
input hash with authoritative current mapped bytes and obsolete a delayed
result if those bytes changed even at the same revision; evaluate inside the
mutation boundary to exclude TOCTOU.
Fault/timing angle: Revision/model change, deletion, or payload replacement
after dispatch but before the completion write, including a same-revision
remediation of a field used by the approved input mapping.
Required faults and enabling state: Hold a valid old result; independently
change each identity component, including model name with otherwise identical
fingerprint, then release it; include a second occurrence with identical bytes.
If the mapping includes the remediated field, include operator remediation
that changes authoritative input bytes while leaving the revision unchanged.
Confidence: high - [evidence](evidence/embedding-completion-is-identity-fenced.md).
P1 lines 149 and 156 require stale-result rejection and current-revision coverage.
Existing check: `crates/host-runtime/src/synapse/protocol.rs:781-805`,
`crates/host-runtime/src/synapse/jobs.rs:515-535`, and the late-publication test
at `jobs.rs:1217` are unaudited local guards, not product identity checks.
Impact: Stale or cross-occurrence vectors appear valid for current retrieval.
Open questions:

- Which projection mutation predicate supplies current(K) atomically, and how
  is obsolete work recorded without satisfying newer work? (needs human input)
- Does any approved embedding input depend on the remediated domain name or
  another remediated field? The answer determines that scenario's applicability.
  (needs human input)

### embedding-complete-requires-durable-vector

Type: safety
Reachability: test-only - no production RP2.1 vector persistence/completion path
exists; JobTable Ready at `crates/host-runtime/src/synapse/jobs.rs:540-550`
contains resident vectors only.
Status: active
Exercised: not yet - crash boundaries and reopened vector/completion observation
do not exist for the proposed projection.
Guarantee: Every durable product completion has its matching validated vector
durable before or atomically with the completion marker.
Check: `always` - after each visible completion and each crash/reopen boundary,
`Complete(K) implies V(K)` with exact identity and expected vector bytes; a
failed or unknown vector commit cannot authorize completion; leftover durable
vector without completion is allowed and must remain reconcilable; this is a
storage-state invariant, not a result-poll assertion.
Fault/timing angle: Termination or I/O failure between inference, vector write,
vector commit, and completion commit, including loss of a commit response.
Required faults and enabling state: Pending(K), a validated result, persistent
storage, an external crash witness, and reopening without graceful cleanup.
Confidence: high - [evidence](evidence/embedding-complete-requires-durable-vector.md).
P1 line 149 explicitly requires persistence before completion; JobTable has no
durable write.
Existing check: None for product durability. JobTable publication and result
lease tests are unaudited in-memory evidence only. The reusable child barrier,
kill/join, and reopen pattern in
`crates/kernel/tests/cas_fault_injection.rs:924-989`, `:1046-1094` is unaudited
and does not supply the missing product vector store or hooks.
Impact: A restart loses a vector that completion suppresses from backfill.
Open questions:

- Does one search transaction contain vector and completion, or is a separate
  vector store involved, and what crash durability is promised? (needs human input)

### embedding-restart-retries-durable-pending

Type: liveness
Reachability: test-only - no production RP2.1 restart scanner exists. JobTable
creates fresh maps and an incarnation at
`crates/host-runtime/src/synapse/jobs.rs:323-342`.
Status: active
Exercised: not yet - process-crash recovery into durable pending work is absent.
Guarantee: Stable, eligible pending work survives process loss and reaches
durable completion within approved retry and recovery bounds once faults cease.
Check: `always` - for each pending K captured at restart that remains current,
with certified lane, available storage, and the declared service opportunity,
stop injected faults and require Complete(K) and V(K) by L.recovery_deadline
within L.retry_attempts; changed identities become explicitly obsolete without
completing newer work; evaluate every finite recovery episode, not an unbounded
eventual condition.
Fault/timing angle: Crash after dispatch, missing descriptor, result eviction,
old-incarnation poll, or retryable engine failure before product completion.
Required faults and enabling state: Actual process termination, durable pending
state, fresh JobTable, and a bounded fault-free recovery window with finite
backlog and declared admission opportunities.
Confidence: medium - [evidence](evidence/embedding-restart-retries-durable-pending.md).
P1 lines 109 and 149 require recovery; its schedule and bounds remain proposed.
Existing check: `crates/host-runtime/tests/synapse_jobs.rs:297-334` covers route
loss, and `crates/host-runtime/src/synapse/jobs.rs:1123` covers retained retryable
failure, both unaudited. Neither is durable process-restart recovery.
Impact: Dense coverage stays missing forever despite a healthy restarted lane.
Open questions:

- RP2.9 must approve retry attempts, recovery window, and service opportunity;
  which terminal failure needs operator intervention? (needs human input)

### embedding-backfill-preserves-query-admission

Type: liveness
Reachability: test-only - no production RP2.1 priority admission exists. Current
query and batch workers share FIFO CPU acquisition at
`crates/host-runtime/src/synapse/mod.rs:682-691`, `:843-851`.
Status: active
Exercised: not yet - a saturated product backfill workload and approved query
service bound are missing.
Guarantee: Backfill saturation preserves the declared query admission capacity
and bounded service opportunity under the approved workload contract.
Check: `always` - with a healthy lane, query occupancy below L.query_capacity,
and offered queries within the declared envelope, backfill alone does not cause
admission refusal; each admitted uncancelled query starts within L.query_wait
and returns by its original D when the approved service bound fits D; excess
load or exhausted D has an explicit non-success disposition; check each query
in a finite saturation episode so FIFO progress alone cannot satisfy priority.
Fault/timing angle: Backfill fills job and CPU wait queues before a query arrives
or keeps replenishing work while the query waits.
Required faults and enabling state: Bounded saturated backfill, a query slot
available, an independently observed query arrival, and native calls completing
within the declared service bound; stop pressure for the recovery observation.
Confidence: medium - [evidence](evidence/embedding-backfill-preserves-query-admission.md).
P1 line 149 requires admission preservation; FIFO source does not establish it.
Existing check: `crates/host-runtime/tests/synapse_protocol.rs:186-233` asserts
mixed FIFO order, unaudited. No product query-priority check exists.
Impact: Offline backfill consumes the useful lifetime of interactive retrieval.
Open questions:

- RP2.9 and the admission owner must define query capacity, waiting/service
  bounds, and permissible backfill work ahead of queries. (needs human input)

### embedding-identity-gc-preserves-live-work

Type: safety
Reachability: test-only - no production RP2.1 identity GC exists. Existing
`crates/host-runtime/src/synapse/jobs.rs:748-886` handles ephemeral job retention,
not durable occurrence/model references.
Status: active
Exercised: not yet - durable identity/reference observation and the GC race seam
are absent.
Guarantee: Identity GC removes only obsolete, unreferenced embedding state and
cannot erase current work or let a delayed result resurrect an obsolete identity.
Check: `always` - at each GC deletion, K is obsolete under current identity and
the declared live-reference set is empty; current Pending(K), current V(K), and
active physical holders survive; after deletion, stale completion cannot insert
K as current or satisfy another identity; evaluate at deletion and completion,
not merely when a sweep selected candidates; a fault-free bounded sweep that
reports completion removes its selected identities that remain eligible and
unreferenced, so a no-op collector cannot report completed cleanup.
Fault/timing angle: Identity change or dispatch after GC selection, a held
result page during cleanup, and late completion after obsolete-state deletion.
Required faults and enabling state: Old and current identities for one source,
an active old result holder, a selected GC candidate, and a delayed completion;
include distinct occurrences sharing payload bytes.
Confidence: medium - [evidence](evidence/embedding-identity-gc-preserves-live-work.md).
P1 lines 39 and 146 assign identity GC; its durable reference rules are proposed.
Existing check: `crates/host-runtime/src/synapse/jobs.rs:1382` protects a leased
result page in memory, unaudited. None checks durable identity GC.
Impact: Cleanup deletes useful vectors, destroys retryable work, or revives stale
coverage through a late result.
Open questions:

- Which references block identity deletion, and what grace/retention and bounded
  sweep policy does RP2.9 approve? (needs human input)

### embedding-supervisor-shares-budget-and-joins

Type: safety
Reachability: test-only - no production RP2.1 embedding supervisor slice or
Synapse EvalBudget bridge exists. The existing scheduler and kernel budget are
separate paths (`crates/daemon/src/dreamer_scheduler.rs:163-220`;
`crates/kernel/src/applicability/checkout.rs:146-203`).
Status: active
Exercised: not yet - shared-budget stage observation and embedding supervisor
registration are absent.
Guarantee: Embedding maintenance uses the shared supervisor and each query
preserves one cancellation/deadline budget while retaining ownership until
physical work finishes.
Check: `always` - each maintenance slice stays within L.slice_work and its
approved lease/deadline; every query stage observes the same D and cancellation
flag; after exhaustion no next stage starts and no successful query is reported;
cooperative stop is observed within L.cancel_observation; every physically
running call retains its task owner, permits, and live charges until it exits,
and shutdown reports joined only after those owners finish; these are observable
transition invariants, not a promise that aborting a future stops native code.
Fault/timing angle: Cancellation during queue wait, tokenization, native
inference, or completion; supervisor stop during a bounded sweep; response loss.
Required faults and enabling state: A shared budget, at least two stages,
another due maintenance slice, a blocked native call, and independently
observed cancellation/deadline crossing before the gate is released.
Confidence: medium - [evidence](evidence/embedding-supervisor-shares-budget-and-joins.md).
P2 line 53 and P7 line 81 require composition; existing ownership paths are
source-verified but not composed for RP2.1.
Existing check: `crates/host-runtime/tests/synapse_protocol.rs:236-309` and
`crates/daemon/src/dreamer_scheduler.rs:1180-1254` are unaudited cancellation
checks. No shared embedding/SQLite/dense budget check exists.
Impact: Deadline renewal, orphaned inference, early capacity reuse, or maintenance
that monopolizes the daemon.
Open questions:

- What public budget bridge and supervisor registration express these owners,
  and what bounds an uncooperative native call's physical drain? (needs human input)

### embedding-dispatch-scan-makes-bounded-progress

Type: liveness
Reachability: test-only - `EmbeddingDispatcher::run_pass` has integration-test
callers, but no caller under `crates/daemon/src` outside its own definition.
Status: active
Exercised: yes - a durable queue with 2,048 older WrongScope rows ahead of one
eligible row is processed across repeated passes by one dispatcher.
Guarantee: One dispatch pass judges at most two pages of at most 1,024 candidates
each and persists its cursor only after the pass succeeds. The persistent cursor
advances only over the consecutive processed WrongScope prefix before the first
actionable row. It freezes before that action, resets at the ordered tail or when
the project or destination binding changes, and therefore makes the next pass
revisit an unresolved eligible, admitted, or terminal row.
Check: `always` - each pass reads no more than two keyset pages and each kernel
call receives at most `MAX_ELIGIBILITY_CANDIDATES`; after every successful pass,
the next pass starts strictly after the last consecutive WrongScope candidate
before the first action, or at the beginning after tail wrap or a binding change.
An actionable row remains visible until its durable state leaves the open-row
keyspace. Under a fixed finite WrongScope prefix and successful passes, later
eligible work cannot starve.
Fault/timing angle: More than two full pages belong to another project. A failed
terminal write must leave the cursor uncommitted so the unresolved candidate is
retried rather than skipped.
Required faults and enabling state: At least 2,048 older WrongScope rows, one
later eligible row, repeated calls on the same dispatcher, and a bounded terminal
write refusal before one retry.
Confidence: high -
[evidence](evidence/embedding-dispatch-scan-makes-bounded-progress.md). The SQL
order, private two-page cap, cursor commit point, tail wrap, and integration test
were verified together.
Existing check: `project_scan_cursor_advances_across_more_than_two_wrong_scope_pages`
and `terminal_search_deadline_preserves_the_candidate_for_retry` in
`crates/daemon/tests/embedding_dispatch.rs`; `eligible_rows_are_taken_oldest_first_not_by_identifier`
checks the keyset order.
Impact: A project with a large older prefix can starve forever, or a failed
disposition can be skipped permanently.
Open questions: None.

### embedding-dispatch-actions-respect-pass-budget

Type: safety
Reachability: test-only - the dispatcher has integration-test callers but no
production caller under `crates/daemon/src`.
Status: active
Exercised: yes - mixed terminal candidates, malformed identity, WrongScope, and
valid selected work are covered with a one-action pass bound.
Guarantee: In one pass, selected jobs plus terminal or malformed candidate
dispositions never exceed `DispatchBounds.max_jobs`; WrongScope consumes scan
work but no action budget, and terminal dispositions commit in one bounded
transaction.
Check: `always` - `selected.len + terminal.len <= max_jobs` at every disposition
boundary; malformed candidates consume one terminal action without entering the
kernel batch; verdict count must exactly equal valid candidate count; one
`write_within` transaction applies all terminal obsoletions or none, and only an
`Obsoletion::Marked` outcome emits a stop event.
Fault/timing angle: A page can contain only terminal candidates, or a malformed
candidate before valid work. The write lock can remain held through the terminal
deadline.
Required faults and enabling state: `max_jobs=1`, at least two terminal rows, a
malformed source identity followed by valid work, a WrongScope prefix, and a
contended search writer.
Confidence: high -
[evidence](evidence/embedding-dispatch-actions-respect-pass-budget.md). Production
counter increments, batch cardinality guard, transactional obsoletion, and the
named tests agree.
Existing check: `max_jobs_bounds_terminal_dispositions`,
`malformed_candidate_is_obsoleted_without_poisoning_valid_work`, and
`terminal_search_deadline_preserves_the_candidate_for_retry` in
`crates/daemon/tests/embedding_dispatch.rs`; `eligibility_cardinality_mismatch_is_a_release_error`
in `crates/daemon/src/embedding_dispatch.rs`.
Impact: A nominally bounded maintenance pass can perform unbounded writes, skip
valid work after malformed input, or emit completion events for no-op races.
Open questions: None.

## Relationship map

- The reused bundle fingerprint record is a prerequisite for
  [count authority](#embedding-count-authority-is-untruncated). It does not prove
  untruncated counting or prevent Claude-token substitution.
- [Count authority](#embedding-count-authority-is-untruncated) supplies
  [preflight](#embedding-input-is-rejected-before-inference). Neither implies
  the other: a correct count may be ignored; a gate may use a wrong count.
- [Pending ownership](#embedding-pending-drives-one-job-table) and
  [durable completion](#embedding-complete-requires-durable-vector) bound
  [restart recovery](#embedding-restart-retries-durable-pending). Safety without
  bounded retry still permits permanent missing coverage.
- [Identity fencing](#embedding-completion-is-identity-fenced) and
  [identity GC](#embedding-identity-gc-preserves-live-work) share the current-K
  predicate. Completion rejection does not prove GC deletion is safe.
- [Projection remediation](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
  supplies the same-revision changed-byte case only when the approved embedding
  mapping depends on that field. K already includes an input hash, so checking
  authoritative current bytes detects this case. This does not mandate a new
  key/version or a broad erasure policy.
- [Query admission](#embedding-backfill-preserves-query-admission) depends on
  [supervisor ownership](#embedding-supervisor-shares-budget-and-joins) for
  truthful capacity, but separate admission capacity does not prove priority.
- Durable Complete(K) implies a matching durable vector only under the
  completion record. JobTable Ready, input hash equality, and total vector count
  do not dominate that property. No broader dominance is claimed.
- [Catch-up and authorized recovery](../export-recovery/catalog.md#catchup-and-authorized-recovery-converge)
  owns projection-level convergence. Embedding restart recovery consumes its
  durable current pending state and does not duplicate that recovery record.

## Per-property handoff

Every row first goes to `/testing:test-strategy` for form and oracle selection.
These are testing seams, not prescribed test implementations.

| Property | Needed observation or construction | Additional handoff |
| --- | --- | --- |
| embedding-count-authority-is-untruncated | Verified artifact digest receipt, full token IDs from an independent fixture, typed count boundary. | `/testing:invariant-test-review` for fixture independence. |
| embedding-input-is-rejected-before-inference | Real count with a counting engine after startup, unchanged lexical row, absent completion. | `/low-level-systems:defensive-assertions-and-invariant-guards` for the preflight boundary. |
| embedding-pending-drives-one-job-table | Durable pending snapshot plus JobTable admission/worker trace and lost response. | `/testing:deterministic-simulation-testing` for replay and admission schedules. |
| embedding-completion-is-identity-fenced | Hold result; mutate one current identity field; compare authoritative mapped input bytes, including same-revision remediation when applicable. | `/testing:deterministic-simulation-testing`; `/testing:invariant-test-review` for noncircular identity oracle. |
| embedding-complete-requires-durable-vector | Reuse the kernel CAS child barrier/kill/reopen pattern with product commit hooks and durable vector/marker observation. | `/testing:crash-consistency-and-failpoint-testing` after the storage protocol is fixed; no new broad crash harness. |
| embedding-restart-retries-durable-pending | Actual restart using the existing crash pattern, fresh JobTable, per-K attempt/commit trace, approved fault-free window. | `/testing:deterministic-simulation-testing` for bounded retry workload; physical crash evidence remains separate. |
| embedding-backfill-preserves-query-admission | Query arrival/admission/start trace under saturated backfill, original D, declared service envelope. | Shared async query lane with RP2.7.U3 and priority integration in RP2.1.U3; `/testing:deterministic-simulation-testing` for mixed admission schedules. |
| embedding-identity-gc-preserves-live-work | GC candidate selection barrier, live-reference oracle, delayed completion, reopened state. | `/testing:deterministic-simulation-testing` for selection/deletion races. |
| embedding-supervisor-shares-budget-and-joins | Shared flag/D trace, physical completion gate, task/permit/charge census and supervisor slice count. | Shared EvalBudget/query adapter prerequisite with RP2.7.U3; `/testing:invariant-test-review` for existing cancellation checks and `/low-level-systems:defensive-assertions-and-invariant-guards` for enforcement. |
| embedding-dispatch-scan-makes-bounded-progress | Ordered backlog above two pages, persistent cursor observations, and a failed terminal write before retry. | `/testing:invariant-test-review` for the bounded-progress oracle. |
| embedding-dispatch-actions-respect-pass-budget | Mixed selected, terminal, malformed, and WrongScope candidates under a one-action bound. | `/testing:invariant-test-review` for action accounting and event semantics. |

The [fault map](fault-map.md) names independent coverage prerequisites. The
[existing-check inventory](existing-checks.md) distinguishes adjacent checks
from missing product checks. Eleven records, eleven index rows, and eleven evidence
files form this handoff. [Portfolio evaluation](portfolio-evaluation.md) records
the central findings, local dispositions, and unresolved owner decisions.

## Mechanical verification receipt

The original read-only verification on 2026-09-10 covered nine records. This
implementation update adds two records, two index rows, and two evidence files;
the METHOD fields occur in order in both additions.

The semantics distribution is eleven `always` records: eight safety and three
bounded-liveness claims. The fault map carries 18 constant `sometimes`
precondition markers. These counts describe artifacts, not executed checks.
No product test, build, or benchmark runs. The central evaluation is complete;
open design prerequisites and acceptance evidence remain outstanding.
