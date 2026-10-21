# project-memory-truncated-flag-excludes-budget-trimming

## Discovery trigger

Lens: claimed guarantees C6 and the wildcard pass. The brief asked for "the
project_memory diagnostics shape in the transform response". The only
disclosure-like member today is `truncated`, and its doc says it is not
about the token budget. KTD5 and R11 add `rules_omitted`, the first
budget-pressure disclosure, which must stay distinct from this flag.

## Evidence trail

- `crates/memory-store/src/lib.rs:2168-2178`: `Canonical { known_as_of,
  truncated, revision }` with the comment "`truncated` records the kernel
  read's row or byte cap over the project's visible rows, not the token
  budget the block was trimmed to".
- Source of the flag: `crates/daemon/src/kernel_routes/read.rs:70-71`
  (`ReadResponse.truncated` doc), `:197` (`newest.finish()` over
  `MAX_READ_ROWS = 8192`, `:26`), `:229-234` (payload prefix past
  `MAX_READ_ROW_BYTES`, `:29`, sets `truncated = true`).
- Carried unchanged: `canonical_memory.rs:188-193` destructures `truncated`
  from the response; `:210` passes it to `new`; `:98-102` copies it into the
  composition. The trim (`m0_compose.rs:74-112`) returns a `Vec` and no
  count or flag.
- Response: `transform.rs:1486-1488` `TransformResponse.project_memory`;
  summarizer `:1611-1614`. Wire: `docs/host-wire-protocol.md:1032` lists
  `project_memory` among optional diagnostics without a shape.
- Tests: `tests/transform_canonical_memory.rs:356-449` drops a row by budget
  and asserts `kind: canonical` and revision stability, not `truncated`;
  `canonical_memory.rs:412` builds responses with `truncated: false` and
  asserts rows and revision; `:46` asserts `truncated: false` with no budget
  pressure.
- Tracker #307 acceptance: "`known_as_of` and `truncated` are pinned in the
  transform meta so absence is distinguishable from budget cutoff" (the
  distinction is read-cap versus budget, and the budget side has no field).

## Failure scenario

An operator sees `project_memory: {kind: "canonical", truncated: false}` and
concludes the block carries every injectable row, while the trim dropped a
rule that did not fit after newer memories. There is no number to read. If a
future change set `truncated` on budget drops, the kernel-cap meaning would
be lost and the two causes would be confused; the plan instead adds a new
member.

## Timing windows and dependencies

None; the flag is written once per served read.

## What a test must construct

1. Two Visible positive rows, a budget that fits one (user-tier
   `injection_budget_tokens` through the child-process pattern at
   `tests/transform_canonical_memory.rs:357-385`, or a unit `ReadResponse`
   with `truncated: false` and a 60-token budget); assert the response's
   `truncated == false` and one row rendered.
2. `MAX_READ_ROWS + 1` visible rows with an unbounded budget; assert
   `truncated == true` (expensive; alternatively a `ReadResponse` built with
   `truncated: true` through `injectable_snapshot`, asserting the flag
   survives the trim).
3. Coverage: `m0_budget_dropped_row` fires in case 1.

## Investigation log

### Q: Does any response member today count budget-dropped rows?

- Sources examined: `ProjectMemoryComposition` (`memory-store/src/lib.rs:2168-2181`),
  `TransformResponse` fields around `transform.rs:1480-1495`,
  `HistorySummarizerDiagnostics` (`:1605-1615`), `trim_memories_to_budget`.
- Findings: none. The trim's `Scan.skipped` is discarded at
  `m0_compose.rs:108-111`.
- Missing evidence: none.
- Conclusion: resolved with answer: no member exists; KTD5 adds the first.
