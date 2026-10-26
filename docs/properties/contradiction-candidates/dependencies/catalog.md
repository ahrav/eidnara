# Property catalog: implemented surfaces the P5 mechanism depends on

System: `/local/home/ahrav/scratch/eidnara`, HEAD `3a3b916b7`, 2026-10-24.
References re-verified at the #1164 branch tip (base `979fb814e`, scorer
commit `05acd0107`); see `../README.md`.
Slug prefix `p5dep-`. Method: `docs/properties/METHOD.md` and the
property-discovery skill. Companion files: `system-model.md`,
`existing-checks.md`, `fault-map.md`, `evidence/<slug>.md`.

## Scope and assumptions

Target: the eight implemented surfaces the P5 "write-time contradiction
candidates" plan consumes (S1 analyzer, S2 scope-filtered decision read, S3
budgeted Kernel reads, S4 source-capture publication, S5 memory-store
baseline, S6 harness-capture retention, S7 routed `review.*` reads and
`kernel.read`, S8 `eidnara_memory` action surface). The unbuilt P5 mechanism
is out of scope and covered by a separate domain-modeling pass.

External-evidence scope (user-confirmed): the plan set (00-index, P4, P5
plans), tracker specs #1145 and #1159 (local copies), repository code, tests,
`docs/host-wire-protocol.md`, `docs/memory-reviewer-operations.md`,
`docs/search-projection.md`, and `docs/properties/` at HEAD. No other tracker
issues, no incident history, no git-history lens.

Assumption (inferred): "default build" means the daemon built without the
`test-support` feature and run with `DaemonConfig::default()`
(`crates/daemon/src/config.rs:150-158`): `memory_enabled: true`,
`source_capture_enabled: false`.

Every documented guarantee below is a claim under test. Every `file:line`
was read at HEAD by this pass; drift from `context.md` or the plan is noted in
`system-model.md` (D8).

## Plan-versus-code disagreements carried as leads

These are recorded in `system-model.md` D1 to D7 and attached to records:

- D1 `ClaimFacts` carries no summary text (record
  `p5dep-claim-fact-bounds-are-refused-before-any-row-is-read`, open question).
- D2 `decision_page_within_budget` is store-wide and its budget bounds only the
  reader wait (`p5dep-decision-page-returns-an-oversize-decision-alone`,
  `p5dep-budget-exhaustion-stops-a-budgeted-kernel-read`).
- D3 a capture job publishes a window batch and a documents batch
  (`p5dep-capture-batch-is-bounded-per-batch`).
- D4 the analysis identity digest changes with the toolchain, not only with an
  epoch (`p5dep-analysis-epoch-and-identity-pin-the-contract`).
- D5 the Kernel prefilter keeps unscoped rows the daemon filter drops
  (`p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict`).
- D6 `kind` is an envelope key the doc does not name
  (`p5dep-review-reads-parse-strict-bodies-after-the-envelope`).
- D7 the pi plugin tool had no `allowedActions` at `3a3b916b7`; #1161 added
  one (`p5dep-memory-tool-actions-are-closed-and-allowlisted`).

## Reachability classes

Labels per METHOD.md rule 4, verified per record:

- `default-production`: reached in a default build with default config. The
  analyzer runs on every lexical query (`query_route.rs:1055`); `kernel.read`
  and the `review.*` family are routed methods any bound route can call; the
  lifecycle loop that runs the sweep starts when the store is installed
  (`lib.rs:4000-4008`); the `eidnara_memory` tool is registered by default in
  both plugins.
- `explicit-config-only`: `source_capture_enabled` defaults `false`
  (`config.rs:154`), so S4 publication, and the rows S6's sweep retires, exist
  only under explicit config. The S6 sweep code path itself runs in every
  default build; the S6 records are labelled `explicit-config-only` because
  their precondition, a `harness_capture` row, needs that config.
- `test-only`: none of the records below. Test hooks in `source_capture.rs`
  are `cfg(test)` and are not cataloged.

## Index

| Slug | Surface | Type | Semantics | Reachability | Priority |
| --- | --- | --- | --- | --- | --- |
| p5dep-analyzer-refuses-oversize-input-before-retention | S1 | safety | always | default-production | high |
| p5dep-analyzer-keeps-original-bytes-and-is-pure | S1 | safety | always | default-production | medium |
| p5dep-analysis-epoch-and-identity-pin-the-contract | S1 | safety | always | default-production | medium |
| p5dep-domain-decision-read-matches-domain-and-kind-in-sql | S2 | safety | always | default-production | medium |
| p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict | S2 | safety | always | default-production | high |
| p5dep-daemon-bounds-rows-after-the-scope-filter | S2 | safety | always | default-production | high |
| p5dep-kernel-domain-read-has-no-row-bound | S2 | reachability | sometimes | default-production | high |
| p5dep-claim-fact-bounds-are-refused-before-any-row-is-read | S3 | safety | always | default-production | medium |
| p5dep-budget-exhaustion-stops-a-budgeted-kernel-read | S3 | safety | always | default-production | high |
| p5dep-decision-page-returns-an-oversize-decision-alone | S3 | safety | always | default-production | medium |
| p5dep-response-never-waits-on-the-capture-job | S4 | safety | always | explicit-config-only | high |
| p5dep-capture-slot-is-single-and-released-on-every-exit | S4 | safety | always | explicit-config-only | high |
| p5dep-capture-batch-is-bounded-per-batch | S4 | safety | always | explicit-config-only | medium |
| p5dep-capture-job-ends-in-one-closed-vocabulary-count | S4 | safety | always | explicit-config-only | medium |
| p5dep-baseline-digest-change-closes-the-activation-gate | S5 | safety | always | default-production | medium |
| p5dep-harness-sweep-retires-owners-before-evidence | S6 | safety | always | explicit-config-only | high |
| p5dep-cited-evidence-survives-the-harness-sweep | S6 | safety | always | explicit-config-only | high |
| p5dep-review-reads-parse-strict-bodies-after-the-envelope | S7 | safety | always | default-production | medium |
| p5dep-observational-routes-read-and-take-no-background-part | S7 | safety | always | default-production | medium |
| p5dep-memory-tool-actions-are-closed-and-allowlisted | S8 | safety | always | default-production | high |

20 records. Semantics distribution: 19 `always`, 1 `sometimes`, 0
`always-or-unreached`, 0 `reachable`, 0 `unreachable`. Types: 19 safety, 1
reachability, 0 liveness. The one liveness candidate (L1, sweep progress) is
folded into `p5dep-harness-sweep-retires-owners-before-evidence` as a bounded
note rather than a separate record; see its open question.

## Records

### p5dep-analyzer-refuses-oversize-input-before-retention

