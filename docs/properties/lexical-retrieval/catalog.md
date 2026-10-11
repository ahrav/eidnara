# Lexical retrieval records

Source: the RP2.3 specification
([#348](https://github.com/ahrav/eidnara/issues/348)) and its lexical retrieval
ticket ([#391](https://github.com/ahrav/eidnara/issues/391)). The parent's
property bundle is unavailable here, so these records are reconstructed from
the ticket's acceptance criteria and verified against the code that implements
them on the #391 branch at `b3b9b7a80`.

## Scope

The lexical lane of the query route: probe analysis and compilation
(`crates/retrieval/src/lexical/compile.rs`), the bounded scan and judged
admission (`crates/retrieval/src/lexical/retrieve.rs`), the retrieval batch
adapter over the kernel's eligibility policy
(`crates/retrieval/src/eligibility.rs`), and the daemon's lexical admission in
`crates/daemon/src/query_route.rs`. The daemon reaches the lane through `scan`
(`crates/daemon/src/query_route.rs:1098`) and `admit` (`:1258`), each under
`QueryRouteLimits::lexical_retrieval_bounds` (`:170`); `retrieve` has no
production caller, and `crates/retrieval/tests/lexical_retrieval.rs:607`
`admitting_a_released_scan_equals_retrieve_and_carries_the_judged_candidate`
is the bridge every `retrieve`-driven check rests on. Fusion, packing, and
host application are outside the part except where AC8 names them.

## Reachability classes

Each record carries its own class with the evidence in its `Reachability`
field.

- `default-production`: every record. A default install serves the route, and
  the lexical lane runs whenever the request analyzes to one or more atoms
  (`crates/daemon/src/query_route.rs:1094` returns `Undeclared` otherwise).
  `one-kernel-eligibility-policy` is a static property of the source tree;
  its label describes the paths the scan protects, which
  `portfolio-evaluation.md` bias 1 records as a human call.
  `host-application-matrices` carries the label for the hosts' default
  application path with no host evidence yet; bias 2 records that.
- `explicit-config-only`: none.
- `test-only`: none. `Completion::Empty` is produced only by tests that call
  `scan` or `retrieve` with zero probes; the route guards the case earlier.

## Part artifacts

The part holds this catalog, one evidence file per record under `evidence/`,
[`existing-checks.md`](existing-checks.md), [`fault-map.md`](fault-map.md),
and [`portfolio-evaluation.md`](portfolio-evaluation.md). The evaluation's
refinements are applied here; its gaps and biases stay in that file.

## Index

| Slug | AC | Type | Check | Exercised |
| --- | --- | --- | --- | --- |
| [`literal-probes-never-operate`](#literal-probes-never-operate) | AC1 | safety | `always` | yes |
| [`zero-terms-run-no-match`](#zero-terms-run-no-match) | AC1 | safety | `always` | yes |
| [`rank-then-occurrence-order`](#rank-then-occurrence-order) | AC2, AC4 | safety | `always` | yes |
| [`eligibility-before-accepted-slots`](#eligibility-before-accepted-slots) | AC3, AC4 | safety | `always` | yes |
| [`one-kernel-eligibility-policy`](#one-kernel-eligibility-policy) | AC3 | safety | `always` | yes |
| [`lexical-work-is-bounded-and-observed`](#lexical-work-is-bounded-and-observed) | AC5 | safety | `always` | partial |
| [`original-budget-stops-sql`](#original-budget-stops-sql) | AC6 | safety | `always` | yes |
| [`retrieval-reads-no-payload`](#retrieval-reads-no-payload) | AC7 | safety | `always` | yes |
| [`no-alternative-lexical-index`](#no-alternative-lexical-index) | AC10 | safety | `always` | yes |
| [`host-application-matrices`](#host-application-matrices) | AC8 | reachability | `sometimes` | not yet |

Semantics distribution: nine `always`, one `sometimes`. AC9, the RP2.9 corpus
run, has no record; `portfolio-evaluation.md` gap 2 queues it.

## Records

Each record names its runnable check and the enabling state that check
constructs. A record exercised only by a fixture engine says so.

### literal-probes-never-operate

Type: safety
Reachability: default-production
Status: active
Exercised: yes - a populated FTS5 engine runs request text holding `OR`,
`NEAR`, quotes, and syntax-shaped input as quoted atoms, and the unquoted
control lets the operators operate
Guarantee: every compiled probe is one quoted atom, so request text never
reaches the engine as query syntax.
Check: `always` - each probe's match set equals the hand-written term set for
its atom, because one operating operator widens or narrows a result silently.
Fault/timing angle: none.
Required faults and enabling state: request text holding `OR`, `NOT`, and
`NEAR(...)` with engine match sets; `*`, `"`, `^`, and column filters at the
analysis level only; a control that removes the quoting.
Confidence: high - [evidence](evidence/literal-probes-never-operate.md). The
compiler and both engine tests were read at the branch.
Existing check: `crates/retrieval/tests/lexical_engine.rs:196`
`literal_operators_are_terms`; `:213`
`removing_the_quotes_makes_operators_operate`;
`crates/retrieval/tests/lexical_retrieval.rs:551`
`a_probe_matches_only_its_term_and_operators_in_text_stay_literal`;
`crates/retrieval/tests/lexical_analysis.rs:236` `atoms_never_contain_quotes`.
Impact: a crafted request widens retrieval past its literal terms.
Open questions: None.

### zero-terms-run-no-match

Type: safety
Reachability: default-production - the route's guard is the `Undeclared`
return at `crates/daemon/src/query_route.rs:1094`; `Completion::Empty` itself
is reached only by tests that call `scan` or `retrieve` with zero probes
Status: active
Exercised: yes - an analysis with zero atoms compiles to no probe and runs no
MATCH, while a nonempty control completes
Guarantee: a request that analyzes to zero atoms runs no engine query and
returns no candidates, with no broad fallback.
Check: `always` - zero probes yield `Completion::Empty` and no contribution,
because a fallback scan would return unrelated rows.
Fault/timing angle: none.
Required faults and enabling state: separator-only and empty request text; a
nonempty control request.
Confidence: high - [evidence](evidence/zero-terms-run-no-match.md).
Existing check: `crates/retrieval/tests/lexical_retrieval.rs:582`
`zero_probes_run_no_match_while_a_control_probe_contributes`;
`crates/retrieval/tests/lexical_engine.rs:268`
`zero_atoms_issue_no_probe_while_a_nonempty_control_completes`;
`crates/daemon/tests/query_route_dense.rs:365`
`a_request_without_prose_leaves_a_ready_dense_lane_undeclared_and_runs_no_producer`
(a separator-only query is `InvalidQuery` at the route).
Impact: an empty request returns arbitrary rows.
Open questions: None.

### rank-then-occurrence-order

Type: safety
Reachability: default-production
Status: active
Exercised: yes - unequal ranks and exact ties are compared with an independent
ordered reference, under duplicated and permuted probes
Guarantee: contributions order by lower raw FTS rank, then occurrence
identifier bytes, and each occurrence keeps its best probe, the lowest ordinal
among equal ranks.
Check: `always` - the contribution order equals the reference order for every
probe multiset, because a storage-order tie-break makes results depend on
insertion history.
Fault/timing angle: none.
Required faults and enabling state: occurrences hit by several probes; equal
raw ranks from distinct probes; a large equal-rank group at the scan bound;
dead rows inside a distinct-rank probe past the scan bound.
Confidence: high - [evidence](evidence/rank-then-occurrence-order.md).
Existing check: `crates/retrieval/tests/lexical_retrieval.rs:641`
`contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation`;
`:682` `equal_ranks_from_distinct_probes_keep_the_lowest_ordinal`; `:1950`
`a_large_equal_rank_group_at_the_bound_keeps_the_lowest_identifiers`; `:2470`
`dead_rows_inside_a_distinct_rank_probe_neither_take_slots_nor_hide_truncation`.
Impact: nondeterministic ranking across rebuilds.
Open questions: None.

### eligibility-before-accepted-slots

Type: safety
Reachability: default-production
Status: active
Exercised: yes - an ineligible leader is judged in a bounded batch before any
accepted slot is used, and equal-byte siblings fill every slot past it
Guarantee: canonical eligibility batches run before an occurrence takes an
accepted slot, so eligible tails and equal-byte siblings survive ineligible
leaders, and distinct equal-byte occurrences stay distinct contributions.
Check: `always` - accepted contributions equal the first `max_accepted`
eligible occurrences in comparator order, because a slot an ineligible leader
took would drop an eligible result.
Fault/timing angle: one-row kernel batches against the accepted bound.
Required faults and enabling state: a hidden leader; equal-byte occurrences of
distinct objects; `batch_rows` of one.
Confidence: high - [evidence](evidence/eligibility-before-accepted-slots.md).
Existing check: `crates/retrieval/tests/lexical_retrieval.rs:716`
`an_ineligible_leader_is_excluded_without_taking_an_accepted_slot`; `:750`
`the_accepted_bound_inside_a_batch_still_tallies_the_rest_and_an_exact_fill_stays_complete`;
`:2253` `equal_byte_siblings_stay_distinct_and_fill_past_an_ineligible_leader`.
Impact: an eligible memory is dropped behind a hidden one.
Open questions: None.

### one-kernel-eligibility-policy

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the daemon wire route, the retrieval batch adapter, and the
kernel agree on every occurrence at one snapshot, and a scan of every
workspace crate's sources outside the kernel finds no other caller of the
`judge_eligibility` entry points
Guarantee: outside the kernel, only the daemon's wire and cache adapter, the
retrieval batch adapter, and the embedding dispatcher call or name the
kernel's `judge_eligibility` entry points; retrieval's product dependencies
include the kernel and exclude the daemon; and the kernel's verdict function
stays private.
Check: `always` - verdicts agree across the three paths, and no other source
calls or names the entry points, because a second policy can diverge on one
verdict.
Fault/timing angle: retirement and correction after a grant.
Required faults and enabling state: claims retired or corrected after a
grant; a foreign project.
Confidence: high - [evidence](evidence/one-kernel-eligibility-policy.md).
Existing check: `crates/daemon/tests/claim_eligibility.rs:184`
`retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot`; `:456`
`canonical_eligibility_has_one_kernel_policy_entry_outside_the_kernel`.
Impact: one path serves an occurrence another refuses.
Open questions: None.

### lexical-work-is-bounded-and-observed

Type: safety
Reachability: default-production
Status: active
Exercised: partial - probes, counted and scanned rows, SQL VM steps, and Rust
allocations are observed at the scan bound and one row below it; judgments and
batches at the bound; SQL steps grow with one more row on a ranked and on a
common run; a common scan makes the same number of allocations for 32 more
matches past its bounds, and ranked allocations grow with the matches inside `rank_budget`;
SQLite's own allocations stay outside the recorder, and the document-size
lookups of FTS5's rank function run in internal statements `sql_steps` excludes
Guarantee: every probe count, scan, rank, and judgment runs under a
caller-supplied bound, and the request reports the work it did, so a result
cap alone never bounds the work.
Check: `always` - each counter stays within its bound at the bound and one past
it, and a smaller result cap leaves scan work unchanged, because an unbounded
scan hides behind a small result.
Fault/timing angle: none.
Required faults and enabling state: a repeated probe; a scan bound one below
the match count; a result cap of one; one more matching row under ranked and
common thresholds; 32 more matching rows past the common thresholds and the
scan bound.
Confidence: medium - [evidence](evidence/lexical-work-is-bounded-and-observed.md).
Counters and Rust allocation events were read and asserted; RP2.9 has not
approved an allocation bound to compare them with.
Existing check: `crates/retrieval/tests/lexical_retrieval.rs:789`
`the_scan_bound_marks_incomplete_only_past_the_bound_and_refusals_precede_every_probe`;
`:1069` `a_repeated_probe_runs_the_engine_once_and_adds_no_work`; `:1576`
`ranking_work_is_admitted_by_exact_counts_at_the_d26b_boundaries`; `:2312`
`allocations_follow_the_scan_bound_and_the_rank_budget`; `:2381`
`work_counters_report_exact_and_over_bound_work`;
`crates/daemon/tests/query_route.rs:320`
`each_bound_saturates_before_its_protected_work`; `:532`
`production_limits_are_the_d23_set`.
Impact: a request does unbounded work under a small result cap.
Open questions:
- Which allocation counter and bound does RP2.9 approve? (needs human input)

### original-budget-stops-sql

Type: safety
Reachability: default-production
Status: active
Exercised: yes - cancellation and deadline injected at every route phase,
including the lexical lane, end the request with the original budget's
terminal, and an engine interrupt anywhere in counting, ranking, or a common
scan is budget exhaustion
Guarantee: the request's original budget bounds queue wait, every probe,
canonical validation, and materialization; no lane takes a fresh deadline, and
cancellation interrupts SQL in flight.
Check: `always` - a deadline that passes inside a phase ends the request at that
phase with `Deadline`, because a lane that renewed its budget would reach the
next phase.
Fault/timing angle: a sleep past the original deadline inside each phase; an
interrupt during each statement.
Required faults and enabling state: per-phase cancellation and deadline hooks;
the connection's interrupt.
Confidence: high - [evidence](evidence/original-budget-stops-sql.md).
Existing check: `crates/daemon/tests/query_route.rs:74`
`cancellation_and_deadline_are_observed_in_every_phase`;
`crates/retrieval/tests/lexical_retrieval.rs:1407`
`an_engine_interrupt_from_the_connection_ends_the_request_as_budget_exhaustion`;
`:2049`
`an_interrupt_anywhere_in_counting_ranking_or_a_common_scan_is_budget_exhaustion`.
Impact: a cancelled request keeps the engine busy.
Open questions: None.

### retrieval-reads-no-payload

Type: safety
Reachability: default-production
Status: active
Exercised: yes - retrieval runs against a projection whose payload table is
renamed away and returns the same result
Guarantee: lexical retrieval returns occurrence identifiers, raw ranks, and
tie identity, never payload bytes; payloads load only after the caller's
authorization.
Check: `always` - the retrieval over a projection whose `payloads` table is
renamed equals the retrieval before the rename, because a denied request must
load no bytes and no payload statement exists in the lexical module to mark
`unreachable`; the readers are `fetch_payload`
(`crates/retrieval/src/packing/mod.rs:269`) and `payload_lookup`
(`crates/retrieval/src/lib.rs:606`), both outside the lane.
Fault/timing angle: none.
Required faults and enabling state: a projection whose `payloads` table is
renamed, so any payload read fails.
Confidence: high - [evidence](evidence/retrieval-reads-no-payload.md).
Existing check: `crates/retrieval/tests/lexical_retrieval.rs:1275`
`retrieval_reads_no_payload_bytes`.
Impact: a denied request loads payload bytes.
Open questions: None.

### no-alternative-lexical-index

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the schema inventory test compares the stored schema with the
documented one, which holds one `unicode61` FTS5 table with `detail = full`
Guarantee: the projection holds one word-level FTS5 index and no prefix,
shingle, n-gram, trigram, or contentless path.
Check: `always` - the stored schema equals the documented schema, because an
added index changes recall and write cost without approval.
Fault/timing angle: none.
Required faults and enabling state: none.
Confidence: high - [evidence](evidence/no-alternative-lexical-index.md).
Existing check: `crates/retrieval/tests/schema_inventory.rs:312`
`the_baseline_matches_the_frozen_inventory_field_for_field` (the `lexical`
table is `crates/retrieval/baseline.sql:94`);
`crates/retrieval/tests/lexical_engine.rs:248`
`no_prefix_expansion_and_the_prefix_control_differs`.
Impact: an unapproved index changes recall and resource use.
Open questions: None.

### host-application-matrices

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - no OpenCode or Pi matrix applies lexical results through
the full exact, lexical, f32, fusion, packing, and application path
Guarantee: both supported hosts apply the fused lexical contribution, one per
occurrence, with no denied payload in captured applied context.
Check: `sometimes` - each supported host cell must reach captured application
at least once per campaign.
Fault/timing angle: a dropped application.
Required faults and enabling state: real host application capability; the
production f32 oracle; the RP2.9 corpus.
Confidence: low - [evidence](evidence/host-application-matrices.md). Blocked on
external prerequisites.
Existing check: none.
Impact: lexical retrieval ships without host evidence.
Open questions:
- Host owners must supply the real OpenCode and Pi application capability and
  matrices. (needs human input)

## Relationship map

- `literal-probes-never-operate` and `zero-terms-run-no-match` bound what
  reaches the engine; every later record assumes the probes are the request's
  literal atoms and nothing else.
- `rank-then-occurrence-order` fixes the comparator that
  `eligibility-before-accepted-slots` judges in: a slot is taken in comparator
  order, so a wrong order drops a different eligible occurrence.
- `eligibility-before-accepted-slots` depends on `one-kernel-eligibility-policy`:
  the batch adapter it judges through is one of the allowed callers, and a
  second policy would make the slot verdicts diverge from the daemon's.
- `lexical-work-is-bounded-and-observed` and `original-budget-stops-sql`
  bound the same scan from two sides: the caller's bounds cap the work a
  completed request does, and the budget ends a request before it completes
  that work.
- `retrieval-reads-no-payload` and `no-alternative-lexical-index` pin the
  statements the scan may run: one FTS5 table, the occurrence join, and the
  tombstone check, and no payload read.
- `host-application-matrices` consumes the contributions every safety record
  shapes; it is the one record this part cannot discharge without host owners.
