# p5dep-daemon-bounds-rows-after-the-scope-filter

HEAD `3a3b916b7`. Surface S2.

## Discovery trigger

Plan U1 "Patterns to follow: `crates/daemon/src/kernel_routes/read.rs:418`
for the scope-filtered decision read." (the plan cited line 185; the call is
at line 418 now). In the daemon read, `read_visible_page` is the only bounded
consumer of the unbounded Kernel API; `newest_page` reads
`visible_chunk_as_of_in_scope`, which ends a chunk by its row and payload
arguments. Lens: resource boundaries; C6.

## Evidence trail

- `read.rs:26-27` `MAX_READ_ROWS: usize = 8192`, doc: rows encode to ~500 bytes
  before payload; memory surfaces render at most ~100 rows per query.
- `read.rs:29-30` `MAX_READ_ROW_BYTES = MAX_WIRE_BODY_BYTES / 8`; doc: redaction
  caps each decision text field at 512 KiB, so a worst-case row is ~1 MiB and
  at least eight always fit.
- `read.rs:119-145` `ServingOrderLast` ordering: older `created_commit_seq`
  ranks greater; among equal commits, greater `object_id` ranks greater; the
  heap's max is the row a sort-then-truncate drops first.
- `read.rs:147-181` `NewestRows`: `push` pops when `len > cap` and sets
  `dropped`; `finish` returns `into_sorted_vec` and `dropped`.
- `read.rs:383-496` `read_visible`, which delegates to `read_visible_page`,
  the path of the in-process callers and of a `kernel.read` request with
  `object_ids` (`:671-675`):
  - `:403-424` Kernel read with `project.scope_term()`.
  - `:412-413` and `:425-434` build `ScopeFilter` and
    `NewestRows::new(MAX_READ_ROWS)`, then push each row that passes
    `selection.keeps`, the `before` cursor (`ServingPosition::precedes`,
    `admission.rs:234-238`, reached through `ReadCursor::position`,
    `read.rs:79-84`; set by `kernel.read` alone), and
    `ScopeFilter::matches`, in that order; the comment explains the bound
    cannot be a SQL `LIMIT`.
  - `:436-445` payload sizes read via `decision_payload_sizes_as_of` for the
    kept decision ids before any payload; comment: "a project can retain
    `MAX_READ_ROWS` decisions of ~1 MiB each, and materializing them all
    would allocate gigabytes".
  - `:446-455` a kept decision without a size entry is
    `CorruptCanonicalRow`.
  - `:456-472` cumulative cut where payload bytes pass
    `MAX_READ_ROW_BYTES`; `truncated = true` on cut.
  - `:473-488` `decisions_for_objects_as_of` for the retained ids; a missing
    row is `CorruptCanonicalRow`.
- `read.rs:289-381` `newest_page`, the path of a `kernel.read` request with
  no `object_ids` (`:676`):
  - `:310-319` each chunk is one `visible_chunk_as_of_in_scope` call with
    `project.scope_term()` and the cursor position, asking for at least
    `MIN_CHUNK_ROWS` (1,024, `:278`) rows and `MIN_CHUNK_PAYLOAD_BYTES`
    (1 MiB, `:283`) of payload.
  - `:328-334` each chunk row passes `ScopeFilter::matches` before it counts
    toward the cap; the page stops with `truncated` when a row passes the
    filter after `MAX_READ_ROWS` rows are kept. The comment at `:302`
    explains the bound cannot be a SQL `LIMIT`.
  - `:336-348` a decision object whose chunk row carries no decision is
    `CorruptCanonicalRow`; the page stops with `truncated` where cumulative
    `payload_bytes` pass `MAX_READ_ROW_BYTES`; a decision whose stored
    payload the page cannot serve as stored is queued for a read by id.
  - `:351-355` the next chunk follows the last row of the current one until
    a chunk reports `exhausted`.
  - `:357-374` `decisions_for_objects_as_of` for the queued ids; a missing
    row is `CorruptCanonicalRow`.
- `admission.rs:3249-3276` `visible_chunk_as_of_in_scope` and `:3626-3723`
  `served_chunk`: a chunk walks commits newest first from the cursor's
  commit and ends at the first commit boundary that holds `min_rows` rows or
  passes `payload_budget` bytes (`:3689-3698`). Each decision object carries
  its payload size and, within the budget and up to 64 KiB
  (`PAGE_PAYLOAD_INLINE_BYTES`, `:3010`), its kind and stored payload
  (`:3729-3761`).
