# injectable-rows-require-visible-verdict-and-positive-decision-kind

## Discovery trigger

Lens: architecture and data flow; claimed guarantees. The brief asked what
happens to an `adr_accepted` row today, because the plan's directive
channel (KTD1) writes `decision_kind = adr_accepted` and KTD6 maps its
`source_kind` to the `USER_DIRECTIVES` heading. Reading the filter chain
showed the kind-based inclusion test is the only category oracle.

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:184-211` `injectable_snapshot`:
  `rows.iter().filter(visibility == Visible).filter_map(decisions.get(object_id))
  .filter(is_positive_memory_category(decision_kind))`, then the trim, then
  `CanonicalMemorySnapshot::new`.
- `canonical_memory.rs:175-183` doc comment: "a row is injectable when it is
  live ..., resolves visible on the surface, and carries a decision whose
  kind is a positive memory category. Labeled rows never reach
  `auto_inject`."
- `crates/daemon/src/memory_render.rs:67-80` `POSITIVE_MEMORY_CATEGORIES`:
  PROJECT_RULES, ARCHITECTURE, CONSTRAINTS, CONFIG_VALUES, NAMING,
  USER_DIRECTIVES, USER_PREFERENCES, CONFIG_DEFAULTS, ARCHITECTURE_DECISIONS,
  ENVIRONMENT, WORKFLOW_RULES, KNOWN_ISSUES. `adr_accepted` is absent.
- `memory_render.rs:82-84` `is_positive_memory_category` is array
  containment, exact and case-sensitive.
- `memory_render.rs:714-735` pins the array to
  `testdata/memory-category-vocabulary.json` `CATEGORY_PRIORITY` and requires
  every `V2_MEMORY_CATEGORIES` and `PROMOTABLE_CATEGORIES` entry to be
  positive.
- `crates/daemon/src/kernel_routes/read.rs:212-217`, `:250-253`: a visible
  decision-kind row with no typed decision at the snapshot fails the whole
  read with `CorruptCanonicalRow`, so the `filter_map` never silently drops
  such a row.
- Tests: `canonical_memory.rs:282` (Labeled, REJECTED_APPROACH, observation
  rows), `memory_render.rs:619`, `tests/transform_canonical_memory.rs:46`
  (`"memory"` kind, retired, superseded, quarantined, other project, other
  domain).
- Tracker #307 scope: "a row is injectable when its registry lifecycle is
  live ..., its `decision_kind` is in the existing positive-category
  vocabulary, and it resolves visible on the surface."

## Failure scenario

A Labeled row (candidate maturity, `explicit_labeled`) rendered into
`<project-memory>` would appear as an unlabeled rule, which #657 forbids.
A row with a forbidden kind rendered would put a rejected approach into the
affirmative block. Conversely, under the plan, a directive row committed as
`adr_accepted` through `directive.record` is dropped at
`canonical_memory.rs:199` until KTD6 changes the category derivation, so AE1
cannot pass against today's filter.

## Timing windows and dependencies

The visibility verdict is the kernel's at `known_as_of`; a disposition change
after the read lands on the next pass. The vocabulary is a compile-time
constant; a new kind needs a code change and a fixture change.

## What a test must construct

1. A `ReadResponse` with a Visible row whose decision kind is
   `adr_accepted`; assert the snapshot rows exclude it and the revision
   equals the snapshot without it.
2. The same with `USER_DIRECTIVES`; assert inclusion and rendering after the
   five ranked categories.
3. Integration: `commit_verified_memory(..., "adr_accepted", ...)` beside a
   `PROJECT_RULES` row; assert the served text omits the first.
4. Coverage: `m0_injectable_row_dropped_by_kind` and
   `m0_labeled_row_in_positive_kind_present` fire.

## Investigation log

### Q: Does the frozen vocabulary fixture stay the inclusion oracle under KTD6?

- Sources examined: `memory_render.rs:714-735`; plan KTD6 and U2 step 6.
- Findings: today category equals `decision_kind` for every row, and the
  fixture test gates kinds. KTD6 derives the category from `source_kind`
  for directives, so `adr_accepted` rows render under a heading that is not
  their kind; the fixture test has no row of that shape.
- Missing evidence: the implementation choice.
- Conclusion: needs human input.
