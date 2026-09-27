# m0-bytes-change-only-at-hard

## Discovery trigger

Specification [#835](https://github.com/ahrav/eidnara/issues/835), Property
Catalog record `m0-bytes-change-only-at-hard` (invariant, default-production),
derived by the invariant-modeling pass at `265df096`, and constraint C-4:
m0 bytes change only when a HARD pass re-freezes them. D-6 places the
liveness pass once per compose, outside the tier computation and the
pressure-retry loop. The M2 ticket
[#839](https://github.com/ahrav/eidnara/issues/839) carries the acceptance
criterion that two SOFT passes after a correcting publish leave m0 unchanged
and that a HARD under budget pressure replays identical bytes.

## Evidence trail

`crates/daemon/src/decay_render.rs::render_rows` zips each stored row with
`corrections_for(segments)`, so corrections are computed once per call and
attached to the render row. Its doc comment states that every tier choice and
pressure retry renders the same set.

`crates/daemon/src/m0_compose.rs::compose_m0` calls `render_rows` on the
loaded rows and only then passes the rows to
`render_m0_with_decay_pressure_retry`, which re-renders at up to three more
pressure multipliers. The retry loop receives rows whose corrections are
already fixed; it never calls `corrections_for`.

`crates/daemon/src/transform.rs::tests::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`
seeds s0 (`db.port = 5432`), runs a first-fold HARD, and records m0. It then
publishes s1 (`db.port = 6543`), arms a SOFT refresh, and runs two passes;
each pass is SOFT and returns m0 bytes equal to the recorded m0. A second
fold publishes s2 (`db.port = 7000`); the next SOFT pass still returns the
recorded m0. A config change then forces a HARD, and that HARD's m0 holds
`the db [corrected @3: db.port = 7000]` and no longer holds `listens on 5432`.

`crates/daemon/src/m0_compose.rs::correction_compose_tests::a_hard_under_budget_pressure_renders_one_correction_set_and_replays`
stores two rows padded with 400 filler words. At budget 60,000 s1 renders at
tier 1 with `we [corrected @2: k.v = b] word`. At budget 500 s1 is demoted to
its dense tier, which lacks the anchor, so the correction becomes the footer
`[corrections: k.v = b @2]`. A second compose at budget 500 returns the same
bytes.

## Failure scenario

Two failures break the property. If liveness ran inside the tier computation
or the retry loop, a retry that demotes a row could compute a different
correction set than the first attempt, and the final bytes would depend on the
retry path rather than the state. If m0 recomposed on a SOFT pass, every
correcting publish would change m0 and bust the prompt cache on every fold
instead of at the next HARD.

## Timing windows and dependencies

The window is between two HARD passes. During it, superseding segments publish
and m1 recomposes, but the transform serves the frozen m0 unit. The property
depends on the transform serving that frozen unit on SOFT; this record does
not re-derive the freezing mechanism and relies on the transform test above
to observe it. Within one HARD, the dependency is that `render_rows` runs
before `render_m0_with_decay_pressure_retry`.

## What a test must construct

A HARD that freezes m0 over a row carrying a claim; a correcting publish
after it; at least two SOFT passes; a second fold before the next HARD; and a
forced HARD at the end. Separately, a compose under a budget small enough to
demote the stale row to a tier whose body lacks the anchor, composed twice
from one state. Both existing tests construct these conditions.

## Investigation log

### Q: Are the corrections fixed before the pressure-retry loop?

- Sources examined: `crates/daemon/src/m0_compose.rs::compose_m0`,
  `crates/daemon/src/m0_compose.rs::render_m0_with_decay_pressure_retry`,
  `crates/daemon/src/decay_render.rs::render_rows`.
- Findings: `compose_m0` binds `decay_history_segments = render_rows(..)` and
  passes that slice into the retry function; the retry closure calls
  `render_m0` with the same `history_segments`. No call to `corrections_for`
  exists inside the retry path.
- Missing evidence: none.
- Conclusion: resolved with answer; corrections are computed once per compose.

### Q: Does the transform test prove m0 is frozen on SOFT, not just unchanged by luck?

- Sources examined:
  `crates/daemon/src/transform.rs::tests::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`.
- Findings: after each correcting publish the test asserts the pass action
  starts with `SOFT` and that m0 equals the recorded bytes, while m1 carries
  the new value. A recompose on SOFT would splice `[corrected @2: ...]` into
  m0 and fail the equality. The final HARD shows the splice does appear once
  m0 recomposes.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the named checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon -p memory-store
  --all-features --locked` filtered to the tests this catalog part names, at
  HEAD `c38af85a`.
- Findings: 17 tests ran and 17 passed, including both tests above.
- Missing evidence: none.
- Conclusion: resolved with answer.
