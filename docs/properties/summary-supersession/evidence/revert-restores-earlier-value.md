# revert-restores-earlier-value

## Discovery trigger

Specification [#835](https://github.com/ahrav/eidnara/issues/835), Property
Catalog record `revert-restores-earlier-value` (invariant,
default-production), derived at `265df096`; constraint C-3 (revert
truncation and recomp delete segment rows only, and nothing new cascades) and
decision D-11 (undo is a later message; argmax makes it live). The store half
belongs to the M1 ticket [#838](https://github.com/ahrav/eidnara/issues/838);
the render half to the M2 ticket
[#839](https://github.com/ahrav/eidnara/issues/839), whose acceptance
criterion is that after truncation to s1, s1 renders without a marker and,
after recomp, rendering is identity.

## Evidence trail

`crates/memory-store/src/lib.rs::MemoryStore::truncate_history_segments_for_revert`
and `crates/memory-store/src/lib.rs::MemoryStore::reset_session_for_recomp`
delete segment rows. Keyed claims live only in the `claims` column of
`history_segments` (`crates/memory-store/baseline.sql:83`, inside the table
opened at line 66), so a deleted row takes its claims with it. The
`note_eval_claims` table in the same file is an unrelated note-evaluation
lease table.

`crates/memory-store/src/lib.rs::tests::truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`
publishes s1 (`k.v = a`), s2 (`k.v = b`), and s3, truncates to sequence 1, and
asserts one row remains, that the only claim for `k.v` is s1's `a`, that the
revert epoch is 1, and that a repeated truncation is a no-op. After
`reset_session_for_recomp` it asserts no segment, and so no claim, remains.

`crates/daemon/src/m0_compose.rs::correction_compose_tests::revert_restores_the_earlier_value_and_recomp_renders_no_correction`
stores s1 (`we set k to a here`, claim `k.v = a` anchored on `set k to a`) and
s2 (`k.v = b`). Before truncation m0 holds
`## 1-1 · S1\nwe [corrected @2: k.v = b] here`. After truncation to 1, m0
holds `## 1-1 · S1\nwe set k to a here` and neither `[corrected` nor
`[corrections`. After recomp, m0 equals the m0 of an empty store.

`crates/daemon/src/decay_render.rs::render_rows` calls `corrections_for` on
every compose, so no correction set is cached across composes and none can
survive a revert epoch.

These checks run at the store and compose seams. No test drives a
transform-level revert that deletes a correcting row and then inspects the
served m0 and m1. That is a queued gap.

## Failure scenario

The user reverts the message that changed `k` from `a` to `b`. If the
correction were stored or cached, s1 would keep rendering
`[corrected @2: k.v = b]` after s2 is gone, and the served history would name
a value the user took back, citing an ordinal that no longer exists.

## Timing windows and dependencies

None within this record. Corrections are recomputed per compose from the rows
that exist. The ordering of the truncation commit against the transform's
terminal CAS is covered by `revert-truncate-commits-outside-the-terminal-cas`
and `revert-epoch-bumps-at-most-once-per-logical-recut` in
`../daemon/transform/catalog.md`; the serialized output cache side is covered
by `output-cache-replace-trails-the-accepted-commit` there.

## What a test must construct

Two rows sharing a key, with the older row's anchor present in its `p1`; a
compose showing the splice; a revert truncation to the older row; a compose
showing the original text; a recomp reset; and a comparison against an empty
store's m0. The store half checks the loaded claims directly. The queued
transform-level test reverts through the transform after a correcting row
and asserts on the served m0 and m1.

## Investigation log

### Q: Can any correction reference a deleted row after truncation?

- Sources examined: `crates/daemon/src/decay_render.rs::render_rows`,
  `crates/daemon/src/decay_render.rs::corrections_for`, the compose test
  above.
- Findings: corrections are derived from the loaded rows on each compose. A
  deleted row is not loaded, so its claims cannot be live and cannot supply a
  `live_value` or `live_ordinal`. The compose test observes no marker after
  truncation.
- Missing evidence: a transform-level revert that inspects served m0 and m1
  (queued gap).
- Conclusion: resolved with answer at the store and compose seams.

### Q: Does recomp leave any claim that could render?

- Sources examined: the store test and the compose test above.
- Findings: the store test asserts `load_history_segments` is empty after
  recomp; the compose test asserts m0 equals the empty store's m0.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the named checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon -p memory-store
  --all-features --locked` filtered to this part's tests, HEAD `c38af85a`.
- Findings: 17 tests ran and 17 passed, including
  `tests::truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`
  in `memory-store` and
  `m0_compose::correction_compose_tests::revert_restores_the_earlier_value_and_recomp_renders_no_correction`
  in `daemon`.
- Missing evidence: none.
- Conclusion: resolved with answer.