Type: safety
Reachability: default-production - `analyze_segments` runs on every lexical
query through `crates/daemon/src/query_route.rs:1055` and `:1495` with
caller-supplied `LexicalBounds` (`:1061-1066`), and on the index side at
`crates/retrieval/src/lexical/index.rs:50`.
Status: active
Exercised: partial - `crates/retrieval/tests/lexical_analysis.rs:244-297`
drive exact-bound and one-over inputs for bytes and atoms. No test observes
allocation or retained-atom count at the moment of refusal.
Guarantee: `analyze_segments` refuses an input whose summed byte length
exceeds `max_input_bytes` before any atom is scanned, and refuses the input
whose atom count would exceed `max_atoms` before that atom is retained; a
refusal returns no partial `Analysis`.
Check: `always` - for every call, `bytes > max_input_bytes` implies
`Err(InputTooLong)` with zero atoms allocated, and the first atom past
`max_atoms` implies `Err(TooManyAtoms)` with exactly `max_atoms` atoms
retained at the time of refusal and none returned. `always` because the
bound is the contract on every evaluation; there is no optional path.
Fault/timing angle: none; pure function.
Required faults and enabling state: an input at `max_input_bytes + 1` bytes;
an input with `max_atoms + 1` atoms where the byte bound is not hit first;
segment inputs whose sum crosses either bound while no single segment does.
Confidence: high - [evidence](evidence/p5dep-analyzer-refuses-oversize-input-before-retention.md).
Read `analysis.rs:78-105`: byte check at `:82-88` precedes the loop; atom
check at `:92-96` precedes `parts(atom)` and `push`.
Existing check: `byte_bound_is_checked_before_analysis_at_exact_and_over_bound_sizes`
(`tests/lexical_analysis.rs:244`),
`atom_bound_refuses_the_next_atom_at_exact_and_over_bound_counts` (`:279`).
Status unaudited. Re-exercised by #1164 through the scorer:
`an_oversize_descriptor_is_refused_whole_and_an_exact_fit_is_analyzed`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:224-241`) gets
`InputTooLong` for a 65537-byte descriptor and an analysis for a
65536-byte one with 32768 atoms;
`an_unscorable_summary_is_reported_and_the_others_still_score`
(`:243-274`) covers the 8192-byte summary bound. The scorer's atom bounds
sit one past the most atoms the byte bounds admit
(`crates/daemon/src/contradiction_candidates/scorer.rs:85-96`), so `TooManyAtoms` is
unreachable through it.
Impact: a P5 pass that analyzes a 64 KiB descriptor and a project's decision
summaries does unbounded work or allocation per pass if the bound truncates
or retains before refusing.
Open questions: None.

### p5dep-analyzer-keeps-original-bytes-and-is-pure

Type: safety
Reachability: default-production - same call sites as the record above.
Status: active
Exercised: partial - goldens at `tests/lexical_analysis.rs:51-218` show
`HTTPServer` and `x86_64` atoms unchanged (`:192`); no test asserts purity
across repeated calls or absence of global state.
Guarantee: `analyze` is a deterministic function of its arguments with no
reads of clock, store, environment, or process state, and every returned atom
and part is a byte-exact substring of the input: no case folding, no
diacritic removal, no normalization.
Check: `always` - for every input, `analyze(x, b) == analyze(x, b)` across
calls and threads, and every `atom` and `part` satisfies
`input.contains(atom)` byte-wise with `atom.chars()` all satisfying
`continues_atom`. `always` because the P5 scorer's determinism (R3) rests on
it on every evaluation.
Fault/timing angle: none.
Required faults and enabling state: mixed-case and diacritic inputs; NFC and
NFD spellings of the same text (the doc at `analysis.rs:5` claims identical
splits, which is a separate claim this record does not promote).
Confidence: high - [evidence](evidence/p5dep-analyzer-keeps-original-bytes-and-is-pure.md).
Read `analysis.rs:107-199`: only `char` classification, slicing, and
`String::push`; no I/O, statics, or folding.
Existing check: the goldens (`tests/lexical_analysis.rs:24-36` harness,
`:51-218` cases). Status unaudited. Re-exercised by #1164: the scorer folds
after analysis (`crates/daemon/src/contradiction_candidates/scorer.rs:111-132`),
and its hand-written term sets
(`crates/daemon/tests/contradiction_candidates_scorer.rs:31-80`, `:131-159`) show
case-boundary parts (`HTTPServer` to `http`, `server`), kept diacritics,
and distinct NFC and NFD terms, which hold only if the analyzer returns
original bytes; `:276-318` shows equal output under 18 input orders.
Impact: the P5 scorer must fold case itself (plan R3); if the analyzer folded,
the fold would be double and the FTS5 contract at `mod.rs:31` would no longer
describe the probe.
Open questions: None.

### p5dep-analysis-epoch-and-identity-pin-the-contract

Type: safety
Reachability: default-production - `AnalysisIdentity::current()` is computed
once per process (`identity.rs:13-18`) and `ANALYSIS_CONTRACT_EPOCH` is a
public constant (`mod.rs:41`).
Status: active
Exercised: yes - `identity_pins_the_manifest`
(`tests/lexical_analysis.rs:389-412`) asserts the epoch string and the
digest literal on every test run.
Guarantee: the analysis contract is pinned by `ANALYSIS_CONTRACT_EPOCH =
"identifier-analysis.v1"` and by `AnalysisIdentity::current()`, a SHA-256 over
a length-delimited manifest of the epoch, `char::UNICODE_VERSION`, the FTS5
tokenizer, detail mode, column names, SQLite version, and SQLite source id;
any change to any component changes the digest and fails the pinned test.
Check: `always` - `ANALYSIS_CONTRACT_EPOCH == "identifier-analysis.v1"` and
`AnalysisIdentity::current().as_str() == <pinned literal>` in the test, and
`preimage()` contains `"{len}:{field}\n"` for each component. `always`
because the pin is evaluated once per build and must hold whenever evaluated.
Fault/timing angle: a toolchain bump that changes `UNICODE_VERSION` or a
SQLite bump changes the digest without a rule change (D4).
Required faults and enabling state: a build under a toolchain with a different
Unicode table, or a different bundled SQLite; a rule edit without an epoch
bump.
Confidence: high - [evidence](evidence/p5dep-analysis-epoch-and-identity-pin-the-contract.md).
Read `identity.rs:36-65` and the test.
Existing check: `identity_pins_the_manifest`. Status unaudited.
Re-exercised by #1164: the matcher preimage names `ANALYSIS_CONTRACT_EPOCH`
and the Unicode version
(`crates/daemon/src/contradiction_candidates/scorer.rs:657-659`), and
`the_matcher_digest_pins_every_rule`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:348-400`) asserts the whole
preimage, including both fields, and the digest literal. `identity_pins_the_manifest` and the retrieval crate are unchanged
since `3a3b916b7`.
Impact: a P5 consumer that records only the epoch string with its derived
rows cannot detect a toolchain-driven change in atom boundaries; a consumer
that records the identity digest can.
Open questions:
- Should the P5 derived rows record `AnalysisIdentity::current()` rather than
  the epoch alone, given D4? (needs human input) #1164 records the epoch and
  `char::UNICODE_VERSION` in the matcher digest; the SQLite fields of the
  identity do not affect `analyze_segments`, which the scorer calls without
  FTS5.

