# Lens: existing test strategy, bug history, unproven assumptions, wildcard

## Existing test strategy

What the suite exercises (names are stable anchors; lines verified at HEAD):

- Pure packing: `crates/retrieval/tests/packing_required.rs:72`
  `every_verdict_maps_to_exactly_one_failure_class`, `:118`
  `stale_rows_and_bounds_are_checked_in_request_order`, `:224`
  `the_load_bound_is_checked_over_the_whole_set_before_any_request_fault`,
  `:253` `reservation_charges_every_byte_and_stops_exactly_at_the_limit`;
  `crates/retrieval/tests/packing_grouping.rs:352`
  `the_scan_skips_and_continues_and_the_prefix_packer_does_not`.
- Memory trim: `m0_compose.rs:300` `budget_boundaries_match_the_replaced_loop`
  (NaN, -3, 0, wrapper-1, wrapper, wrapper+10.5, +40, +40.5, +41, 1e30,
  +inf) and `:337` proptest `skip_and_continue_delegation_matches_the_replaced_loop`.
  Both use a byte-length estimator and compare with the retained loop.
- Snapshot: `canonical_memory.rs:282`, `:315`, `:373`, `:390`, `:412`, `:457`.
- Render: `memory_render.rs:619`, `:646`, `:670`, `:714`, `:737`.
- Transform unit: `transform.rs:13851`, `:13894`, `:13958`, `:17437`.
- Daemon integration: `crates/daemon/tests/transform_canonical_memory.rs:46`,
  `:241`, `:320`, `:356` over a real kernel store, including the unknown
  `claim_lane` field (`:206-214`) and a 500-token user budget (`:356-449`).
- Harness: `memory-mark-command.test.ts` (14 tests), Pi
  `eidnara-memory-mark.test.ts` (5), OpenCode `command-handler.test.ts:750-850`
  (4); `index-env-guard.test.ts:38, 54`; `index-in-process-latch.test.ts`;
  `hook.test.ts:1803` asserts `is_subagent: true` for a restored child.
- e2e: `compression-fidelity-memory.test.ts` (seeded, wrong-project,
  candidate, budget-excluded, quarantined, rejected variants);
  `compression-fidelity-pi.test.ts:155` C1 at P1 through Pi.

What no test asserts (suspiciously quiet):

1. A subagent pass (`is_subagent: true`) serving no `<project-memory>`.
   `transform.rs:11723` is the only guard and its tests concern cache TTL,
   emergency, and reasoning.
2. The additive-only path (`eidnara_folds = false`) rendering
   `<project-memory>`; the `eidnara_folds: false` tests at
   `transform.rs:16293`, `:16438`, `:16521`, `:16586` do not name the block.
3. Pi sending `is_subagent: false`; `pi-pass-inputs.test.ts` covers usage
   and completion time only.
4. Pi compaction-off registering no `context` handler (`index.ts:565`); the
   `compactionOff` tests concern commands and tools.
5. The trim cost model against the real tokenizer: every trim test uses
   `|text| text.len()`.
6. The TypeScript side of the 4,000-token default (`config.rs:30-31`).
7. The memory-mark Pi registration being absent from `subagent-entry.ts`.
8. `project_memory.truncated` staying `false` when the token budget drops a
   row (`:356-449` asserts rows and revision, not `truncated`).

## Bug history and density

`git log` over the S4 files shows performance and summarizer work
(`d696d0f6b`, `d0be19de9`, `32f46519e`, `5bd3d84e2`) and no fix commit
naming the memory block. Harness commits name Pi compaction handoff fixes
(`fe2c98930`, `416aa7374`, `7d89fe739`), all around `compactionOff` and
eviction, which is the same gate the plan's U6 reopens.

Tracker leads (read with `gh issue view`):

- #307 (closed): defines the m0 composition contract; acceptance rows for
  quarantine absence, `known_as_of`/`truncated` pinning, and the unknown
  `claim_lane` field. The integration test above is its executable form.
- #314 (closed): the host memory-mark command; confirmation exactly when
  derived visibility changes; idempotent under a client-derived key.
- #453 (closed as duplicate of #443): serving order `created_commit_seq`
  desc, `object_id` asc; row and payload-prefix caps before the token budget.
- #579, #657, #408, #709 (open, named by the brief): settle that model output
  stays Candidate and that delivery narrows and never admits; they bound
  N1..N5 and are not re-mined here.
- Tracker-wide searches for "memory injection", "project memory",
  "packing budget", "harness parity", "is_subagent", "memory-mark",
  "trim_memories", "canonical memory" returned no open bug or incident about
  the block, the trim, or the command runner. Nothing to promote.

## Unproven assumptions

- `DefaultHasher::new()` is deterministic across processes but its algorithm
  is unspecified across Rust releases; `revision` is persisted in
  `ModuleMeta.project_memory` (`memory-store/src/lib.rs:2356`) and compared
  across passes (`transform.rs:12436`). A toolchain upgrade may move every
  stored revision once. No comment acknowledges it.
- The trim estimates `<CAT>\n</CAT>\n` and `line\n` with the cached
  tokenizer while the block concatenates them differently; with a byte
  estimator the sums are byte-identical (worked out in evidence), with the
  tokenizer the block's count can differ at token boundaries. The code
  nowhere promises the rendered block fits the budget in tokens.
- The trim's `budget.floor() as u64 - wrapper as u64` cannot underflow
  because the guard `wrapper as f64 > budget` returns first; `+inf` floors
  to `u64::MAX` by saturation. Both are implicit.
- `memory_enabled` is read from `binding.config` at bind time; the comment
  at `lib.rs:5818` acknowledges a reload can pair `memory_enabled: true`
  with no read.
- The parity claim assumes both harnesses send the same request fields; the
  shared builder makes most fields identical, but `is_subagent` and
  `tool_present` differ by harness and `cwd` does not exist yet.

## Wildcard

- `extractM0` returns the first message text containing `<session-history>`
  and that same text carries `<project-memory>` because `compose_m0` appends
  the block to m0 (`m0_compose.rs:239-243`). A parity test that compares m0
  texts byte-for-byte across harnesses will also compare the history block
  and docs; equality of rule object ids needs a block-scoped extraction.
- `memorySeed` admits with `CodeObserved` on a `code_present` observation
  that shares the decision's lineage (`lib.rs:13877-13879`); that is the same
  exact-lineage trick the plan's `ConfigObserved` path relies on. S1 owns the
  policy claim; this pass only records that the fixture already depends on it.
- The budget-excluded e2e variant uses `h.ballast(12_000)` against the 4,000
  default; a 500-token user budget needs a child process with
  `XDG_CONFIG_HOME` (`transform_canonical_memory.rs:357-385`), which shows
  the budget is user-tier only and not request-supplied.
- `render_memory_block` is also called with the wrapper `"project-memory"`
  by `compose_additive_m0`; nothing checks that the wrapper is a valid
  element name (`memory_render.rs:154`), which is a precondition the new
  `<omitted-rules count="N"/>` line must also respect if it is rendered
  through the same function.
