# served-rows-exclude-objects-invalidated-at-the-snapshot

## Discovery trigger

Portfolio evaluation gap G2 against the risk model "a stale, revoked, or
superseded rule is served": revocation is covered by the cascade record and
the reused `stale-projection-cannot-authorize-current-use`, but no record
named the registry-level exclusion of retired and superseded rows from a
served read. Lead L3 ("superseded is registry invalidation plus
`superseded_by`, not a disposition row") pointed at the mechanism.

## Evidence trail

Verified at HEAD `08de9051a`:

- `crates/kernel/src/admission.rs:3449` `served_classes_sql` builds the one
  statement every served-class read runs. Its `WHERE` clause at
  `:3543-3546` reads `o.created_commit_seq<=:governing_as_of AND
  (o.invalidated_commit_seq IS NULL OR :governing_as_of<o.invalidated_commit_seq)`.
- `admission.rs:3068` `visible_as_of_in_scope` and `:3104`
  `visible_selected_as_of` call `served_rows` (`:3379`), which calls
  `served_classes_selected` (`:3588`) with that SQL; Hidden rows are then
  dropped (`:3113`).
- `crates/daemon/src/kernel_routes/read.rs:180-182` `read_visible` calls
  `visible_as_of_in_scope` for both `RowSelection` variants.
- `crates/kernel/src/object_write.rs:90-110` `invalidate` sets
  `object_registry.invalidated_commit_seq` and the table row's
  `invalidated_commit_seq` to the committing sequence, refusing a second
  invalidation with `NotFound`.
- `crates/kernel/src/slice/write.rs:410-415` `correct_decision_inner`
  (the body of `supersede_decision`) calls `invalidate` on the replaced
  decision before inserting or folding the replacement, then
  `set_successor` at `:434-439` links `superseded_by`.
- `write.rs:543` `retire_decision` and `:549` `retire_decision_with_cascade`
  invalidate the decision and run the authority cascade.
- `crates/kernel/src/eligibility.rs:208-214` `judge` returns
  `Superseded` when `superseded_by` is set and `Retracted` when
  `invalidated_commit_seq` is set; this is the explicit-egress consumer of
  the same columns and is not on the m0 path.
- `crates/daemon/tests/transform_canonical_memory.rs:46` seeds
  `mem-retired` and `mem-old` (`:70-71`), retires the first and supersedes
  the second with `mem-new` at revision 2 in one envelope (`:118-131`),
  runs a pass, and asserts "Soon retired." and "Old architecture note." are
  absent from the served text (`:146-147`) while the block is present.

## Failure scenario

A user revises a directive; the route supersedes D1 with D2 at commit C.
The next pass pins `as_of >= C`. If the served read ignored
`invalidated_commit_seq`, both D1 and D2 would render under
`USER_DIRECTIVES`, the agent would see the old and new instruction side by
side, and R7's "the predecessor is superseded" would hold in the registry
but not in the context. The same shape applies to a repository rule
retired at a new HEAD (R8): the removed unit would keep rendering under
`PROJECT_RULES` with a stale `[dir]` label.

## Timing windows and dependencies

- The predicate and the row fetch run in one SQL statement inside one read
  snapshot; there is no window inside a read.
- Across reads, the window is the pass pinning: a pass pinned before C
  serves the old row by design and records `known_as_of < C`; the next
  pass drops it and the snapshot revision digest changes, which drives a
  HARD fold (`snapshot-revision-digests-only-rendered-rows`).
- Depends on `invalidate` being the only way a row leaves the live set;
  deletion is not a kernel operation on `object_registry`.

## What a test must construct

1. A bound project with one Visible `PROJECT_RULES` decision A and one
   Visible decision B on lineage L at revision 1 (the existing test's seed
   shape).
2. One envelope at commit C that calls `retire_decision(A)` and
   `supersede_decision(B, B2)` with B2 on lineage L at revision 2.
3. A `read_visible(AutoInject, C - 1)` read: A and B present, B2 absent.
4. A `read_visible(AutoInject, C)` read: A and B absent, B2 present, and
   `B.object.invalidated_commit_seq == Some(C)` when read through a
   snapshot before C.
5. The same two reads through a transform pass, asserting the served block
   text, as `transform_canonical_memory.rs:46` does for the later read.

Observation points: the `rows` array of the read response (fields
`invalidated_commit_seq`, `superseded_by`, `object_id`) and the served
`<project-memory>` text.

## Investigation log

### Q: Is the equality boundary (as_of equal to the invalidating commit) exclusive?

- Sources examined: `admission.rs:3545-3546` (strict `<`),
  `object_write.rs:90-100` (sets the sequence to the committing
  `commit_seq`), `transform_canonical_memory.rs:131` (reads at `tip` after
  the shaping commit, which is `>=` the invalidating sequence).
- Findings: the SQL excludes a row whose `invalidated_commit_seq` equals
  `governing_as_of`. The existing test reads at the tip after the commit,
  so it exercises `>=` but not equality in isolation.
- Missing evidence: a test that pins `as_of` exactly at the invalidating
  commit.
- Conclusion: resolved with answer from the SQL (exclusive at equality);
  unexercised as a dedicated check.

### Q: Does any served read bypass served_classes_sql?

- Sources examined: callers of `visible_as_of`, `visible_as_of_in_scope`,
  and `visible_domain_decisions_as_of_in_scope` in `admission.rs` and
  `crates/daemon/src`.
- Findings: all three route through `visible_selected_as_of` and
  `served_rows`; the explicit egress path uses `judge` on the same
  registry columns.
- Missing evidence: none for the m0 path.
- Conclusion: resolved with answer (no bypass on the m0 path).
