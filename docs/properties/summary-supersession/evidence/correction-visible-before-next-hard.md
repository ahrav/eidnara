# correction-visible-before-next-hard

## Discovery trigger

Specification [#835](https://github.com/ahrav/eidnara/issues/835), Property
Catalog record `correction-visible-before-next-hard` (reachability, bounded
liveness, default-production), derived at `265df096`; constraint C-4 and
decision D-8, the m1 delta. The M2 ticket
[#839](https://github.com/ahrav/eidnara/issues/839) implements the
`<memory-updates>` block and records decision A7, which lists every claim on
the new rows rather than only superseding ones. Open question Q3 of #835 and
the M3 ticket [#840](https://github.com/ahrav/eidnara/issues/840) call for the
50-fold burst measurement that decides whether a correction may force a HARD.

## Evidence trail

`crates/daemon/src/m1_compose.rs::compose_m1` loads the rows above the folded
sequence with `load_history_segments_above` and fills the `memory_updates`
slot with `render_memory_updates` over those rows.
`crates/daemon/src/m1_compose.rs::render_memory_updates` computes
`live_claims` over the loaded rows and emits one `MarkerForm::Entry` per claim
on them, in row and claim order, each carrying its key's live value and
ordinal, inside `<memory-updates>` after `PRECEDENCE_SENTENCE`. It returns an
empty string when the rows carry no claims.

`crates/daemon/src/m1_compose.rs::m1_revision_signal_timed` hashes
`max_history_segment_seq` into the in-session revision, so any new segment
changes the m1 revision and the next pass recomposes m1. That is the stated
bound of one m1 recompose.

`crates/daemon/src/transform.rs::tests::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`
asserts after the first correcting fold that both SOFT passes carry
`<memory-updates>`, the precedence sentence, and
`[corrections: db.port = 6543 @2]`. After the second fold the block reads
`[corrections: db.port = 7000 @3; db.port = 7000 @3]`, one entry per claim on
the new rows, both naming the live value.

`crates/daemon/src/m0_compose.rs::correction_compose_tests::m1_names_every_claim_on_its_rows_with_the_live_value`
composes m1 above folded sequence 1 over rows carrying `k.v = b`, `j.v = x`,
and a retraction of `k.v`. The block is exactly
`[corrections: k.v retracted @4; j.v = x @3; k.v retracted @4]`: `j.v`
supersedes nothing and still appears, and both `k.v` entries name the live
retraction. The folded row's text stays out of m1.

## Failure scenario

A correcting segment publishes after a HARD. m0 still serves the stale value
until the next HARD. If m1 omitted the block, or the block omitted the new
claim, the agent would act on the stale value for the whole window. If the m1
revision did not change on a new segment, m1 would not recompose and the block
would not appear until something else changed.

## Timing windows and dependencies

The window runs from the correcting publish to the next HARD. A natural HARD
fires when `crates/daemon/src/transform.rs::soft_pressure_refold` finds m1
above 20% of the history budget, or when more rows sit above the folded
sequence than `DEFAULT_M1_ROW_CAP` (259), which makes `compose_m1` return no
body. D-8 forces no HARD on a correction, so in a burst without an idle gap the
block grows with every fold and is billed uncached each publish.

## What a test must construct

A HARD; a fold producing a claim that supersedes one in m0; one SOFT pass; and
an assertion on the block content, including a claim that supersedes nothing,
two claims with one key, and a retraction. For the Q3 cost, a burst of folds
with no idle gap on a realistic fixture, recording block size per fold until a
natural HARD fires.

## Investigation log

### Q: Is the bound one m1 recompose rather than an unbounded "eventually"?

- Sources examined: `crates/daemon/src/m1_compose.rs::m1_revision_signal_timed`,
  the transform test above.
- Findings: the revision hash covers the maximum segment sequence, so a
  publish changes it. The transform test observes the block on the first SOFT
  pass after each publish.
- Missing evidence: none.
- Conclusion: resolved with answer; the bound is one m1 compose.

### Q: Q3, should a correction force a HARD when the m1 block grows?

- Sources examined: an uncommitted M3 burst driver at HEAD `c38af85a` over the
  388-row store-shape fixture, 50 folds with no idle gap, budget 60,000; the
  #840 criterion.
- Findings: one m0 re-freeze on the fixture costs 43,285 tokens. At 3 claims
  per fold, m1 exceeds 20% of the budget and a natural HARD fires at fold 45,
  with 35,865 cumulative block tokens; the block at fold 45 is 4,099 bytes,
  1,523 tokens, 135 entries. At the 8-claim cap the natural HARD fires at fold
  32, with 47,680 cumulative block tokens; the block is 7,630 bytes, 2,854
  tokens, 256 entries. The criterion is to force a HARD only when the block's
  cumulative uncached tokens exceed one m0 re-freeze before a natural HARD
  fires. It is not met at 3 claims per fold. At 8 claims per fold it is met by
  about 10%, and only in the last folds before the natural HARD.
- Missing evidence: owner confirmation. The driver and its raw report are not
  committed, so the numbers are not reproducible from the repository alone.
- Conclusion: needs human input. The recorded decision is no forced HARD, the
  D-8 default, proposed by the agent and pending owner confirmation. A forced
  HARD would change D-8 and this record's bound.

### Q: Do the named checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon -p memory-store
  --all-features --locked` filtered to this part's tests, HEAD `c38af85a`.
- Findings: 17 tests ran and 17 passed, including both tests above.
- Missing evidence: none.
- Conclusion: resolved with answer.