### p5dep-domain-decision-read-matches-domain-and-kind-in-sql

Type: safety
Reachability: default-production - `kernel.read` with
`RowSelection::DomainDecisions("memory")` is used by `canonical_memory.rs:188`
and `memory_capture.rs:1054`, both on default paths.
Status: active
Exercised: yes - `crates/kernel/tests/kernel_canonical_read.rs:227`, `:448`
compare the domain selection to the filtered full read and show another
domain's payload volume does not cost the domain read.
Guarantee: `visible_domain_decisions_as_of_in_scope(surface, as_of, domain,
scope)` returns exactly the rows of `visible_as_of_in_scope(surface, as_of,
None, scope)` whose `object_kind == "decision"` and `domain_id == domain`,
and the domain/kind predicate is evaluated in SQL before any row is decoded,
so a corrupt row outside the domain never fails the read.
Check: `always` - `domain_read.rows == full_read.rows.filter(kind ==
"decision" && domain_id == domain)` at the same snapshot, and a row of another
domain with an undecodable payload leaves the domain read `Ok`. `always`
because every P5 candidate-set read depends on it.
Fault/timing angle: none; single snapshot.
Required faults and enabling state: decisions in two domains at one snapshot;
a corrupt or oversize payload in the other domain.
Confidence: high - [evidence](evidence/p5dep-domain-decision-read-matches-domain-and-kind-in-sql.md).
Read `admission.rs:3551-3556` (`ids_predicate`) and `:3178-3207`.
Existing check: `the_domain_selection_equals_the_filtered_full_read`,
`the_domain_selection_reads_no_row_of_another_domains_payload_pressure`,
in-module `admission.rs:6240-6257`. Status unaudited.
Impact: the P5 candidate set would include non-memory decisions or fail on
another domain's corruption.
Open questions: None.

### p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict

Type: safety
Reachability: default-production - every `kernel.read` passes
`project.scope_term()` (`read.rs:178`).
Status: active
Exercised: partial - `crates/kernel/tests/kernel_scope.rs:235-388` covers
branch-dimension terms: matching, other value, redacted, no term on the
dimension, and a redacted filter value. No test covers a `set` operator
term, an operator other than `exact`/`set`, or `Dimension::Project`
directly.
Guarantee: for a `ScopeTermFilter { dimension, value }`, the SQL restriction
keeps every row whose scope the scope algebra could judge as matching
`value` on `dimension`: rows whose scope has no term on the dimension, a
term with an operator other than `exact` or `set`, a redacted stored term, or
a redacted filter value are kept; rows with a NULL scope are kept; only rows
whose every term on the dimension is a decodable `exact`/`set` that excludes
`value` are dropped.
Check: `always` - for every row `r` in the unfiltered read,
`algebra.matches(r.scope, dimension, value) != NoMatch` implies `r` is in
the filtered read; equivalently the filtered set is a superset of the
algebra's match set. `always` because a dropped matching row is a silent
cross-project omission on every read.
Fault/timing angle: redaction placeholders decode to `Uncertain`; the filter
must keep them (`admission.rs:3543-3550`).
Required faults and enabling state: scopes with no term, with a non-exact
operator, with a redacted value; a redacted filter value; a NULL scope row.
Confidence: high - [evidence](evidence/p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict.md).
Read `admission.rs:3626-3640` (the `WHERE` tail) and `project.rs:199-212`.
Existing check: `a_scope_filter_keeps_rows_whose_redacted_term_the_algebra_reports_uncertain`.
Status unaudited.
Impact: a P5 pass that trusts the Kernel prefilter alone (without the daemon
`ScopeFilter`) would see rows of other projects whose scopes are uncertain,
and rows with no scope (D5); a prefilter that dropped uncertain rows would
hide same-project decisions from the candidate set.
Open questions:
- D5: the daemon `ScopeFilter::matches` returns `false` for a `None` scope
  (`project.rs:204-206`) while the SQL keeps NULL scopes (`:3559`). Is a
  decision with no scope ever written in the `memory` domain at HEAD?
  (partial: `ProjectBinding::scope_spec` always attaches a scope for
  route-written rows; other writers not audited)

### p5dep-daemon-bounds-rows-after-the-scope-filter

Type: safety
Reachability: default-production - `read_visible` (`read.rs:163`) is the
only path behind `kernel.read` and the in-process `DomainDecisions` callers.
Status: active
Exercised: yes - `crates/daemon/tests/kernel_routes.rs:1958`, `:2080`,
`:2112` construct over-cap row counts and over-budget payloads.
Guarantee: after the Kernel read and the daemon `ScopeFilter`, `read_visible`
keeps at most `MAX_READ_ROWS` (8192) newest rows by `(created_commit_seq
desc, object_id)`, sets `truncated` when any row was dropped, reads
`decision_payload_sizes_as_of` for the kept decision ids before loading any
payload, cuts the row list where cumulative payload bytes pass
`MAX_READ_ROW_BYTES`, and only then loads `decisions_for_objects_as_of` for
the retained ids; a kept decision without a payload size or row fails the
read as `CorruptCanonicalRow`.
Check: `always` - `rows.len() <= 8192`; `truncated == (filtered_count >
8192 || payload_cut)`; the kept set equals sort-then-truncate of the filtered
set; no `decisions_for_objects_as_of` call names an id outside the retained
set; and `sum(payload_sizes[retained]) <= MAX_READ_ROW_BYTES` or the retained
set is exactly the prefix the cut allows. `always` because every read must
hold these bounds.
Fault/timing angle: the row bound cannot be a SQL `LIMIT` because the filter
runs after SQL (`read.rs:190`); a bound applied before the filter would
under-serve.
Required faults and enabling state: more than 8192 same-project visible rows;
decisions whose payload sizes sum past `MAX_WIRE_BODY_BYTES / 8`; a decision
row deleted out of band to force `CorruptCanonicalRow`.
Confidence: high - [evidence](evidence/p5dep-daemon-bounds-rows-after-the-scope-filter.md).
Read `read.rs:186-249`.
Existing check: `a_read_over_the_row_cap_serves_the_newest_rows_and_flags_truncation`,
`bounded_selection_matches_a_full_sort_and_truncate`,
`a_read_over_the_byte_budget_serves_the_newest_rows_that_fit`. Status
unaudited.
Impact: a P5 pass that copies this pattern (plan U1 "patterns to follow"
`read.rs:185`) inherits a bound that is applied after full materialization of
the Kernel rows; see the next record.
Open questions: None.

### p5dep-kernel-domain-read-has-no-row-bound

