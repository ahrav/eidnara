# rank-then-occurrence-order

## Discovery trigger

#391 AC2: scan lower raw FTS rank first, then the approved occurrence
comparator, and compare unequal ranks and exact ties with an independent
ordered reference. #391 AC4 supplies the reduction half of the record: reduce
each occurrence to its best ordinal probe rank, break ties by the comparator,
and keep the ranking unchanged under duplicated and permuted probes. The PR
967 description maps AC2 to this record. The RP2.3 property bundle is
unavailable here, so the catalog reconstructs the record from the ticket's
acceptance criteria and verifies it against the #391 branch.

## Evidence trail

Production code, `crates/retrieval/src/lexical/retrieve.rs`:

- `by_id` (`:280`) compares the eight-byte identifier prefix `Id.key`, then
  the full identifier bytes, so the tie-break is occurrence identifier order.
- `scan_ranked` (`:650`) keeps the request's best `scan_rows` matches across
  its ranked probes. `ranked_rows` (`:730`) reads each ranked probe in rowid
  order with the `rank` column; `merge_best` (`:697`) merges the probes' rowid
  streams and keeps one match per rowid, the lowest rank then the lowest
  ordinal; `select_nth_unstable_by` on `rank.total_cmp` (`:675`) partitions the
  merged matches at the bound, every group below the cut rank goes to
  `Kept::pending` in rank order (`:688`), the cut group is read through
  `identify` (`:759`), which sorts it by `by_id` then ordinal (`:811`), and
  `Kept::tail` is truncated at the bound, so identifiers decide which
  equal-rank matches the bound keeps. A dead row keeps its slot;
  `admit_batches` (`:1006`) drops it unjudged.
- `admit_batches` and `revalidate` (`:1109`) carry each accepted hit's
  `rank` and `ordinal` into `Contribution` unchanged.

Daemon path: `lexical_read` calls `scan` (`crates/daemon/src/query_route.rs:1113`)
and `admit_lexical` calls `admit` (`:1273`) under the same projection
connection, with `lexical_retrieval_bounds` (`:172`) from
`QueryRouteLimits::production` (`:137`).

Tests, `crates/retrieval/tests/lexical_retrieval.rs`, all on a projection
written through `apply_batch` (`Fixture::project`, `:378`) and a real
`KernelStore` (`Fixture::new`, `:268`), with `bounds()` (`:153`): `scan_rows`
64, `max_accepted` 64, `batch_rows` 2:

- `keyed_reference` (`:523`) is the oracle: `ranks` (`:556`) reads every
  match's `occurrence_id` and `rank` with a plain `MATCH`, keeps each
  identifier's minimum rank, and sorts by `total_cmp` then identifier.
- `:687` `contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation`
  runs `parse fetch io` on the six-row corpus, asserts `keyed` equals the
  reference, six contributions, non-decreasing ranks, equal keyed output for
  `io fetch parse` and `parse parse io fetch fetch parse`, `probes == 6` for
  the duplicated request, and gamma's ordinal 0, 2, 0 across the three runs.
- `:728` `equal_ranks_from_distinct_probes_keep_the_lowest_ordinal` confirms
  `note` and `unrelated` rank delta equally, asserts ordinal 0 under both
  probe orders, and equal keyed output.
- `:2418` `a_large_equal_rank_group_at_the_bound_keeps_the_lowest_identifiers`
  projects 3000 tied rows in reversed order over three batches, asserts the
  scan keeps exactly the 64 lowest identifiers, that the reference agrees,
  `ScanBound`, and `ranked_matches == 3000`. The tied rows fill the table, so
  `identify` reaches each kept rowid by stepping its cursor.
- `:2452` `a_tie_group_spread_thinly_through_the_table_keeps_the_lowest_identifiers`
  projects 240 tied rows among 3000 rows no probe matches, asserts a 240-row
  window identifies every tied row in identifier order and a 64-row window
  keeps exactly the 64 lowest identifiers, with `ScanBound`,
  `ranked_matches == 240`, and `scanned_rows == 64`. The kept rowids are a
  sparse subsequence of the table, so `identify` falls back to seeking each
  one.
- `:2970` `dead_rows_inside_a_distinct_rank_probe_keep_their_window_slots`
  projects 150 rows with distinct ranks, tombstones indexes 0..10 and 60..70
  through `tombstone_raw` (`:1028`), asserts the scan keeps the first 64
  reference rows whatever their liveness, and asserts the contributions are
  the 50 live ones among them, with `dead_rows == 14`, `scanned_rows == 64`,
  and `ScanBound`.
- `:3170` `a_dead_row_at_a_rank_change_opens_no_slot_for_the_next_rank_group`
  ends the bound at a rank change, stores the tail with its highest identifier
  first, tombstones the head's lowest identifier, and asserts the window is
  still the whole head with `dead_rows == 1` and `ScanBound`.

## Failure scenario

A comparator keyed on rowid or storage order yields a contribution order that
depends on projection insertion history. The same kernel state and the same
request return different top rows after a rebuild, so RP2.7 fusion receives a
different lexical ranking, and a probe permutation changes which occurrence
wins a tie. An occurrence hit by two probes with a dedup that keeps the later
hit reports the wrong rank and ordinal as provenance.

## Timing windows and dependencies

None at the comparator. The order is computed in memory after every probe has
run, and both sorts are total because identifier, rank, and ordinal together
are unique per hit. Two kept lexical rows may name one occurrence; admission
keeps the first through `reached` in `admit_batches` (`:1017`, `:1052`), as
`a_second_lexical_row_for_one_occurrence_yields_one_contribution`
(`lexical_retrieval.rs:3074`) pins.
The scan bound interacts with order: `scan_ranked` must finish a rank group
before deciding which equal-rank rows fit. A dead row inside the bound keeps
its slot, so liveness never moves the window.

## What a test must construct

- Occurrences hit by several probes with unequal ranks, so dedup must choose.
- Equal raw ranks from distinct probes, so the ordinal tie-break is observable.
- Duplicated and permuted probe lists against one fixed reference.
- An equal-rank group larger than `scan_rows`, stored in reversed identifier
  order, so rowid order and identifier order disagree at the bound.
- Dead rows at the front of and across the scan bound in a distinct-rank probe,
  written as raw tombstones so the engine still matches them and the window
  must keep their slots.
- An independent oracle reading `rank` through plain `MATCH` and sorting with
  `total_cmp` then identifier bytes.

## Investigation log

### Q: Do the cited tests exercise the production projection or a fixture engine?

- Sources examined: `Fixture::project` (`lexical_retrieval.rs:378`), which
  calls `apply_batch` on the opened `SqliteStore`; `Fixture::retrieve`
  (`:429`), which calls `retrieve` (`retrieve.rs:359`); `lexical_read`
  (`query_route.rs:1090`) and the `admit` call (`:1273`).
- Findings: the tests write rows through the retrieval projection write path
  and call the same `scan` and `admit` the daemon calls. They use `bounds()`
  with `scan_rows` 64 and `max_accepted` 64, while production uses 4096 and
  128 (`query_route.rs:137`). The tests call `retrieve` directly, so the
  daemon route and its limits are covered only through the parity of the
  shared functions.
- Missing evidence: no cited test runs the daemon route with production
  bounds against an equal-rank group at the 4096 bound.
- Conclusion: resolved with answer - projection, through the same `scan` and
  `admit` entry points the daemon uses, at test bounds.
