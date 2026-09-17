# packing-adjustment-removes-last-admitted-within-the-cap

## Discovery trigger

RP2.8's packing contract says wrappers, separators, or escapes may push a
fitting selection over a bound; adjustment must remove lowest-priority optional
groups deterministically within the approved pass cap, reclaim their wrappers,
or return one typed failure without emitting over-budget bytes. The Q4 ruling
fixed the pass unit as a group and the order as last-admitted first.

## Evidence trail

- `crates/daemon/src/packing/serialize.rs` `finalize` loops while `serialize`
  reports a bound, pops the last admitted group into `removed`, and rebuilds
  the ledger through `rebuild` from the required render the admission carries,
  under each remaining group's partition index, so a removed group's `GroupOpen` and
  `GroupClose` entries are not re-appended and the remaining labels match the
  admission's.
- The loop returns `PackingFailure::AdjustmentCapExhausted` when the pass count
  reaches the cap or the admitted list is empty, before constructing any
  `PreparedOutput`.
- The loop polls the budget at the top of every pass and again after the
  fitting body is written and hashed, before returning the `Preparation`; a
  budget that ends in either window refuses with `PackingFailure::Deadline`.
  `guard_calls::on_write` (`crates/daemon/src/dispatch.rs`, test-support
  only) runs a hook at the start of every guard write, so a test can end the
  budget inside the second window.
- Only the serialized-bytes bound (and the guard's transport maximum) can
  start a pass. `prepare_optional` (`crates/daemon/src/packing/mod.rs:613`)
  refuses a closed render past a rendered-bytes or estimated-tokens bound as
  `PreparationRefusal::Accounting`, so `finalize` never sees one; the test
  `an_accounting_overflow_is_refused_by_the_optional_phase_before_any_measurement`
  drives both phases from one `AccountingBounds` and observes that refusal.
- A rebuild cannot re-enter an accounting bound the admission passed because
  the profile charges each fragment independently of what follows it: every
  fragment starts with `<` and ends with `\n`, which the exact tokenizer's
  pre-tokenizer splits on, and `AccountingProfile::heuristic` documents the
  same requirement for an estimator. The test
  `the_exact_tokenizer_charges_fragments_independently_so_a_rebuild_drops_only_the_removed_cost`
  checks that the exact count of the closed render equals the sum of its
  per-entry counts and that the repaired total is the full total less the
  removed group's cost.
- `crates/daemon/tests/packing_serialize.rs` checks the removed group's
  partition index, compares the repaired body and ledger with a fresh admission
  of the remaining groups, checks the total dropped by the removed group's
  charge, and compares the body after every optional group is removed with the
  required-only render.

## Failure scenario

An adjustment that trims bytes from the end of the render, or drops the group
with the smallest charge, makes the body depend on the shape of the overflow
and can split a wrapper from its content.

## Timing windows and dependencies

None.

## What a test must construct

- A closed render and a serialized-bytes limit one below its length.
- Each accounting bound one below the closed render, fed to both phases from
  one `AccountingBounds`, so the refusal is observed at the optional phase with
  the guard counters at `(0, 0)`.
- A limit below the required render, so every optional group is removed.
- A cap smaller than the number of passes the limit demands.
- A budget cancelled from the guard's write hook, so the poll after the write
  is the one that refuses.

## Investigation log

### Q: Can a rebuilt ledger exceed an accounting bound the admission passed?

- Sources examined: `crates/daemon/src/packing/serialize.rs` `rebuild` and
  `finalize`; `crates/daemon/src/packing/render.rs` `Ledger::delta`, the
  fragment constructors, and the totals; `crates/daemon/src/packing/accounting.rs`
  `AccountingProfile::heuristic`; `crates/tokenizer/src/lib.rs` `CLAUDE_PAT_STR`
  and `suffix_anchor`; `grep -rn 'AccountingProfile::' crates --include=*.rs`.
- Findings: a rebuilt ledger is the admitted ledger less the removed groups'
  entries plus a re-priced close, so its total is the admitted total less the
  removed costs only if the profile charges each fragment the same whatever
  follows it. Every fragment starts with `<` and ends with `\n`; the
  pre-tokenizer's punctuation class excludes whitespace and its whitespace
  class excludes `<`, so the exact tokenizer's pieces end at every seam and
  the count of the closed render equals the sum of its per-entry counts. The
  only production profile is `exact_tokenizer`; `heuristic` is constructed by
  tests. A constructed estimator that charges a shorter render more drives
  `finalize` to emit a body whose ledger total exceeds the bound the admission
  passed (326 admitted, 1326 after one rebuild); no shipped estimator does.
- Missing evidence: none for the production profile.
- Conclusion: resolved with answer - the seam additivity is the load-bearing
  fact; `AccountingProfile::heuristic` documents it as the estimator's
  requirement and
  `the_exact_tokenizer_charges_fragments_independently_so_a_rebuild_drops_only_the_removed_cost`
  pins it for the exact profile. `finalize` re-checks no accounting bound.

### Q: Does a budget that ends after the loop's poll still refuse?

- Sources examined: `finalize`'s loop, whose poll sits at the top of each
  pass; `serialize`, which copies the body through the guard; the SHA-256 pass
  over the body; `prepare_optional`'s poll after `ledger.close()`.
- Findings: before this change a budget that ended while the fitting body was
  written and hashed returned the preparation. A test-support hook in the
  guard's write (`guard_calls::on_write`) cancels the budget inside that
  window; the test observed `Ok` before the poll was added and `Deadline`
  after.
- Missing evidence: none.
- Conclusion: resolved with answer - the poll after the write and hash is the
  same rule the optional phase applies after pricing its close.