Type: reachability
Reachability: default-production - any `kernel.read` on a project with many
decisions.
Status: active
Exercised: not yet - no test constructs a same-project `memory` domain with
more than `MAX_READ_ROWS` decisions and observes the Kernel-side
materialization before the daemon heap.
Guarantee: the situation "the Kernel returned more rows than the daemon
keeps" is reachable at HEAD: `visible_domain_decisions_as_of_in_scope` has
no `max_rows`, `max_bytes`, or `EvalBudget` parameter and materializes every
matching row (`admission.rs:3188-3200`); the only bounds are the daemon's,
applied afterwards.
Check: `sometimes` - a campaign must produce at least one history in which
`rows.len() > MAX_READ_ROWS` is observed on the Kernel API's return value
inside the over-cap fixture (`kernel_routes.rs:1958`), calling the Kernel
API directly rather than probing `read.rs:192`, so that the daemon-side
bound and the plan's OQ5-1 concern are exercised rather than vacuously
satisfied. `sometimes` because this is situation coverage of the state
"Kernel returned more rows than the daemon will keep"; it is not a code
location (`reachable`) and it is not an invariant.
Fault/timing angle: none; a large project is the enabling state.
Required faults and enabling state: at least 8193 visible same-project
`memory` decisions at one snapshot.
Confidence: high - [evidence](evidence/p5dep-kernel-domain-read-has-no-row-bound.md).
Read the signature at `admission.rs:3165-3176` and `visible_selected_as_of`.
Existing check: none for the Kernel-side count; the daemon-side cap test
(`kernel_routes.rs:1958`) constructs over-cap rows and so would witness this
state if instrumented. Status unaudited.
Impact: the plan's R2 reads the whole scope-filtered domain per pass; the
work is unbounded in project size at the Kernel layer (plan OQ5-1). A P5
design that assumes a bounded Kernel read is unsound at HEAD.
Open questions:
- OQ5-1 (plan): add a budgeted or bounded variant, or read ids then page.
  (needs human input)

### p5dep-claim-fact-bounds-are-refused-before-any-row-is-read

Type: safety
Reachability: default-production - `claim_facts_at_within_budget` is used by
`crates/retrieval/src/claims.rs` on the search path.
Status: active
Exercised: yes - `crates/kernel/tests/kernel_claim_facts.rs:466-540+` for
`TooManyClaims`, `DuplicateClaim`, `NotADecision`, `FutureSnapshot`;
`crates/retrieval/tests/claims.rs:993` for an exhausted budget before the read.
Guarantee: `claim_facts_at_within_budget(ids, target, bounds, budget)` returns
`TooManyClaims` when `ids.len() > bounds.max_claims`, `DuplicateClaim` for a
repeated id, and `InvalidInput` for an empty or over-512-byte id, all before a
reader connection is acquired; an exhausted budget returns `Deadline` before
`claim_facts_inner` runs.
Check: `always` - for every call, `ids.len() > max_claims` implies
`Err(TooManyClaims)` with zero reader acquisitions
(`controlled_acquisitions_for_test` unchanged); `budget.is_exhausted()` at
entry implies `Err(Kernel(Deadline))` with zero acquisitions. `always`
because the refusal order is the whole point of the bound.
Fault/timing angle: none.
Required faults and enabling state: `max_claims + 1` ids; a budget already
exhausted at call time; an id of 513 bytes.
Confidence: high - [evidence](evidence/p5dep-claim-fact-bounds-are-refused-before-any-row-is-read.md).
Read `claim_facts.rs:230-304`: `limit.run` checks first (`:238`),
`check_claim_bounds` (`:258`) precedes `reader_with_limit` (`:261`).
Existing check: `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly`,
`an_exhausted_budget_is_refused_before_the_projection_is_read`. Status
unaudited.
Impact: for P5 R2 this bound caps the ids per call but, per D1, the call
returns no summary text; the plan's data path through this API does not
exist at HEAD.
Open questions:
- D1: plan R2 says summaries come through `claim_facts_at_within_budget`;
  `ClaimFacts` (`claim_facts.rs:146-156`) has no text field. Which read
  supplies summaries for P5: `decisions_for_objects_as_of_within_budget` or
  `decision_page_within_budget`? (needs human input)

### p5dep-budget-exhaustion-stops-a-budgeted-kernel-read

Type: safety
Reachability: default-production - the retrieval `claims` path and the
embedding dispatch path call budgeted reads
(`crates/daemon/tests/embedding_dispatch.rs:1102` exercises a held pool).
Status: active
Exercised: partial - a held reader pool ends an eligibility read at its bound
(`embedding_dispatch.rs:1102`); an exhausted budget is refused before a claims
read (`retrieval/tests/claims.rs:993`). No test crosses the deadline inside a
`claim_facts` statement, and no test holds the pool against
`decision_page_within_budget` or `decisions_for_objects_as_of_within_budget`.
The held-pool seam is `test-support` only, so a default build cannot
exercise this record.
Guarantee: a budgeted Kernel read returns `KernelError::Deadline` once its
`EvalBudget` is exhausted (deadline passed, own interrupt raised, or parent
interrupt raised) at the next poll point. For `claim_facts_at_within_budget`
the poll points are the reader acquisition loop, each `limit.check()`
between claims, and SQLite's progress handler inside each statement. For
`decision_page_within_budget` and `decisions_for_objects_as_of_within_budget`
the poll points are the reader acquisition loop only; the statement itself is
bounded by `max_rows` or the id list, not by the budget.
Check: `always` - two labelled clauses, one per API family (D2). (A)
`claim_facts_at_within_budget`: returns `Deadline` at the next poll point
after exhaustion, where poll points are acquisition retries, `limit.check()`
between claims, and the SQLite progress handler; an interrupt raised during
the statement returns `Deadline` before the statement completes. (B)
`decision_page_within_budget` and
`decisions_for_objects_as_of_within_budget`: return `Deadline` at the next
acquisition retry only. With the reader pool fully held, every budgeted read
returns `Deadline` at the first poll after the deadline (units: polls).
`always` because every budgeted call must honor its budget.
Fault/timing angle: a deadline that passes after acquisition but during a
long statement; a parent cancellation of a `linked()` child budget
(`checkout.rs:162-168`, `open.rs:1606-1615`).
Required faults and enabling state: a held reader pool
(`hold_readers_for_test`, `open.rs:614`, `test-support`); a deadline placed
inside the statement; a parent interrupt raised mid-read.
Confidence: medium - [evidence](evidence/p5dep-budget-exhaustion-stops-a-budgeted-kernel-read.md).
Read `slice/read.rs:134`, `:241`, `open.rs:565-593`, `:1654-1664`,
`:1693-1722`, `claim_facts.rs:238`, `:261`, `:274`, `:350`, `:357`. The
statement-interrupt path for claim facts is read in code but not observed by
a test this pass found.
Existing check: `an_eligibility_read_against_held_kernel_readers_ends_at_the_budget`,
`an_exhausted_budget_is_refused_before_the_projection_is_read`,
`exhausted_budget_is_refused_before_kernel_readers_are_taken`
(`search_catchup.rs:1876`). Status unaudited.
Impact: plan R1/KTD2 give the P5 pass its own deadline; if it uses the
decision-page API it gets a bounded wait plus a row-bounded scan, not a
deadline-bounded scan.
Open questions:
- D2: does the P5 pass need a statement-level interrupt on its decision read?
  If so, the page API would need `reader_with_limit`. (needs human input)

