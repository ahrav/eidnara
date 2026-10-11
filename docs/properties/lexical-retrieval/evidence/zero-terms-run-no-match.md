# zero-terms-run-no-match

## Discovery trigger

#391 AC1: literal probes, in its zero-term corner. A request that analyzes to
zero atoms compiles to no probe, runs no engine query, and returns no
candidates, with no broad fallback. The RP2.3 property bundle is unavailable
here, so the catalog reconstructs this record from the ticket's acceptance
criterion and verifies it against the #391 branch. The #967 PR description
maps AC1 to the catalog as met by existing tests.

## Evidence trail

Production code:

- `compile` (`crates/retrieval/src/lexical/compile.rs:23`) collects one probe
  per atom, so an analysis with zero atoms yields an empty `Vec<Probe>`.
- `Analysis::is_empty` (`crates/retrieval/src/lexical/analysis.rs:48`) is
  true when the atom list is empty; its doc comment on `:47` states the
  contract: a successful empty analysis compiles to no probe and the caller
  issues no MATCH for it.
- `scan` (`crates/retrieval/src/lexical/retrieve.rs:349`) initializes the
  result's completion on `:374` to `Completion::Empty` when `probes` is empty
  and `Completion::Complete` otherwise. The probe loop and the run loop that
  follow iterate zero times, so `count_probe`
  (`crates/retrieval/src/lexical/retrieve.rs:566`) and the scan statements
  run for no probe. `Consumed` stays at its default of zero probes, zero
  counted rows, zero scanned rows.
- `Completion::Empty` (`crates/retrieval/src/lexical/retrieve.rs:102`) is
  documented as "Zero probes: no MATCH ran and there is nothing to rank".
  `incomplete` (`:538`) lets only `BudgetExhausted` replace it, so a
  zero-probe result reports `Empty` rather than a scan or rank reason.
- `admit_inner` (`crates/retrieval/src/lexical/retrieve.rs:482`) judges the
  empty hit list in zero batches, so `batches` stays zero and `snapshot` and
  `incarnation` stay `None`.
- The daemon ends the lane before `scan`: `lexical_read`
  (`crates/daemon/src/query_route.rs:1075`) returns
  `LaneRead::Ended(LaneStatus::Undeclared)` on `:1094` when the analysis is
  empty, and `admit_lanes` (`crates/daemon/src/query_route.rs:1353`) refuses
  a request whose exact, lexical, and dense lanes are all undeclared with
  `InvalidQuery("the query yields no probe")` on `:1420`.

Tests cited by the record, read at the branch:

- `zero_probes_run_no_match_while_a_control_probe_contributes`
  (`crates/retrieval/tests/lexical_retrieval.rs:582`) runs `retrieve` over a
  projection from `Fixture::all_admitted` (`:317`) with `probes("!!! ...")`,
  where `probes` (`:164`) runs `analyze` then `compile`. It asserts
  `completion == Completion::Empty`, no contributions, `consumed.probes == 0`,
  `consumed.scanned_rows == 0`, `consumed.batches == 0`, and `snapshot ==
  None`. The control `probes("io")` asserts `Completion::Complete`, one
  contribution, one probe, one scanned row, two batches, and a snapshot.
- `zero_atoms_issue_no_probe_while_a_nonempty_control_completes`
  (`crates/retrieval/tests/lexical_engine.rs:268`) uses the scratch FTS5 table
  from `populated` (`:180`). It asserts `probe_matches` (`:80`) for
  `!!! ... ""` is an empty vector of probe results, and for the control `b`
  is `[[1, 9]]`. It then shows what the fallbacks would do: binding the
  expression `""` matches nothing, and binding the empty expression is an
  engine error, so neither empty form is a match-all.

Supporting tests outside the record's list:

- `zero_atoms_compile_to_no_probe`
  (`crates/retrieval/tests/lexical_analysis.rs:230`) asserts `compile` of
  `!!! ...` and of `analyze_segments(&[])` are both empty.
- `a_single_declared_lane_serves_and_no_lane_is_refused`
  (`crates/daemon/tests/query_route.rs:499`) runs the daemon route with the
  query `"   "` and asserts `Err(QueryFailure::InvalidQuery(_))`.

## Failure scenario

A request of punctuation or whitespace reaches the engine as an empty or
match-all expression, or the lane substitutes a table scan for a missing
probe. The request returns arbitrary occurrences the caller never named,
each of which then passes through eligibility and materialization as if
retrieved. Work counters report a scan the request did not ask for.

## Timing windows and dependencies

None. The zero-probe decision is made from `probes.is_empty()` before any
statement is prepared, and the daemon's `is_empty` check runs before `scan`.

## What a test must construct

- Separator-only request text (`!!! ...`), text with only empty quotes
  (`!!! ... ""`), the empty string, and whitespace (`"   "` at the daemon).
- A nonempty control request against the same store, so the zero result is
  shown to come from the request and not from an empty or broken index.
- A populated projection with admitted decisions, so the control produces a
  contribution with a kernel snapshot and the zero case's `None` snapshot is
  a discriminating assertion.
- For the engine-level fallback controls, a scratch FTS5 table and direct
  bindings of the quoted empty phrase `""` and of the empty string as the
  MATCH expression.

## Investigation log

### Q: Can a zero-probe request reach `scan` through the daemon, and does `scan` still hold the guarantee if it does?

- Sources examined: `crates/daemon/src/query_route.rs:1075` through `:1097`
  and `:1412` through `:1423`; `crates/retrieval/src/lexical/retrieve.rs:349`
  and `:374`; `crates/retrieval/tests/lexical_retrieval.rs:582`;
  `crates/daemon/tests/query_route.rs:499`.
- Findings: the daemon returns `Undeclared` on an empty analysis before
  calling `scan`, so production zero-term requests never reach the retrieval
  function. `scan` itself guards the case independently: with an empty slice
  it sets `Completion::Empty` and its two loops iterate zero times, which the
  `:582` test observes on the production `retrieve` function. The guarantee
  holds at both layers, each checked by its own test.
- Missing evidence: a daemon test asserting the lexical lane's status is
  `Undeclared` for a prose-only request of punctuation while another lane is
  declared; `:499` covers the all-lanes-undeclared refusal only.
- Conclusion: resolved; `scan` holds the guarantee on its own and the daemon
  ends the lane earlier, so the record is exercised on the production
  function against a projection and on a fixture engine.