- `read.rs:87-97` `ReadResponse.truncated` doc: byte-budget truncation also
  happens at serialization (`encode_page`, `:585-617`), so the wire flag can
  be `true` while this one is `false`.
- Tests: `crates/daemon/tests/kernel_routes.rs:1958`
  `a_read_over_the_row_cap_serves_the_newest_rows_and_flags_truncation`;
  `:2080` `bounded_selection_matches_a_full_sort_and_truncate`; `:2112`
  `a_read_over_the_byte_budget_serves_the_newest_rows_that_fit`; `:6973`
  `paging_with_before_serves_every_row_of_a_surface_over_the_row_cap_once`
  (the row bound with a `before` cursor); `:7106`
  `a_read_continues_past_the_rows_its_scope_filter_drops`; `:7174`
  `a_page_serves_the_decisions_its_chunk_carried_by_size_alone`; `:7225`
  `the_row_cap_serves_a_surface_of_its_size_whole_and_flags_one_row_more`;
  `:7273`
  `a_read_fails_closed_for_a_decision_object_without_a_live_decision_row`;
  `:7414` `a_full_page_reads_later_chunks_to_decide_truncation`.
  The route tests' over-cap and over-budget reads carry no `object_ids` and
  so run `newest_page`;
  `:2080` checks the `NewestRows` heap that `read_visible_page` uses, and
  `crates/daemon/src/kernel_routes/read.rs:1025`
  `a_domain_selection_over_the_row_cap_keeps_the_newest_rows` puts one row
  more than the cap through `read_visible`.

## Failure scenario

If the heap were applied before the scope filter, a project with many
other-project rows that the SQL prefilter keeps (uncertain scopes) could
evict same-project rows before the filter judged them, under-serving the
bound project. If payloads were loaded before sizes, a project with thousands
of large decisions would allocate gigabytes per read. A P5 pass that copies
`read_visible`'s shape but omits the size step would inherit the second
failure. On the `newest_page` path the first failure is a cap counted on
chunk rows before the filter judges them, and the second is a chunk that
holds every payload it reads instead of the payloads within its budget.

## Timing windows and dependencies

`read_visible_page` follows its row read with two Kernel reads at the same
`known_as_of` (`visible.known_as_of`, `read.rs:443`, `:479`), so the size map
and the decision rows describe the same snapshot. The decision rows are read
after the cut, so a decision retired between the two reads at a later commit
is still read at the earlier snapshot. `newest_page` reads one chunk or more
at one `requested` snapshot (`read.rs:296-299`, `:313`), each chunk in its
own read transaction, and then reads by id at the same `requested`
(`read.rs:364`) the decisions whose stored payload it cannot serve as stored.
A decision retired at a later commit between two of those reads is still
read at `requested`.

## What a test must construct

- > 8192 same-project visible rows with interleaved commit sequences; assert
  the kept set equals sort-then-truncate (existing for `newest_page` through
  the route tests, for `NewestRows` alone, and for `read_visible_page`
  through `read_visible` at
  `crates/daemon/src/kernel_routes/read.rs:1025`).
- Decisions whose sizes sum past `MAX_READ_ROW_BYTES`; assert the cut index
  and that `decisions_for_objects_as_of` was called with exactly the retained
  ids on `read_visible_page`, and with only the retained decisions the page
  cannot serve as stored on `newest_page` (an instrumented store or a count
  of ids in the call).
- A decision row deleted via raw SQL; assert `CorruptCanonicalRow` (existing
  for `newest_page`: `crates/daemon/tests/kernel_routes.rs:7273` ends the
  decision row at the tip).

## Investigation log

### Q: Is `MAX_READ_ROWS` the right bound for a P5 pass?

- Sources examined: `read.rs:26` doc; plan OQ5-1.
- Findings: the bound was sized for wire responses and client rendering, not
  for a background scorer. The plan's cap is an open question.
- Missing evidence: the P5 author's choice.
- Conclusion: outside this record; see
  `p5dep-kernel-domain-read-has-no-row-bound`.
