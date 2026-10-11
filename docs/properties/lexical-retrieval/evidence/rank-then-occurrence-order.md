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

- `by_id` (`:236`) compares the eight-byte identifier prefix `Hit.key`, then
  the full identifier bytes, so the tie-break is occurrence identifier order.
- `best_first` (`:243`) orders by identifier, then `rank.total_cmp`, then probe
  ordinal.
- `comparator` (`:249`) orders by `rank.total_cmp`, then `by_id`.
- `scan` (`:349`) sorts hits with `best_first` (`:458`), runs `dedup_by` on
  `by_id` equality (`:459`) so each occurrence keeps its lowest rank and, among
  equal ranks, its lowest ordinal, then sorts with `comparator` (`:460`).
- `scan_probe` (`:594`) keeps each probe's best `scan_rows` live rows.
  `ranked_rows` (`:712`) reads a ranked probe in rowid order with the `rank`
  column and sorts in memory by rank then rowid. `scan_ranked` (`:649`)
  extends each window to the end of its rank group, `identify` (`:737`)
  fetches identifiers, and `resolve` (`:761`) offers rows in rank then
  identifier order, so identifiers decide which equal-rank rows the bound
  keeps. `Taken::offer` (`:885`) skips dead rows before any slot is taken.
- `admit_batches` (`:958`) and `revalidate` (`:1014`) carry each accepted hit's
  `rank` and `ordinal` into `Contribution` unchanged.

Daemon path: `lexical_read` calls `scan` (`crates/daemon/src/query_route.rs:1098`)
and the route calls `admit` (`:1258`) with `lexical_retrieval_bounds` (`:170`)
from `QueryRouteLimits::production` (`:135`).

Tests, `crates/retrieval/tests/lexical_retrieval.rs`, all on a projection
written through `apply_batch` (`Fixture::project`, `:346`) and a real
`KernelStore` (`Fixture::new`, `:253`), with `bounds()` (`:153`): `scan_rows`
64, `max_accepted` 64, `batch_rows` 2:

- `keyed_reference` (`:478`) is the oracle: `ranks` (`:511`) reads every
  match's `occurrence_id` and `rank` with a plain `MATCH`, keeps each
  identifier's minimum rank, and sorts by `total_cmp` then identifier.
- `:641` `contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation`
  runs `parse fetch io` on the six-row corpus, asserts `keyed` equals the
  reference, six contributions, non-decreasing ranks, equal keyed output for
  `io fetch parse` and `parse parse io fetch fetch parse`, `probes == 6` for
  the duplicated request, and gamma's ordinal 0, 2, 0 across the three runs.
- `:682` `equal_ranks_from_distinct_probes_keep_the_lowest_ordinal` confirms
  `note` and `unrelated` rank delta equally, asserts ordinal 0 under both
  probe orders, and equal keyed output.
- `:1950` `a_large_equal_rank_group_at_the_bound_keeps_the_lowest_identifiers`
  projects 3000 tied rows in reversed order over three batches, asserts the
  scan keeps exactly the 64 lowest identifiers, that the reference agrees,
  `ScanBound`, and `ranked_matches == 3000`.
- `:2470` `dead_rows_inside_a_distinct_rank_probe_neither_take_slots_nor_hide_truncation`
  projects 150 rows with distinct ranks, tombstones indexes 0..10 and 60..70
  through `tombstone_raw` (`:974`), asserts the scan keeps the first 64 live
  reference rows with `scanned_rows == 64` and `ScanBound`, then tombstones
  until exactly 64 live rows remain and asserts `Complete`.
- `:2605` `a_slot_a_dead_row_opens_goes_to_the_next_rank_groups_lowest_identifier`
  ends the bound at a rank change, stores the tail with its highest identifier
  first, tombstones the head's lowest identifier, and asserts the open slot
  goes to the tail's lowest identifier with `ScanBound`.

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
are unique per hit and identifiers are unique after dedup (comment at `:456`).
The scan bound interacts with order: `scan_ranked` must finish a rank group
before deciding which equal-rank rows fit, and a dead row inside the bound
opens a slot that the next group's lowest identifier takes.

## What a test must construct

- Occurrences hit by several probes with unequal ranks, so dedup must choose.
- Equal raw ranks from distinct probes, so the ordinal tie-break is observable.
- Duplicated and permuted probe lists against one fixed reference.
- An equal-rank group larger than `scan_rows`, stored in reversed identifier
  order, so rowid order and identifier order disagree at the bound.
- Dead rows at the front of and across the scan bound in a distinct-rank probe,
  written as raw tombstones so the engine still matches them.
- An independent oracle reading `rank` through plain `MATCH` and sorting with
  `total_cmp` then identifier bytes.

## Investigation log

### Q: Do the cited tests exercise the production projection or a fixture engine?

- Sources examined: `Fixture::project` (`lexical_retrieval.rs:346`), which
  calls `apply_batch` on the opened `SqliteStore`; `Fixture::retrieve`
  (`:397`), which calls `retrieve` (`retrieve.rs:317`); `lexical_read`
  (`query_route.rs:1098`) and the `admit` call (`:1258`).
- Findings: the tests write rows through the retrieval projection write path
  and call the same `scan` and `admit` the daemon calls. They use `bounds()`
  with `scan_rows` 64 and `max_accepted` 64, while production uses 4096 and
  128 (`query_route.rs:135`). The tests call `retrieve` directly, so the
  daemon route and its limits are covered only through the parity of the
  shared functions.
- Missing evidence: no cited test runs the daemon route with production
  bounds against an equal-rank group at the 4096 bound.
- Conclusion: resolved with answer - projection, through the same `scan` and
  `admit` entry points the daemon uses, at test bounds.
