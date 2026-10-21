# memory-trim-admits-in-serving-order-not-category-order

## Discovery trigger

Lens: architecture and data flow; claimed guarantees C5. The brief named
"trim_memories_to_budget newest-first behavior". Reading the trim showed it
preserves the supplied order and does not look at category rank; the
renderer regroups afterwards. The plan's problem frame ("a rule competes
with every other memory for the same 4,000 tokens") is this record.

## Evidence trail

- `crates/daemon/src/m0_compose.rs:70-73` doc: "Keeps the memories whose
  rendered lines fit `budget_tokens`, in the supplied serving order,
  charging each category's wrapper once. A row that does not fit is
  skipped, so a later smaller row can still be admitted."
- `m0_compose.rs:84-111`: `positive` is the input filtered by
  `is_positive_memory_category` in input order; `skip_and_continue` iterates
  `0..positive.len()`; admitted indices map back in order.
- `crates/retrieval/src/packing/scan.rs:18-43`: `skip_and_continue` visits
  every index once, `checked_sub` decides admit or skip, continues on skip.
- Supplied order: `crates/daemon/src/kernel_routes/read.rs:99` (heap order
  comment) and `:127` ("serving order (`created_commit_seq` descending,
  `object_id` breaking ties)"); `NewestRows::finish` at `:197`.
- Tracker #453: "Preserve ... serving order: descending creation commit
  sequence, then ascending object ID."
- Render order is separate: `crates/daemon/src/memory_render.rs:99-113`
  `memory_render_order` sorts by rank, then category name for the unranked
  tail, then `object_id`; `render_memory_block` at `:157-213`.
- Tests: `canonical_memory.rs:412`
  `rows_past_the_memory_budget_are_neither_injected_nor_digested` (newer
  oversized row skipped, older short row kept, both PROJECT_RULES);
  `m0_compose.rs:300`, `:337` (trim equals the retained loop over mixed
  ARCHITECTURE, NAMING, WARNING rows).
- Plan KTD5 changes this for rule rows only; optional rows keep this rule.

## Failure scenario

Two Visible rows: a NAMING row committed at seq 9 and a PROJECT_RULES row at
seq 3; a budget that fits one line. The trim keeps NAMING and skips
PROJECT_RULES. The block renders only NAMING. A user who recorded the rule
earlier sees it vanish when any later memory arrives, with no disclosure
(see `project-memory-truncated-flag-excludes-budget-trimming`).

## Timing windows and dependencies

Pure function; the only time-dependent input is the serving order's
`created_commit_seq`, fixed at commit. The estimator is
`token_cache::cached_estimate_tokens` in production and a byte counter in
every test.

## What a test must construct

1. `rows = [NAMING(seq 9), PROJECT_RULES(seq 3)]` in serving order, byte
   estimator, `budget = 34 + cost(NAMING line) + cost("<NAMING>\n</NAMING>\n")`.
   Assert `trim(...) == [NAMING]`.
2. Reverse the commit order so PROJECT_RULES is newer; assert
   `trim(...) == [PROJECT_RULES]`.
3. Assert the result is always a subsequence of the input (proptest).
4. Coverage: `m0_trim_recency_inversion_present` and
   `m0_trim_skipped_then_admitted` fire.

## Investigation log

### Q: Is the input order to the trim always the read's serving order?

- Sources examined: `canonical_memory.rs:195-210`; `read.rs:163-258`.
- Findings: `injectable_snapshot` iterates `rows` in the order `read_visible`
  returned; no sort occurs between the read and the trim.
- Missing evidence: none.
- Conclusion: resolved with answer: yes, newest first by
  `created_commit_seq`, then `object_id` ascending.
