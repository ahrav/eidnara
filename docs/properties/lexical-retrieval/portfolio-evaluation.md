# Lexical retrieval portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.3 for #391, during review of PR #967, by an evaluator that had not
taken part in the discovery. It was given `../METHOD.md`, `catalog.md`,
`existing-checks.md`, `fault-map.md`, the production code in
`crates/retrieval/src/lexical/retrieve.rs`, `compile.rs`,
`crates/retrieval/src/eligibility.rs`, and the lexical admission in
`crates/daemon/src/query_route.rs`, plus the cited tests in
`crates/retrieval/tests/lexical_retrieval.rs`, `lexical_engine.rs`,
`crates/daemon/tests/claim_eligibility.rs`, and `query_route.rs`. It did not
open `evidence/`. It ran
`cargo test -p retrieval --locked --test lexical_retrieval --test
lexical_engine` (`lexical_engine`: 15 passed; `lexical_retrieval`: 36 passed,
1 ignored, the `#[ignore]` scale gate
`lexical_scan_p99_at_one_million_occurrences`) and
`cargo test -p daemon --locked --test claim_eligibility` (2 passed). It
grepped `crates/*/src` for callers of `lexical::retrieve`,
`retrieve_with_hook_for_test`, `scan`, and `admit` from the lexical module:
`retrieve` has no production caller; the daemon reaches the lane through
`scan` (`crates/daemon/src/query_route.rs:1098`) and `admit` (`:1258`), and
`retrieve_with_hook_for_test` is exported only under the `test-support`
feature (`crates/retrieval/src/lexical/mod.rs:27`). Every `file:line`
reference in `catalog.md` and `existing-checks.md` was checked with `sed -n`
at `b3b9b7a80`, 92 in all; one is wrong (refinement 1). The disposition is
ours.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing against #391's AC1 to AC10.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 8 | applied to the catalog, existing-checks, and fault-map |
| gap | 3 | queued |
| bias | 4 | require human judgment, listed below |

## Refinements applied

1. **`existing-checks.md` cites a call site as the definition of `judge`.**
   In "Suspiciously quiet areas", replace "Every one funnels into the private
   `judge` (`:310`)" with "Every one funnels into the private `judge`
   (defined at `:198`, called at `:310`)". `crates/kernel/src/eligibility.rs:198`
   is `fn judge(`; `:310` is `verdict: judge(candidate, &facts, destination,
   in_scope),`.
2. **The catalog names no production entry point.** In `catalog.md`, after
   "A record exercised only by a fixture engine says so.", add: "The daemon
   reaches the lane through `scan` (`crates/daemon/src/query_route.rs:1098`)
   and `admit` (`:1258`), each under `QueryRouteLimits::lexical_retrieval_bounds`
   (`:170`); `retrieve` has no production caller, and
   `crates/retrieval/tests/lexical_retrieval.rs:607`
   `admitting_a_released_scan_equals_retrieve_and_carries_the_judged_candidate`
   is the bridge every `retrieve`-driven check rests on."
3. **`zero-terms-run-no-match` asserts a completion production never
   produces.** `lexical_read` returns `LaneStatus::Undeclared` when the
   analysis is empty (`crates/daemon/src/query_route.rs:1094`) before `scan`
   runs, so `Completion::Empty` is reached only by tests that call `scan` or
   `retrieve` with zero probes. Replace `Reachability: default-production`
   with `Reachability: default-production - the route's guard is the
   `Undeclared` return at `crates/daemon/src/query_route.rs:1094`;
   `Completion::Empty` itself is test-only`. Append to `Existing check:`
   "; `crates/daemon/tests/query_route_dense.rs:365`
   `a_request_without_prose_leaves_a_ready_dense_lane_undeclared_and_runs_no_producer`
   (a separator-only query is `InvalidQuery` at the route)".
4. **`retrieval-reads-no-payload` uses `unreachable` without a code
   location.** No payload read exists in `crates/retrieval/src/lexical/`;
   the readers are `fetch_payload` (`crates/retrieval/src/packing/mod.rs:269`)
   and `payload_lookup` (`crates/retrieval/src/lib.rs:606`), and the test's
   oracle is result equality under a renamed table, which METHOD classes as
   `always(!X)`. Replace the Check line with: "Check: `always` - the retrieval
   over a projection whose `payloads` table is renamed equals the retrieval
   before the rename, because a denied request must load no bytes and no
   payload statement exists in the lexical module to mark `unreachable`."
