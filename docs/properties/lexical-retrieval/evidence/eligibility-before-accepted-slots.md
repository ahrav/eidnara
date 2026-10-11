# eligibility-before-accepted-slots

## Discovery trigger

#391 AC3: bounded canonical eligibility batches run before accepted-slot or
heap use, and with sufficient scan budget, eligible tails and fitting
equal-byte siblings survive ineligible leaders. #391 AC4 adds that distinct
equal-byte occurrences remain distinct. The PR 967 description maps the
equal-byte sibling test to AC3 and AC4. The RP2.3 property bundle is
unavailable here, so the catalog reconstructs the record from the ticket's
acceptance criteria and verifies it against the #391 branch.

## Evidence trail

Production code, `crates/retrieval/src/lexical/retrieve.rs`:

- `admit` (`:499`) and `admit_inner` (`:508`) take the `Scan` and run
  `admit_batches`, then `revalidate` (`:1108`), which re-judges the accepted
  set in one batch before any contribution is pushed.
- `admit_batches` (`:1005`) stops at `accepted.len() == max_accepted`
  (`:1021`) and otherwise takes at most `batch_rows` kept matches, sized from
  the open slots scaled by the matches read per acceptance so far (`:1033`)
  and, after a batch with no new acceptance, twice that batch,
  reads their occurrence rows through `resolve` (`:920`), and sends the live
  ones to `judge_batch` (`:971`) before reading any verdict. An `Eligible`
  verdict pushes to `accepted` only while a slot is open (`:1091`), otherwise
  it records `AcceptedBound`; `PolicyExcluded` goes to `consumed.exclude`.
  An ineligible leader therefore consumes judgment work and no slot.
- `judge_batch` (`:971`) calls `judge_tracked`
  (`crates/retrieval/src/eligibility.rs:296`), which calls the kernel's
  eligibility batch and reports a moved snapshot or incarnation.
- Kernel policy: `judge` (`crates/kernel/src/eligibility.rs:198`) returns
  `Hidden` when no read serves the object or the served visibility is
  `Hidden` (`:229`); `judge_eligibility` (`:341`) is the public batch entry.
  `MAX_ELIGIBILITY_CANDIDATES` is 1024 (`:21`), and `scan` refuses
  `max_accepted` or `batch_rows` above it with `BatchOverBound`
  (`retrieve.rs:152`).
- Each kept match is a distinct lexical rowid after `merge_best` (`:697`), and
  a lexical row names one occurrence, so equal-byte occurrences of distinct
  objects stay separate hits with bit-equal ranks under `rank.total_cmp`.

Daemon path: the route calls `admit` (`crates/daemon/src/query_route.rs:1259`)
with `validation_batch` 128 as `batch_rows` and `lexical_accepted` 128 as
`max_accepted` (`:135`, `:170`).

Tests, `crates/retrieval/tests/lexical_retrieval.rs`, on a projection written
through `apply_batch` (`Fixture::project`, `:378`) and a real `KernelStore`:

- `:762` `an_ineligible_leader_is_excluded_without_taking_an_accepted_slot`
  builds `Fixture::new` (`:268`) with every corpus object admitted except
  gamma, confirms the `parse` reference (`:483`) puts gamma first, and runs
  with `max_accepted` 1 and `batch_rows` 1. It asserts the one contribution is
  `reference[1]`, `AcceptedBound`, `excluded == [(Hidden, 1)]`, `judged == 3`
  and `batches == 3` (two admission batches plus one revalidation), and that
  default bounds return `reference[1..]` as `Complete`.
- `:797` `admission_batches_scale_the_open_slots_by_the_acceptance_so_far_and_an_exact_fill_stays_complete`
  uses the same fixture with `batch_rows` 4. With `max_accepted` 2 it asserts
  `reference[1..3]`, `AcceptedBound`, `judged == 4 + 2`, `batches == 2`, and
  `excluded == [(Hidden, 1)]`, so the fourth row in the batch is judged though
  no slot remains. With `max_accepted` 3 it asserts `Complete` and
  `reference[1..]`.
- `:2742` `equal_byte_siblings_stay_distinct_and_fill_past_an_ineligible_leader`
  projects three objects with identical text `sibling twin text`, sorts them
  by identifier, inserts the comparator's first as a decision with no
  admission so the kernel returns `Hidden`, and admits the other two through
  `Fixture::admit` (`:363`). With `max_accepted` 2 and `batch_rows` 1 it
  asserts, for `sibling`, `sibling sibling`, `twin sibling`, and `sibling
  twin`, that the contributions are the two admitted siblings in identifier
  order, `Complete`, `excluded == [(Hidden, 1)]`, and bit-equal ranks.

## Failure scenario

An implementation that reserves a slot per scanned hit before judging it, or
that stops at `max_accepted` scanned rows, fills the slots with hidden or
retired leaders and drops eligible occurrences behind them. The caller sees
fewer results than the kernel admits, and a hidden object's position decides
which eligible memory is returned. A dedup keyed on text bytes merges distinct
objects with identical text into one contribution, so one admitted memory
disappears.

## Timing windows and dependencies

Admission runs in kernel batches of at most `batch_rows`, each judged against one
kernel read snapshot. The accepted bound is checked before each batch
(`:947`) and per verdict inside a batch (`:999`), so a batch that crosses the
bound still judges its remaining rows. `revalidate` adds one batch over the
accepted set. A snapshot or incarnation move between batches stops admission
and is covered by other records. The equal-byte test uses one-row batches, so
the leader's verdict is known before the first sibling is judged.

## What a test must construct

- A kernel with one object inserted as a decision and never admitted, so the
  kernel's `judge` returns `Hidden` for it, while its siblings are admitted.
- The hidden object placed first in comparator order: lowest rank, or an exact
  rank tie with the lowest identifier.
- `batch_rows` of 1 so slot use and judgment are observable per row, and
  `batch_rows` larger than `max_accepted` so a bound inside a batch is
  observable.
- Equal-byte occurrences of distinct objects, so distinctness depends on the
  identifier rather than on text or rank bits.
- An ordered reference from `Fixture::reference` (`:515`) to name the expected
  survivors.
- Duplicated and permuted probe lists over the same rows.

## Investigation log

### Q: Does a cited test judge a hidden leader and its eligible tail in one chunk?

- Sources examined: `:797`, which uses `batch_rows` 4 over four `parse`
  matches with gamma hidden first; `admit_batches` (`:1005`) verdict loop.
- Findings: with `max_accepted` 2 the first batch holds gamma and the first
  eligible row, and the one slot left, scaled by one acceptance in two reads,
  sizes the second batch at the next two rows, so the tail fills two slots
  with the fourth judged past the bound. `:762` and `:2742` use one-row batches.
- Missing evidence: none for the fixture bounds. No cited test runs the
  daemon's production `batch_rows` 128 against a hidden leader.
- Conclusion: resolved with answer - yes, `:797` covers the in-batch case.

### Q: Is the leader hidden by the kernel or by the projection?

- Sources examined: `:2253` lines 2262 to 2268, which commit the leader as a
  decision without `record_admission`; kernel `judge` (`:198`, `:229`).
- Findings: the projection holds all three rows and the engine matches all
  three; only the kernel's served-visibility check excludes the leader, and
  `excluded == [(Hidden, 1)]` confirms the verdict source.
- Missing evidence: none.
- Conclusion: resolved with answer - the kernel, through canonical eligibility.
