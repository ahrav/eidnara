# snapshot-revision-digests-only-rendered-rows

## Discovery trigger

Lens: state and persistence; claimed guarantees C3. The brief named
"CanonicalMemorySnapshot::new revision digest inputs". KTD5 and R11 add the
omitted-rules count `N` to this digest so that a change in what was omitted
refolds m0, which makes the current input set and its trigger path the
dependency to catalog.

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:42-49` doc: digest "in object-id
  order after the renderer's positive-category filter ... Each row
  contributes its category and the exact line `render_memory_line` emits ...
  `truncated` is not digested."
- `:50-67` `new`: filter positive, sort by `object_id`, `DefaultHasher::new()`,
  hash literal `"eidnara-project-memory-revision-v1"`, then per row
  `category.hash`, `render_memory_line(row).hash`; `known_as_of` and
  `truncated` stored but not hashed.
- Rows are already budget-trimmed when `new` runs (`:206-210`), so
  budget-dropped rows are not digested.
- Consumers: `CanonicalMemoryRead::revision` (`:118-123`);
  `ProducerContext::project_memory_revision` (`transform.rs:594-598`);
  `m1_revision_signal_timed` (`crates/daemon/src/m1_compose.rs:61-92`)
  hashes `memory_enabled.then_some(revision).flatten()` into
  `external_revision`; `apply_additive_only` and the fold path compare it
  with `loaded.meta.m1_external_revision` when `initialized` and non-zero
  (`transform.rs:2907-2909`); `MaterializeReason::ProjectMemoryEpoch`
  (`:12436-12437`).
- Durable: `ProjectMemoryComposition::Canonical { revision }`
  (`crates/memory-store/src/lib.rs:2168-2178`) in `ModuleMeta.project_memory`
  (`:2356`) and on the response (`transform.rs:1488`).
- Tests: `canonical_memory.rs:315`, `:373`, `:390`, `:412`;
  `transform.rs:13958` (revision 5 then 9 with same rows: no HARD; new text:
  HARD with `project_memory_epoch`); `tests/transform_canonical_memory.rs:46`
  (quarantine moves it, third pass keeps the frozen m0), `:356` (edit past
  the budget holds it).

## Failure scenario

A digest that included `known_as_of` would refold m0 on every kernel commit,
busting the provider cache each turn. A digest that missed a rendered input
(a renamed category, a changed line renderer) would keep a stale block
frozen while the rendered bytes should have changed. Under KTD5, a digest
that omits `N` keeps m0 frozen when the set of served rules is unchanged but
the number of omitted rules moved, so the disclosure line goes stale.

## Timing windows and dependencies

One digest per read; two passes differ only through a commit between their
reads. `DefaultHasher::new()` uses fixed keys, so the value is stable within
one toolchain; std documents the algorithm as unspecified across releases.
The persisted `m1_external_revision` is compared across daemon restarts and
upgrades.

## What a test must construct

1. Two snapshots over the same rows with different `known_as_of` and
   `truncated`, and a permuted row order; assert equal revisions.
2. Add a `REJECTED_APPROACH` row and a budget-dropped row; assert equal.
3. Edit one byte inside the 64 KiB cut; assert different. Edit past the cut;
   assert equal.
4. Transform: freeze a HARD, change one row's text, assert `action: "HARD"`
   and `materialize_reason: "project_memory_epoch"`; same rows at a later
   `known_as_of`, assert not HARD.
5. Coverage: `m0_revision_compared_against_frozen` fires (requires a prior
   HARD so `m1_external_revision != 0`).

## Investigation log

### Q: Is a one-time refold on a toolchain upgrade acceptable, or should the hasher be pinned?

- Sources examined: `canonical_memory.rs:55-57`; `m1_compose.rs:76-84`;
  `memory-store/src/lib.rs:2356`; std `DefaultHasher::new` documentation
  ("The internal algorithm is not specified, and so it and its hashes should
  not be relied upon over releases").
- Findings: the revision is persisted and compared across versions; a hasher
  change moves every stored revision once, requesting one HARD per session
  with `project_memory_epoch`. No comment acknowledges this.
- Missing evidence: a product decision on acceptable refold cost.
- Conclusion: needs human input.

### Q: Should the domain tag move to `-v2` when `N` joins the digest?

- Sources examined: `canonical_memory.rs:57`; plan KTD5, R11;
  `crates/retrieval/AGENTS.md` ("a new domain tag for every digest whose
  bytes change" is that crate's rule for fusion digests).
- Findings: the tag names a byte layout; the retrieval crate's convention
  rotates tags on layout change; the daemon has no stated convention.
- Missing evidence: the daemon's convention.
- Conclusion: needs human input.