5. **`literal-probes-never-operate` overstates the engine witnesses.** The
   engine match sets (`lexical_engine.rs:196`, `:213`) cover `OR`, `NOT`, and
   `NEAR(a b)`; `*`, `"`, `^`, and column filters are covered only by analysis
   goldens (`lexical_analysis.rs:170`, `:236`), with no engine match set.
   Replace "request text holding `OR`, `NEAR`, `*`, `"`, parentheses, and
   column filters; a control that removes the quoting." with "request text
   holding `OR`, `NOT`, and `NEAR(...)` with engine match sets; `*`, `"`,
   `^`, and column filters at the analysis level only; a control that
   removes the quoting."
6. **`lexical-work-is-bounded-and-observed` credits no route-level check.**
   `existing-checks.md` lists `crates/daemon/tests/query_route.rs:320`
   `each_bound_saturates_before_its_protected_work` and `:532`
   `production_limits_are_the_d23_set`, which carry AC5's "missing approved
   limits fail closed" clause at the route. Append both to the record's
   `Existing check:` after `:2381`.
7. **`fault-map.md` routes nine of ten criteria.** The intro sentence skips
   AC9. After "and AC10 to the single index." add "AC9, the RP2.9 corpus run,
   has no record; see gap 2 in `portfolio-evaluation.md`."
8. **The catalog lacks the index table METHOD requires.** Insert before the
   first `###` heading:

   ```markdown
   ## Index

   | Slug | Type | Reachability | Semantics | Status | Confidence |
   | --- | --- | --- | --- | --- | --- |
   | [literal-probes-never-operate](#literal-probes-never-operate) | safety | default-production | always | active | high |
   | [zero-terms-run-no-match](#zero-terms-run-no-match) | safety | default-production | always | active | high |
   | [rank-then-occurrence-order](#rank-then-occurrence-order) | safety | default-production | always | active | high |
   | [eligibility-before-accepted-slots](#eligibility-before-accepted-slots) | safety | default-production | always | active | high |
   | [one-kernel-eligibility-policy](#one-kernel-eligibility-policy) | safety | default-production | always | active | high |
   | [lexical-work-is-bounded-and-observed](#lexical-work-is-bounded-and-observed) | safety | default-production | always | active | medium |
   | [original-budget-stops-sql](#original-budget-stops-sql) | safety | default-production | always | active | high |
   | [retrieval-reads-no-payload](#retrieval-reads-no-payload) | safety | default-production | always | active | high |
   | [no-alternative-lexical-index](#no-alternative-lexical-index) | safety | default-production | always | active | high |
   | [host-application-matrices](#host-application-matrices) | reachability | default-production | sometimes | active | low |

   ## Records
   ```

   The `retrieval-reads-no-payload` row assumes refinement 4; write
   `unreachable` there if it is declined.

## Gaps queued

1. **Post-admission revalidation has no record.** AC7's second sentence,
   "final canonical revalidation catches post-admission changes", is
   implemented by `revalidate` (`crates/retrieval/src/lexical/retrieve.rs`,
   called from `admit_inner`) and witnessed by five uncredited tests
   `existing-checks.md` already lists: `lexical_retrieval.rs:1150`, `:1173`,
   `:1203`, `:1243`, `:2170`. No catalog record owns them;
   `eligibility-before-accepted-slots` covers the pre-slot judgment and
   `one-kernel-eligibility-policy` the agreement, neither the re-judgment.
   Queue `accepted-set-is-revalidated-before-return` (`always`,
   default-production through `admit` at `query_route.rs:1258`): every
   returned contribution was judged eligible in the final batch, and a
   retirement, snapshot move, or incarnation change after admission drops or
   marks it with the recorded precedence.
2. **AC9 has no record and the scale gate is unanchored.** The RP2.9 corpus
   run is cited by no record; `lexical_scan_p99_at_one_million_occurrences`
   (`lexical_retrieval.rs:1724`) is `#[ignore]` with one failed `ubuntu-latest`
   run, and it measures `production_bounds()` (`:1705`), a hand copy of the
   D23 values. No check ties that copy to
   `QueryRouteLimits::production().lexical_retrieval_bounds()`
   (`crates/daemon/src/query_route.rs:135`, `:170`); `retrieval` cannot depend
   on `daemon`, so the equality needs a daemon-side test. Queue a
   `reachability` record for the D21 run (`sometimes`) and a daemon unit test
   asserting the six bound values equal the retrieval test's constants.
