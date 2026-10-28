# p5dep-daemon-bounds-rows-after-the-scope-filter

HEAD `3a3b916b7`. Surface S2.

## Discovery trigger

Plan U1 "Patterns to follow: `crates/daemon/src/kernel_routes/read.rs:185`
for the scope-filtered decision read." The daemon read is the only bounded
consumer of the unbounded Kernel API. Lens: resource boundaries; C6.

## Evidence trail

- `read.rs:26` `MAX_READ_ROWS: usize = 8192`, doc: rows encode to ~500 bytes
  before payload; memory surfaces render at most ~100 rows per query.
- `read.rs:29` `MAX_READ_ROW_BYTES = MAX_WIRE_BODY_BYTES / 8`; doc: redaction
  caps each decision text field at 512 KiB, so a worst-case row is ~1 MiB and
  at least eight always fit.
- `read.rs:99-125` `ServingOrderLast` ordering: older `created_commit_seq`
  ranks greater; among equal commits, greater `object_id` ranks greater; the
  heap's max is the row a sort-then-truncate drops first.
- `read.rs:127-161` `NewestRows`: `push` pops when `len > cap` and sets
  `dropped`; `finish` returns `into_sorted_vec` and `dropped`.
- `read.rs:163-249` `read_visible`:
  - `:170-188` Kernel read with `project.scope_term()`.
  - `:189-195` `ScopeFilter` then `NewestRows::new(MAX_READ_ROWS)`; the
    comment explains the bound cannot be a SQL `LIMIT`.
  - `:196-212` payload sizes read via `decision_payload_sizes_as_of` for the
    kept decision ids before any payload; comment: "a project can retain
    `MAX_READ_ROWS` decisions of ~1 MiB each, and materializing them all
    would allocate gigabytes".
  - `:213-219` a kept decision without a size entry is
    `CorruptCanonicalRow`.
  - `:220-234` cumulative cut where payload bytes pass
    `MAX_READ_ROW_BYTES`; `truncated = true` on cut.
  - `:235-243` `decisions_for_objects_as_of` for the retained ids; a missing
    row is `CorruptCanonicalRow`.
- `read.rs:66-76` `ReadResponse.truncated` doc: byte-budget truncation also
  happens at serialization (`:477-498`), so the wire flag can be `true` while
  this one is `false`.
- Tests: `crates/daemon/tests/kernel_routes.rs:1958`
  `a_read_over_the_row_cap_serves_the_newest_rows_and_flags_truncation`;
  `:2080` `bounded_selection_matches_a_full_sort_and_truncate`; `:2112`
  `a_read_over_the_byte_budget_serves_the_newest_rows_that_fit`.

## Failure scenario

If the heap were applied before the scope filter, a project with many
other-project rows that the SQL prefilter keeps (uncertain scopes) could
evict same-project rows before the filter judged them, under-serving the
bound project. If payloads were loaded before sizes, a project with thousands
of large decisions would allocate gigabytes per read. A P5 pass that copies
`read_visible`'s shape but omits the size step would inherit the second
failure.

## Timing windows and dependencies

Two Kernel reads at the same `known_as_of` (`visible.known_as_of`), so the
size map and the decision rows describe the same snapshot. The decision rows
are read after the cut, so a decision retired between the two reads at a
later commit is still read at the earlier snapshot.

## What a test must construct

- > 8192 same-project visible rows with interleaved commit sequences; assert
  the kept set equals sort-then-truncate (existing).
- Decisions whose sizes sum past `MAX_READ_ROW_BYTES`; assert the cut index
  and that `decisions_for_objects_as_of` was called with exactly the retained
  ids (an instrumented store or a count of ids in the call).
- A decision row deleted via raw SQL; assert `CorruptCanonicalRow`.

## Investigation log

### Q: Is `MAX_READ_ROWS` the right bound for a P5 pass?

- Sources examined: `read.rs:26` doc; plan OQ5-1.
- Findings: the bound was sized for wire responses and client rendering, not
  for a background scorer. The plan's cap is an open question.
- Missing evidence: the P5 author's choice.
- Conclusion: outside this record; see
  `p5dep-kernel-domain-read-has-no-row-bound`.
