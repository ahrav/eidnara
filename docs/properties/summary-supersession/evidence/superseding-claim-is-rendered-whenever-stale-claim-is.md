# superseding-claim-is-rendered-whenever-stale-claim-is

## Discovery trigger

The #835 Property Catalog record of the same name, derived at `265df096`,
and C-14: because the loaded set R is the newest suffix of non-legacy rows,
any segment newer than a loaded stale segment is itself loaded, so a
superseding claim is in R whenever the stale claim is. R means loaded, not
rendered with a body. The M2 ticket #839 lists the record with a trivial test
while R was every row; the M3 ticket #840 carries the suffix-construction
test over #826's bounded R, with the acceptance criterion that for every
rendered claim the store-wide argmax equals the argmax over R.

## Evidence trail

`crates/memory-store/src/lib.rs::load_history_segment_fold` reads the newest
`pressure_window` non-legacy rows by `sequence DESC`, then, when the horizon
asks for more and the window was full, the next older non-legacy rows below
the last one read, again by `sequence DESC`. Both reads take a contiguous
newest run of non-legacy rows. It then adds legacy rows whose sequences are in
`legacy_seqs` and sorts by sequence.

`crates/memory-store/src/lib.rs::load_history_segments_above` reads rows with
`sequence > after_sequence` by `sequence DESC LIMIT cap + 1`, legacy or not,
and reverses them. When the result overflows the cap, `compose_m1` withholds
the body (`body` is `None`), so a non-suffix m1 set never renders.

`crates/daemon/src/m0_compose.rs::compose_m0` and
`crates/daemon/src/m1_compose.rs::compose_m1` pass exactly these rows to
`crates/daemon/src/decay_render.rs::render_rows`, and m1 passes them to
`render_memory_updates`. Neither makes another segment read.

Legacy rows carry no claims. The store enforces this since commit
`0231f2f4`: `crates/memory-store/src/lib.rs::prepare_history_segment` skips
`prepare_claims` when `legacy == 1`, and
`crates/memory-store/src/lib.rs::tests::a_legacy_row_stores_no_claims`
checks it. The publish path also never produces one:
`crates/daemon/src/history_summarizer.rs` sets `legacy` to 1 only when `p1`
is empty, and strict validation rejects tierless output. R is the newest
non-legacy rows plus every legacy row, so R holds claims only on its newest
non-legacy suffix.

Test:
`crates/daemon/src/m0_compose.rs::bounded_read_tests::every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set`
seeds `SyntheticHistory::mixed(4_000).with_claims(2, ClaimRegime::EveryThirdMessage)`,
which places legacy rows at distances 3, 120, 400, 2,600, and 2,900 and draws
claim keys from 64 keys so they recur. It computes `live_claims` over every
row, then over the m0 fold at budgets 20, 60,000, and 10,000,000 and over m1's
rows above folded sequences 3,999, 3,900, and 3,741. For each set it asserts
the set is non-empty and strictly smaller than the store, that it holds
claims, and that every loaded claim's `live_claims` entry over R equals the
store-wide entry. Commit `1c30058c` adds an assertion that no legacy row in
the session carries claims, and a negative control: the m0 fold at budget
60,000 with its newest claimed row removed fails the property.

## Failure scenario

A read that returns an older row but skips a newer middle row, for example a
sampled or importance-filtered read, loads a stale claim without its
corrector. `corrections_for` then treats the stale claim as live and the
served history presents the superseded value as current, with no marker.

## Timing windows and dependencies

None within a compose; each read runs in one connection snapshot. The
property depends on range order, which appends enforce and
`capture_legacy_seqs_verifying_order_tx` verifies, and on legacy rows
carrying no claims.

## What a test must construct

A store with more rows than R, keys recurring across R's lower boundary,
legacy rows inside and past the pressure window, and each production read
at several budgets and folded sequences; then the argmax over R against the
argmax over the whole store for every loaded claim.

## Investigation log

### Q: Could a legacy row with claims be loaded while a newer non-legacy corrector is not?

- Sources examined: `load_history_segment_fold`,
  `capture_legacy_seqs_verifying_order_tx`,
  `crates/daemon/src/history_summarizer.rs` segment mapping.
- Findings: the m0 fold loads every captured legacy row regardless of age but
  only a newest run of non-legacy rows. A legacy row holding claims below that
  run would be loaded without newer non-legacy rows. The publish path marks a
  row legacy only when `p1` is empty, which strict validation rejects.
  Since `0231f2f4` the store itself keeps no claims on a legacy row
  (`prepare_history_segment` skips `prepare_claims` when `legacy == 1`,
  checked by `a_legacy_row_stores_no_claims`), so a validation bypass cannot
  store one. The suffix test also asserts no legacy row carries claims.
- Missing evidence: none.
- Conclusion: resolved with answer: no; the store enforces that legacy rows
  hold no claims.

### Q: Does the test show it can fail?

- Sources examined: the test body.
- Findings: its oracle, store-wide argmax against R's argmax, is
  independent of the read. Commit `1c30058c` adds a negative control: the
  m0 fold at budget 60,000 with its newest claimed row removed fails the
  property. The test also asserts each loaded set holds claims, so the
  property is not vacuous.
- Missing evidence: none.
- Conclusion: resolved with answer: yes, the negative control demonstrates
  failure power.

### Q: Does the check pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon --all-features --locked --lib -E 'test(decay_render::) | test(correction) | test(every_loaded_claim)'`
  at `c38af85a`.
- Findings: 28 passed, including this test in 1.395 s.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the premise and the negative control pass at the final commit?

- Sources examined: `cargo +1.98 nextest run -p memory-store -p daemon --all-features --locked`
  at `a75ff925`, which runs
  `crates/memory-store/src/lib.rs::tests::a_legacy_row_stores_no_claims` and
  `crates/daemon/src/m0_compose.rs::bounded_read_tests::every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set`.
- Findings: both pass; the only failures in that run are four timing tests
  unrelated to claims that also fail on the base under host load.
- Missing evidence: none.
- Conclusion: resolved with answer.