3. **AC6's cross-request isolation has no lexical witness.** "Late
   cancellation of A cannot interrupt healthy B" is checked for the dense lane
   (`crates/daemon/tests/query_route_compressed.rs:289`) and the storage
   handler (`crates/storage/src/lib.rs:5825`, `:5880`); the lexical evidence
   is the trailing unbounded run in `lexical_retrieval.rs:2049` after the
   handler is removed, which is one connection in sequence, not a late
   cancel against a running peer. Either list the dense and storage checks
   under `original-budget-stops-sql` with a sentence saying the lane shares
   the connection path, or add a lexical cell beside `:289`.

## Biases for a human

1. **`one-kernel-eligibility-policy` is a static property labelled as a
   runtime class.** The check reads source files; `default-production`
   describes no execution path. The scan also tracks `judge_eligibility` only,
   while the `judge_surface_eligibility` family (`crates/kernel/src/eligibility.rs:369`,
   `:381`, `:439`, `:467`) is called from three other files. Whether to widen
   the allowed list to those entries, or accept the private-`judge` funnel as
   sufficient, is a design decision.
2. **`host-application-matrices` carries a reachability label without
   evidence.** METHOD rule 4 requires the label's evidence per record; the
   record cites none for `default-production`, and `Exercised: not yet`. The
   evaluator cannot confirm a default install applies lexical contributions
   through both hosts. Whether the label stays, moves to `unresolved, needs
   host owners`, or the record moves to the host owners' part is a human
   call; AC8 and AC9 together are the acceptance criteria this part cannot
   discharge.
3. **One AC5 record holds deterministic counters and an unapproved
   allocation bound.** `lexical-work-is-bounded-and-observed` is `medium`
   because RP2.9 has approved no allocation bound, yet probes, rows, batches,
   and `sql_steps` are asserted exactly (`lexical_retrieval.rs:2381`).
   Splitting the allocation clause into its own `partial` record would let the
   counter record be `high`; keeping one record preserves AC5's shape.
4. **The ordering reference shares the engine.** `keyed_reference`
   (`lexical_retrieval.rs:478`) reads `rank` from the same FTS5 table with the
   same probes, then takes the minimum per occurrence and sorts. It is
   independent of `retrieve.rs`'s comparator and dedup code and cannot catch
   a shared misreading of the engine's rank sign. Whether the catalog should
   say "independent of the retriever, not of the engine" is a wording call.

## Semantics distribution

`always` 8, `always-or-unreached` 0, `sometimes` 1, `reachable` 0,
`unreachable` 1. With refinement 4 the split is `always` 9, `sometimes` 1.
The distribution is defensible: every lexical property is a safety claim over
a pure scan and a judged admission, both evaluated on every request, and the
one `sometimes` is the host campaign. The module has no optional path, so the
absence of `always-or-unreached` is correct.

## Verification

- Records: 10 `###` headings in `catalog.md`; 10 files in `evidence/`, one per
  slug, names equal. Index rows: 10 after refinement 8 was applied with the
  sibling parts' column set (slug, AC, type, check, exercised).
- Links: every `evidence/<slug>.md` link resolves; `existing-checks.md` and
  `fault-map.md` resolve; `portfolio-evaluation.md` resolves once this file
  lands. Both GitHub links are external and were not fetched for this check.
- Schema: all 12 fields present and in METHOD order in every record, checked
  by script.
- Line references: 92 `file:line` citations from `catalog.md` and
  `existing-checks.md` printed with `sed -n`; 91 land on the named function,
  constant, or statement. `crates/kernel/src/eligibility.rs:310` is a call
  site, not the definition (refinement 1).
- Reachability: `scan` and `admit` are called from
  `crates/daemon/src/query_route.rs:1098` and `:1258`; `retrieve` and
  `retrieve_with_hook_for_test` have no caller under `crates/*/src`. The
  `default-production` labels on the eight code-path records hold through
  `scan` and `admit`; refinement 3 narrows one, bias 1 and bias 2 question two.
- Test runs: `lexical_engine` 15 passed; `lexical_retrieval` 36 passed, 1
  ignored; `claim_eligibility` 2 passed; all `--locked`, debug profile.