### p5dep-decision-page-returns-an-oversize-decision-alone

Type: safety
Reachability: default-production - `decision_page_within_budget` is a public
Kernel API with one production caller at HEAD: the claim materializer's
bootstrap page (`crates/daemon/src/claim_sources.rs:802`), run from the
search lifecycle owner (`crates/daemon/src/search_lifecycle_owner.rs:570`).
Tests call it at `crates/kernel/tests/kernel_slice.rs:1637-1775`. P5 would be
its second production caller. (Refinement R2 proposed `test-only`; the
bundle pass found the production caller and kept the label.)
Status: active
Exercised: yes - `kernel_slice.rs:1664-1700` sets a 1-byte bound and twin
bounds one byte apart.
Guarantee: a page examines at most `max_rows` decision rows (live or
invalidated) in `object_id` order after `after`, returns the live ones, and
stops before a live decision whose payload would push the page past
`max_payload_bytes`, except that the first live row of a page is always
returned even when its payload alone exceeds the bound; `next` is the last
examined id while rows remain.
Check: `always` - `examined <= max_rows`; `sum(live payload sizes) <=
max_payload_bytes || live.len() == 1`; `next.is_some() == (more rows remain
after examined)`. `always` because every page must hold these bounds.
Fault/timing angle: none beyond the reader wait (previous record).
Required faults and enabling state: a payload larger than the byte bound;
pages of only invalidated rows; exactly `max_rows` rows remaining.
Confidence: high - [evidence](evidence/p5dep-decision-page-returns-an-oversize-decision-alone.md).
Read `slice/read.rs:126-220`.
Existing check: `decision_pages_examine_a_bounded_row_count_and_cut_before_the_byte_bound`.
Status unaudited.
Impact: for P5, a page is store-wide (no domain or scope predicate,
`:139-146`), so paging to the `memory` domain of one project examines every
domain's and every project's rows in `object_id` order (D2).
Open questions:
- D2: is a domain- and scope-filtered page variant needed for P5 OQ5-1?
  (needs human input)

### p5dep-response-never-waits-on-the-capture-job

Type: safety
Reachability: explicit-config-only - `source_capture_enabled` defaults
`false` (`config.rs:154`); with it off, `offer` is never reached
(`source_capture.rs:3`, test `source_capture_tests.rs:298`).
Status: active
Exercised: yes - `the_response_is_identical_whatever_capture_and_the_kernel_are_doing`
(`source_capture_tests.rs:330`) and `capture_latency_measurement` (`:938`).
Guarantee: the transform response is settled (encoded) before the
synchronous handoff runs; the handoff (`into_job` with `try_acquire_owned`,
`admission.spawn`) performs no await, no kernel call, and no waiting lock
(`lib.rs:9228-9235`, `source_capture.rs:602-605`); the job body runs on a
spawned task. `AfterResponse` is detached from the outcome (`lib.rs:13470`),
`settle_prepared` runs (`:13477`), and only on a `RequestOutcome::Response`
is `after.run()` called (`:13479-13481`), which takes the slot and spawns the
job; a non-response outcome drops the work unrun and releases its charges.
Check: `always` - for every pass, the `AfterResponse` closure contains no
blocking or kernel operation, and the encoded response bytes are
byte-identical with capture on, off, kernel starting, kernel unavailable, or
slot busy. `always` because plan KTD2 and the module header make this the
invariant of every pass.
Fault/timing angle: a kernel that blocks or is unavailable; a busy slot; a
job that panics.
Required faults and enabling state: capture on; a kernel held unavailable or
starting during the pass; a prior job holding the slot.
Confidence: high - [evidence](evidence/p5dep-response-never-waits-on-the-capture-job.md).
Read `lib.rs:9219-9236`, `:13470-13481`, `dispatch.rs:24-35`, `:445-460`.
Existing check: the two tests above. Status unaudited.
Impact: a P5 trigger that ran inside `AfterResponse` before `job.run()` or
awaited the job would re-couple response latency to the kernel; a trigger
chained after the job's terminal `Count` keeps this property.
Open questions: None.

### p5dep-capture-slot-is-single-and-released-on-every-exit

