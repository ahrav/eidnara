# apply-corrections-is-a-pure-function-of-body-and-corrections

## Discovery trigger

The #835 Property Catalog record of the same name, derived at `265df096`,
and C-10: rendering is a pure function of the loaded segment set and its
claims; no map iteration order, clock, or allocator state reaches the output.
The spec record asks for the same inputs in two processes with different
hasher seeds. The M2 ticket #839 carries the acceptance criterion that
`apply_corrections` yields identical bytes across processes with different
hasher seeds.

## Evidence trail

`crates/daemon/src/decay_render.rs::live_claims` returns a `BTreeMap` keyed
by claim key and fills it in slice order.

`crates/daemon/src/decay_render.rs::corrections_for` iterates the segment
slice and each segment's claim `Vec` and looks up the `BTreeMap` by key; it
never iterates the map.

`crates/daemon/src/decay_render.rs::apply_corrections` builds `Vec`s for
hits, degrees, the splice mask, kept hits, and footer entries. The kept hits
sort by `Reverse(hit.start)`; spliced hits are pairwise disjoint, so their
starts are distinct and the sort order is total. The footer follows slice
order.

`crates/daemon/src/decay_render.rs::correction_marker` and
`corrections_line` are `format!` and `join` over their arguments.

`crates/daemon/src/m1_compose.rs::render_memory_updates` iterates the row
slice and claim vectors and looks up `live_claims` by key.

A search of `decay_render.rs`, `m0_compose.rs`, and `m1_compose.rs` for
`HashMap`, `HashSet`, `Instant`, and `SystemTime` finds one `HashMap` in the
tier selection of `decay_render.rs` (`curve_index_by_original`), used only
through `insert` and `get`, never iterated; one `HashSet` in the
`trim_memories_tests` reference function of `m0_compose.rs`, test code only;
and one `Instant` in `m1_compose.rs::m1_revision_signal_timed`, which feeds a
timing field and not composed bytes.

Tests:

- `crates/daemon/src/decay_render.rs::tests::corrections_render_to_fixed_bytes`
  builds two rows (a spliced `db.port` correction, a missed `ui.mode`
  retraction footered) and asserts the literal output of
  `render_decayed_history_segments` over rows built with `corrections_for`.
- `crates/daemon/src/decay_render.rs::tests::render_golden_matches_reference`
  cases 7 and 8 in `crates/daemon/testdata/render-golden.json` hold literal
  corrected bytes, including a footer and a retraction.

Nextest runs each test in its own process, and the standard library seeds
`RandomState` per process, so each run of these tests is a fresh-seed
process checked against the same literal bytes. The record's Exercised
status is `partial`: each test process checks a literal-byte golden, and no
test runs two hasher seeds in one comparison.

## Failure scenario

A `HashMap` iterated to build footer entries or the m1 entry list orders
them by hasher seed. Two HARD passes over the same state then serve
different m0 bytes and miss the prompt cache, and replays in the transform
harness stop matching.

## Timing windows and dependencies

None. The claim covers the render path from loaded rows to bytes; the rows
themselves come from ordered SQL reads (`ORDER BY sequence`).

## What a test must construct

A fixed input with a splice, a footer, and a retraction, checked against
literal bytes rather than bytes computed by the production formatter; the
same check in processes with different hasher seeds.

## Investigation log

### Q: Is cross-process determinism shown by a two-process test?

- Sources examined: the `decay_render.rs` tests module, the
  `correction_compose_tests` module, the nextest process model.
- Findings: no test starts two processes with chosen hasher seeds and
  compares outputs. Evidence is literal-byte goldens checked in per-process
  test runs, each with its own `RandomState` seed, plus the source reading
  that finds no hash-map iteration on the path.
- Missing evidence: a test that fixes two distinct seeds and compares bytes.
- Conclusion: resolved with answer at the strength stated; a per-run
  literal-byte check fails if a seed-dependent order reaches the output,
  though a single run could pass by chance on a small input. Exercised
  stays `partial` until a test compares bytes under two seeds.

### Q: Can allocator state reach the output?

- Sources examined: `apply_corrections`, `correction_marker`,
  `corrections_line`, `render_one_history_segment`.
- Findings: no code reads a pointer value, capacity, or address into bytes.
  The `std::ptr::eq` check in the splice table tests the borrowed return, not
  output content.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon --all-features --locked --lib -E 'test(decay_render::) | test(correction) | test(every_loaded_claim)'`
  at `c38af85a`.
- Findings: 28 passed, including `corrections_render_to_fixed_bytes` and
  `render_golden_matches_reference`.
- Missing evidence: none.
- Conclusion: resolved with answer.
