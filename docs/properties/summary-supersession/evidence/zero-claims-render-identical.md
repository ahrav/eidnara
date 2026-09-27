# zero-claims-render-identical

## Discovery trigger

The #835 Property Catalog record of the same name, derived at `265df096`,
with C-5 (with no claims the rendered bytes equal today's) and D-7 (with no
corrections `apply_corrections` returns its input without allocating). The
M2 ticket #839 carries the acceptance criterion: with no claims on any
segment, m0 and m1 bytes equal the pre-change goldens byte for byte; with
claims present but none superseded, m0 bytes still equal them. Decision A7 in
#839 narrows the m1 clause to "no claims on the new segments", because the m1
block lists every claim on the new rows whether or not it supersedes one.

## Evidence trail

`crates/daemon/src/decay_render.rs::apply_corrections` returns
`Cow::Borrowed(body)` as its first statement when `corrections` is empty.

`crates/daemon/src/decay_render.rs::corrections_for` emits a correction only
for a claim that is not its key's live claim, so claims that nothing
supersedes produce empty vectors, and `render_rows` stores them.

`crates/daemon/src/decay_render.rs::render_one_history_segment` on the tiered
branch passes the tier text through `apply_corrections`; a borrowed return
leaves the text unchanged, and escaping and the heading guard run as before.
The legacy branch does not call `apply_corrections`.

`crates/daemon/src/m1_compose.rs::render_memory_updates` returns an empty
string when the rows carry no claims, and `compose_m1` passes that to the
`memory_updates` slot of `assemble_m1`, which was empty before M2.

Tests:

- `crates/daemon/src/decay_render.rs::tests::apply_corrections_splices_disjoint_first_hits_and_footers_the_rest`
  ends by asserting `apply_corrections(body, &[])` is `Cow::Borrowed` and
  `std::ptr::eq` to the input.
- `crates/daemon/src/m0_compose.rs::correction_compose_tests::rows_without_claims_render_the_bytes_they_rendered_before`
  composes m1 over two claim-free rows and compares it with `assemble_m1` over
  `render_new_history_segments` of the claim-free rows and an empty updates
  slot. It then stores the same rows with two claims on distinct keys and
  asserts m0 equals the claim-free store's m0 and holds no `[corrected`.
- `crates/daemon/src/decay_render.rs::tests::render_golden_matches_reference`
  and `render_tight_golden_matches_reference_with_real_estimator` read
  literal bytes from `crates/daemon/testdata/render-golden.json` and
  `render-tight-golden.json`. Commit `b79a94d5`, which added corrections,
  only added lines to `render-golden.json` (cases 7 and 8 carry
  corrections; cases 0 to 6 carry none, and `corrections` defaults to empty
  in the fixture struct) and changed no other file under
  `crates/daemon/testdata`. No later commit touches that directory.

## Failure scenario

An allocation or a trailing newline added on the empty path, or a footer
emitted for claims that supersede nothing, changes the served bytes of every
session on upgrade and costs each one its prompt cache.

## Timing windows and dependencies

None. The property depends on `corrections_for` emitting nothing for live
claims, which is `live-claim-is-max-seq-per-key-in-rendered-set`.

## What a test must construct

The pre-change render goldens run unchanged; one store without claims and
one with claims on distinct keys, composed through `compose_m0`; an m1
compose over claim-free rows; the empty-corrections call checked by pointer.

## Investigation log

### Q: What does decision A7 change about the m1 clause?

- Sources examined: #839 Design Decisions (A7), `render_memory_updates`, the
  `rows_without_claims_render_the_bytes_they_rendered_before` body.
- Findings: m1 lists every claim on its rows, so claims that supersede
  nothing still produce a `<memory-updates>` block. The test checks m1 only
  for the claim-free store; for the store with unsuperseded claims it checks
  m0 alone.
- Missing evidence: none for the narrowed clause.
- Conclusion: resolved with answer; m1 is identical only when the new rows
  carry no claims, per A7. m0 is identical whenever no loaded claim is
  superseded.

### Q: Is the m1 comparison against literal pre-change bytes?

- Sources examined: the test body.
- Findings: the m1 reference is built by `assemble_m1` and
  `render_new_history_segments` at HEAD, not read from a literal golden.
  Literal-byte evidence for the claim-free render comes from the render
  goldens, which exercise the shared `render_one_history_segment` path.
- Missing evidence: a literal-byte m1 golden from before `b79a94d5`.
- Conclusion: resolved with answer; the m1 clause rests on the same
  renderer's claim-free path plus the unchanged render goldens.

### Q: Do the checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon --all-features --locked --lib -E 'test(decay_render::) | test(correction) | test(every_loaded_claim)'`
  at `c38af85a`.
- Findings: 28 passed, including both render goldens and the compose test.
- Missing evidence: none.
- Conclusion: resolved with answer.