Type: safety
Reachability: explicit-config-only - as above.
Status: active
Exercised: yes - `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
(`source_capture_tests.rs:674`), `a_settled_report_counts_the_job_that_released_the_slot`
(`source_capture.rs:1588`).
Guarantee: the daemon holds one publication slot (`Semaphore::new(1)`,
`source_capture.rs:266`); `into_job` takes it with `try_acquire_owned` or
refuses as `RefusedQueueFull` without waiting (`:602-605`); the permit lives
in `ActiveSlot` inside the job and is released, with the `Active` gauge
decremented, when the job's closure returns or unwinds (`:420-431`,
`:667`).
Check: `always` - `slot.available_permits() <= 1` and at most one
`CaptureJob` exists at a time; after any job end (completed, deadline,
discarded, panicked) `available_permits() == 1` and `Active == 0`. `always`
because a leaked permit stops every later capture.
Fault/timing angle: a panic inside `publish` or `publish_locked` (the latter
caught and resumed after the commit gate drops, `:790-803`); shutdown
cancellation mid-job.
Required faults and enabling state: a second pass handed off while a job
holds the slot; an injected panic in the blocking closure; cancellation.
Confidence: high - [evidence](evidence/p5dep-capture-slot-is-single-and-released-on-every-exit.md).
Existing check: the two tests above. Status unaudited.
Impact: a P5 pass that ran inside the job's closure would extend slot
occupancy and raise `RefusedQueueFull` for the next pass; a pass outside the
job must not take the slot.
Open questions: None.

### p5dep-capture-batch-is-bounded-per-batch

Type: safety
Reachability: explicit-config-only - as above.
Status: active
Exercised: partial - `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`
(`source_capture_tests.rs:437`) for the unit count; byte-bound deferral and
the two-batch shape per job are not asserted together.
Guarantee: each screened batch admits at most `MAX_SOURCE_BATCH_UNITS` (32)
units and `MAX_SOURCE_BATCH_BYTES` (64 KiB) of text; a unit over
`MAX_SOURCE_UNIT_BYTES` (64 KiB) is refused whole; excess units are deferred
to a later pass. One capture job publishes the window batch and then, when
the pass read configured documents, a second documents batch under the same
commit gate and budget (D3).
Check: `always` - for every `publish_screened[_between]` call, `admitted.len()
<= 32 && admitted_bytes <= 65536` and no admitted unit has `text.len() >
65536`; per job, the number of screened batches is at most 2. `always`
because every batch must hold the bound.
Fault/timing angle: none.
Required faults and enabling state: a window of 33 units; a window whose
text sums past 64 KiB; a 64 KiB + 1 unit; a pass with both a window and
pending document reads.
Confidence: high - [evidence](evidence/p5dep-capture-batch-is-bounded-per-batch.md).
Read `harness_sources.rs:612-616`, `:1557`, `:1602-1603`;
`source_capture.rs:832-840`, `:904-980`.
Existing check: the 33-unit test; `a_refused_window_still_captures_its_documents`
(`:2227`) shows the documents batch is independent. Status unaudited.
Impact: the P5 trigger's per-pass input is at most 32 window descriptors plus
the documents batch's descriptors, not 32 total (D3 corrects the plan's "one
batch per pass" reading).
Open questions:
- D3: the header at `source_capture.rs:3` says "at most one screened batch".
  Is the documents batch meant to be covered by that sentence? (needs human
  input)

### p5dep-capture-job-ends-in-one-closed-vocabulary-count

Type: safety
Reachability: explicit-config-only - as above.
Status: active
Exercised: partial - per-scenario tests assert specific terminals;
`the_capture_report_carries_no_unit_text` (`:782`) asserts the report's
content class. No test asserts exactly one terminal per job across all exits.
Guarantee: every `CaptureJob::run` returns exactly one terminal `Count`
(`Completed`, `Deadline`, `Discarded`, `SessionInvalidated`,
`KernelUnavailable`, `KernelError`, `WorkerPanicked`, `RefusedResidentShort`)
and increments exactly that counter once; the counter and log vocabulary is
the closed `counts!` list (`source_capture.rs:76-114`) and carries no unit
text, path, identity, or digest. No hook runs after publish outside
`cfg(test)`; the returned `Count` is the only completion signal.
Check: `always` - for every job, `sum(terminal counters after) -
sum(terminal counters before) == 1`, the returned `Count` is in the terminal
subset, and every log line's reason is in `Count::ALL`. `always` because the
P5 trigger must key off this signal on every job.
Fault/timing angle: join error vs panic (`:668-676`); `end()` called once per
return path.
Required faults and enabling state: each exit path forced: cancel before
start, kernel unavailable, budget exhausted, delete epoch moved, panic in
closure, publish refusal.
Confidence: medium - [evidence](evidence/p5dep-capture-job-ends-in-one-closed-vocabulary-count.md).
Every `return self.end(..)` in `publish`/`publish_locked` was read; the
`publish_locked` `Ok(end) => end` at `:800` returns a value already counted
by `end()` inside, and `run` counts join failures at `:672-675`. Exactly-once
is argued from code structure, not observed.
Existing check: `the_capture_report_carries_no_unit_text`,
`the_capture_report_reaches_host_status_field_for_field`. Status unaudited.
Impact: a P5 trigger that waits for a post-publish hook has nothing to wait
on at HEAD; it must be inserted at `admission.spawn(job.run())`
(`lib.rs:9231`) or inside `run`.
Open questions: None.

### p5dep-baseline-digest-change-closes-the-activation-gate

Type: safety
Reachability: default-production - `MemoryStore::open` runs on every daemon
start; `evaluate` runs on every MemoryReviewer pass and before every job
(`memory-reviewer-operations.md:44`).
Status: active
Exercised: yes - `crates/memory-store/tests/baseline.rs:495-517` opens a
prior-baseline file and observes refusal with unchanged bytes; #1165
re-exercises it with the fixture frozen at the baseline before
`contradiction_candidates` (`crates/memory-store/tests/fixtures/prior_baseline.sql`);
`activation.rs:491-495` observes `IdentityMismatch("memory store baseline")`.
Guarantee: a Memory Store file opens only when its `format_marker` digest
equals `memory_store::baseline_digest()`; any baseline text change (such as
P5 U3 adding `contradiction_candidates`) changes that digest, so an existing
store refuses to open untouched and the MemoryReviewer activation record's
`memstore_baseline_digest` term no longer matches, closing the gate as
`identity_mismatch` until the owner renews the record.
Check: `always` - `marker(file) != baseline_digest() ⇒ open refused as
StoreError::Baseline with bytes unchanged`; `open Ok ⇒ marker(file) ==
baseline_digest()`; and `evaluate(record, live) ==
Err(IdentityMismatch("memory store baseline"))` whenever
`record.memstore_baseline_digest != live.memstore_baseline_digest` and every
earlier term matches. `always` because the gate is evaluated on every pass.
Fault/timing angle: none; a deployment step.
Required faults and enabling state: a store created under the current
baseline, then a binary with a changed `baseline.sql`; an activation record
naming the old digest.
Confidence: high - [evidence](evidence/p5dep-baseline-digest-change-closes-the-activation-gate.md).
Read `memory-store/src/lib.rs:335-340`, `storage/src/lib.rs:2631`,
`activation.rs:174-240`, `worker.rs:167`.
Existing check: the tests above and
`docs/properties/shared-primitives/catalog.md:676`
`store-schema-identity-matches-the-baseline` (active) for the open refusal
across consumers. Status unaudited here; that record reports its own audit.
Impact: P5 U3 is a baseline rotation (index D4, Batch M1); every deployment
must reset the store and renew the activation record; proposals bound to the
old incarnation become unadoptable (`memory-reviewer-operations.md:236-240`).
Open questions: None.

### p5dep-harness-sweep-retires-owners-before-evidence

Type: safety
Reachability: explicit-config-only - the sweep path runs in every default
build (`lib.rs:4000-4008`, `lifecycle.rs:327`) but the property's
precondition, a `harness_capture` row, exists only when
`source_capture_enabled` was true (`config.rs:154`).
Status: active
Exercised: partial - `local_file.rs:797`, `:895` and `lifecycle.rs:704` cover
pin recheck, page rollback, and one retirement; no test observes the order
of owner and evidence invalidation within one commit or a descriptor's
`invalidated_commit_seq` after the sweep.
Guarantee: for each `harness_capture` evidence row whose `retain_until <=
now`, that no live pin holds, and whose registry object is a live evidence
object, the sweep retires every live `source_descriptor` observation citing
it (setting each descriptor's `invalidated_commit_seq`), then retires the
evidence unless another live row cites it; at most 64 captures per call, in
one commit per page, oldest deadline first; a capture pinned since selection
is skipped; a refused capture rolls back and the page recommits without it.
Check: `always` - after a sweep commit at `seq`, every retired descriptor has
`invalidated_commit_seq == seq`, the evidence has `invalidated_commit_seq ==
seq` or is cited by a live non-descriptor row, and `retired + retained <=
64`. Within the commit, descriptor invalidation precedes the
`cited_elsewhere` check and `retire_evidence`; observed through outcome: an
uncited capture ends `retired`, which is possible only if its own
descriptors were retired before `cited_elsewhere` ran. `always` because
every sweep must leave this state.
Fault/timing angle: a pin taken between selection and the writer recheck; a
`NotFound`/`Conflict`/`InvalidInput` from one capture's step inside a page.
Required faults and enabling state: capture on; a descriptor older than 30
days (`now` advanced past `retain_until`); a pin on one candidate; a forced
refusal on one capture of a multi-capture page.
Confidence: high - [evidence](evidence/p5dep-harness-sweep-retires-owners-before-evidence.md).
Read `local_file.rs:64-70`, `:245-310`, `:453-501`, `:514-565`;
`slice/write.rs:569-577`, `:627-666`.
Existing check: the three tests above. Status unaudited.
Impact: P5 R4's sweep drops rows "whose descriptor is retired"; P4 C6 refuses
a `dispute` whose evidence is not live as `not_found`. A candidate older than
30 days cannot be confirmed once its evidence is retired.
Open questions:
- Liveness bound (L1): an expired, unpinned, uncited capture is retired
  within `ceil(pending_ahead / 64)` passes at 5 s retry interval while passes
  advance. No test measures this; a record would need a fault-free window
  with the lifecycle loop running. Should it be a separate liveness record?
  (needs human input)

### p5dep-cited-evidence-survives-the-harness-sweep

Type: safety
Reachability: explicit-config-only - the sweep path runs in every default
build (`lib.rs:4000-4008`, `lifecycle.rs:327`) but the property's
precondition, a `harness_capture` row, exists only when
`source_capture_enabled` was true (`config.rs:154`).
Status: active
Exercised: not yet - no test places a `decision_events` or `decisions`
citation on a harness capture's evidence and runs `expire_harness_captures`.
Guarantee: evidence that a live observation (other than the capture's own
descriptors), a live decision, a decision event of a live decision, or a live
asserted edge cites is not retired by the sweep; its descriptors are retired
and the capture is counted `retained`; it stays retained until that citation
is gone. A P4 `dispute` event citing the descriptor's `evidence_id` on a live
decision is such a citation.
Check: `always` - for every swept capture, `cited_elsewhere(evidence) ⇒
evidence.invalidated_commit_seq IS NULL after the sweep`, and `count ==
"retained"` on the first sweep that selects the capture; later sweeps
exclude it as a candidate (`local_file.rs:556-559`). `always` because the
retention promise (`schema.rs:136`) holds on every sweep.
Fault/timing angle: a citation written between the candidate read and the
writer transaction; `cited_elsewhere` runs inside the writer so it sees it.
Required faults and enabling state: capture on; an expired descriptor whose
evidence a live decision's `decision_events` row cites (a confirmed P5
candidate through P4 `dispute`); then a sweep.
Confidence: high - [evidence](evidence/p5dep-cited-evidence-survives-the-harness-sweep.md).
Read `slice/write.rs:42-54` (`EVIDENCE_CITED_SQL`), `:589-608`;
`local_file.rs:487-501`, `:587-602`.
Existing check: guard `write.rs:606-608` (`retire_evidence` returns
`Conflict` on a cited row), unreachable from the sweep path because
`retire_capture` returns `retained` first (`local_file.rs:496-498`); route to
`/low-level-systems:defensive-assertions-and-invariant-guards`. No sweep
test with a foreign citation. Status unaudited.
Impact: W1: confirming a P5 candidate keeps its evidence alive past 30 days
for as long as the disputed decision is live; the descriptor is still
retired, so P5 R4's "descriptor retired" sweep rule and P4's "evidence live"
confirm rule diverge for a confirmed candidate. The P5 design must decide
which row its sweep keys on.
Open questions:
- Does P5 R4 intend to drop a candidate whose descriptor is retired even when
  its evidence is live because the candidate was confirmed? (needs human
  input)

### p5dep-review-reads-parse-strict-bodies-after-the-envelope

Type: safety
Reachability: default-production - `review.list` and `review.read` are routed
methods on any bound route.
Status: active
Exercised: yes - `memory_reviewer_wire.rs:238-252` (unknown field and
malformed identity are `invalid_params`), `:643-649` (`v` field on accept),
`:25-34` (`disabled` without MODULE authority).
Guarantee: for `review.list`, `review.read`, `review.accept`, and
`kernel.read`: the route binding and `project_root` are checked first and a
mismatch answers `invalid`/`project_mismatch` before the body is parsed; the
envelope keys `method`, `kind`, `session_id`, `project_root` are removed and
the remaining body is parsed with `deny_unknown_fields`, so any other field
is `invalid_params`; an uninstalled Memory Store or a root whose authority is
not MODULE answers `{"kind":"terminal","terminal":"disabled"}`; a store that
cannot be read answers `store_unavailable`.
Check: `always` - `project mismatch ⇒ outcome == project_mismatch && body not
parsed`; `body has a field outside the struct ⇒ invalid_params`; `authority
== NotModule ⇒ terminal disabled`; `authority read Err ⇒ terminal
store_unavailable` (no seam at HEAD (F18); untestable until one exists).
`always` because every request must be judged this way.
Fault/timing angle: a store read failure during the authority lookup.
Required faults and enabling state: a route bound to another root; a body
with an unknown field; a root without MODULE authority; an injected store
read failure.
Confidence: high - [evidence](evidence/p5dep-review-reads-parse-strict-bodies-after-the-envelope.md).
Read `wire.rs:32-51`, `:131-139`, `:203-220`; `kernel_routes/mod.rs:415-474`.
Existing check: the wire tests above; the literal-set test at
`wire.rs:448-463+`. Status unaudited.
Impact: P5 U4's owner read "in the same family" inherits this contract; a
P5 route that parsed its body before the project check, or accepted unknown
fields, would break the family's stated order. `review.use_outcome` (#1157)
joined the family after `3a3b916b7`: it reads the Kernel alone and has no
`disabled` terminal (`host-wire-protocol.md:798`); this record does not
cover it.
Open questions:
- D6: `kind` is stripped as an envelope key (`mod.rs:461`) but the doc at
  `host-wire-protocol.md:768` names three envelope fields. Is a body field
  `kind` meant to be accepted silently? (needs human input)

### p5dep-observational-routes-read-and-take-no-background-part

Type: safety
Reachability: default-production - any route opened with harness `cli`.
Status: active
Exercised: yes - `memory_reviewer_wire.rs:260`
`an_observational_route_reads_the_same_outcomes_as_an_ordinary_route`;
`lib.rs:35508` `an_observational_binding_reads_its_project_and_takes_no_part_in_background_work`.
Guarantee: a binding whose harness is `OBSERVATIONAL_HARNESS` (`"cli"`)
authorizes and resolves its project like any route and can call the routed
read family with the same answers, and is excluded from
`RouteBindings::participating` so it enrolls no project in the MemoryReviewer
worker, scheduler, or search maintenance roster and never displaces a
participating binding for the same root.
Check: `always` - `review.list`/`review.read` answers on a `cli` route equal
the answers on an ordinary route for the same project and receipt; and
`participating()` never contains a binding with `is_observational()`.
`always` because both halves must hold for every observational route.
Fault/timing angle: an observer binding newest on a root with an older
participating binding; the observer closing first.
Required faults and enabling state: two routes on one root, one `cli`; a
recorded receipt to read.
Confidence: high - [evidence](evidence/p5dep-observational-routes-read-and-take-no-background-part.md).
Read `lib.rs:274-281`, `:382-386`, `:413-417`.
Existing check: the two tests above. Status unaudited.
Impact: P5 U4's owner read is an observational read; it must be callable on a
`cli` route and must not enroll the project in any background work.
Open questions: None.

### p5dep-memory-tool-actions-are-closed-and-allowlisted

Type: safety
Reachability: default-production - `eidnara_memory` is registered by default
in opencode (`packages/opencode-plugin/src/plugin/tool-registry.ts:54-59`)
and pi (`packages/pi-plugin/src/tools/index.ts:69-75`,
`packages/pi-plugin/src/index.ts:475`).
Status: active
Exercised: partial - opencode allowlist tests
(`tools.test.ts:1162-1234`) cover the empty, omitted, and partial
allowlists; the human-authority test (`:1149-1160`) covers the
approve/enforce/delete refusals; the dispute test (`:1338-1354`) covers the
missing tool-call identity, the context-researcher agent, and a narrowed
allowlist with zero transport calls; the pi tests cover a disallowed action
(`packages/pi-plugin/src/tools/eidnara-memory.test.ts:303-326`), the default
allowlist refusing `dispute` (`:422-431`), and the registration omission
(`packages/pi-plugin/src/tools/index.test.ts:40-70`). This pass found no
test that calls `get` without a tool-call id to show a read runs under the
`"read"` identity.
Guarantee: the action vocabulary is the closed list
`["create","get","revise","archive","merge","dispute"]`; `get` is the only
read and every other action is a mutation that requires a stable tool-call
identity; in both plugins an omitted `allowedActions` admits the default
actions, which exclude `dispute`, an explicit empty list admits none, and a
disallowed or unknown action answers
`Error: Action '<a>' is not allowed in this context.` without a daemon call;
the exposed `action` enum and the dispute fields follow the allowlist; in
opencode a call from the `context-researcher` agent is refused inside the
tool; in pi the tool is omitted from the context_researcher's registered
surface (`memoryToolEnabled: false`).
Check: `always` - `action ∉ allowed ⇒ error text && transport.calls
unchanged`; `isEidnaraMemoryMutation(a) == (a != "get")`; `agent ==
"context-researcher" ⇒ refused` (opencode); `"dispute" ∉
EIDNARA_MEMORY_DEFAULT_ACTIONS`; the pi registration for
`memoryToolEnabled: false` excludes `eidnara_memory`. `always` because every
invocation is judged.
Fault/timing angle: none.
Required faults and enabling state: each action under an empty, partial, and
omitted allowlist; a call with `agent: "context-researcher"`; a pi
registration with `memoryToolEnabled: false`.
Confidence: high - [evidence](evidence/p5dep-memory-tool-actions-are-closed-and-allowlisted.md).
Read `types.ts:6-29`, `tools.ts:48-58`, `:99-138`, `:152-154`,
`packages/opencode-plugin/src/plugin/tool-registry.ts:58`,
`packages/pi-plugin/src/tools/eidnara-memory.ts:97-108`, `:125-185`,
`packages/pi-plugin/src/tools/index.ts:30-32`, `:69-75`,
`packages/pi-plugin/src/subagent-entry.ts:63`.
Existing check: the tests above. Status unaudited.
Impact: P5 R6 adds `candidates` as a read; it must enter the closed list and
`EIDNARA_MEMORY_READ_ACTIONS` (`types.ts:24`), and its default-allowlist
membership is a rollout decision in both plugins.
Open questions:
- D7 is resolved since `3a3b916b7`: #1161 gave the pi tool `allowedActions`
  with the same default as opencode (`eidnara-memory.ts:129-134`). Whether
  `candidates` joins `EIDNARA_MEMORY_DEFAULT_ACTIONS` is a decision for #1168
  (needs human input).

## Relationship map

Shared mechanisms:

- S1 records share `analyze_segments`; `p5dep-analyzer-refuses-oversize-input-before-retention`
  and `p5dep-analyzer-keeps-original-bytes-and-is-pure` are independent
  (bounds vs content). `p5dep-analysis-epoch-and-identity-pin-the-contract`
  guards both against silent rule drift. Suspected dominance: if the identity
  pin holds and the goldens hold, the purity claim likely holds for the
  pinned toolchain.
- S2: `p5dep-domain-decision-read-matches-domain-and-kind-in-sql` and
  `p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict` are the two
  predicates of one SQL statement (`served_classes_sql`).
  `p5dep-daemon-bounds-rows-after-the-scope-filter` consumes their output;
  `p5dep-kernel-domain-read-has-no-row-bound` is the coverage witness that
  makes the daemon bound non-vacuous.
- S3: `p5dep-budget-exhaustion-stops-a-budgeted-kernel-read` governs the
  acquisition path of both other S3 records; D1 and D2 tie all three to plan
  OQ5-1.
- S4: `p5dep-response-never-waits-on-the-capture-job` depends on
  `p5dep-capture-slot-is-single-and-released-on-every-exit` (a leaked slot
  does not delay the response but stops capture) and on
  `p5dep-capture-job-ends-in-one-closed-vocabulary-count` (the trigger point
  P5 would use). `p5dep-capture-batch-is-bounded-per-batch` bounds the P5
  per-pass input.
- S6: `p5dep-cited-evidence-survives-the-harness-sweep` is the exception
  branch of `p5dep-harness-sweep-retires-owners-before-evidence`; both depend
  on `p5dep-capture-*` having produced rows. W1 couples S6 to P4 `dispute`.
- S5 stands alone; it is the deployment precondition for P5 U3.
- S7 and S8 are the two surfaces P5 U4 extends;
  `p5dep-observational-routes-read-and-take-no-background-part` constrains the
  route, `p5dep-memory-tool-actions-are-closed-and-allowlisted` the tool.

Links to existing catalogs (not duplicated here):

- `docs/properties/shared-primitives/catalog.md:676`
  `store-schema-identity-matches-the-baseline`: the open refusal S5 relies on.
- `docs/properties/memory-store/catalog.md:717`, `:761`, `:819`: invalidated
  migration records; they establish that no migration or version ledger exists.
- `docs/properties/lexical-retrieval/catalog.md:205`
  `lexical-work-is-bounded-and-observed` and `:244` `original-budget-stops-sql`:
  the retrieval-side bound and budget records; neither covers `analyze`
  directly, which is why S1 has its own records here.
- `docs/properties/daemon/interpretation-layer/catalog.md`: source-capture
  adapter records (`il-*`); none covers publication timing or the slot.

## Handoff

Every record routes to `/testing:test-strategy` for form and oracle. Records
needing a held reader pool, a panic injection, or an advanced clock
(`p5dep-budget-exhaustion-stops-a-budgeted-kernel-read`,
`p5dep-capture-slot-is-single-and-released-on-every-exit`,
`p5dep-harness-sweep-retires-owners-before-evidence`,
`p5dep-cited-evidence-survives-the-harness-sweep`) also route to
`/testing:deterministic-simulation-testing` or the existing `test-support`
seams. Existing checks route to `/testing:invariant-test-review`. Open
questions tagged `(needs human input)` go to the P5 spec author.
