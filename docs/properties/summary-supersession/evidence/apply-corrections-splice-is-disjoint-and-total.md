# apply-corrections-splice-is-disjoint-and-total

## Discovery trigger

The #835 Property Catalog record of the same name, derived at `265df096`,
and D-7: each anchor is located by first occurrence in the original body; a
miss goes to the footer; any hit whose span overlaps another goes to the
footer, except that in a three-way chain the two disjoint outer hits splice
and the middle one footers; kept hits splice from the highest offset down;
remaining corrections append one footer line in `idx` order; the footer
renders even when the tier body is empty. The M2 ticket #839 names this
record and its faults (identical, nested, repeated, and three-way overlapping
anchors, an absent anchor, an empty tier body).

## Evidence trail

`crates/daemon/src/decay_render.rs::apply_corrections` computes one hit per
correction with `body.find(anchor)` on the original body, treating a `None`
or empty anchor as a miss. It computes each hit's overlap degree over the
other hits and splices hit `i` only when every hit it overlaps has a strictly
greater degree. Two spliced hits cannot overlap: each would need the greater
degree. The kept hits sort by `Reverse(hit.start)` and splice with
`replace_range` on a copy of the body, so earlier offsets stay valid. The
footer collects every non-spliced correction in slice order through
`correction_marker(MarkerForm::Entry, ..)` and `corrections_line`, and
appends it after a newline only when the output is non-empty.

The slice order is `idx` order: `corrections_for` emits a row's corrections
in claim position order.

`crates/daemon/src/decay_render.rs::render_one_history_segment` calls
`apply_corrections` on the trimmed tier text and returns the heading alone
only when the result is empty, so a title-only row with corrections renders
heading plus footer.

Tests:

- `crates/daemon/src/decay_render.rs::tests::apply_corrections_splices_disjoint_first_hits_and_footers_the_rest`
  runs a table over the body `port 1 then mode a then port 1 again`: one hit
  spliced at its first occurrence only; two disjoint hits; an absent anchor
  and a `None` anchor footered in `idx` order; footer order `k.z` then `k.a`
  (not key or ordinal order); an overlapping pair; identical anchors; nested
  anchors; a three-way chain whose outer hits splice and whose middle
  footers. It also asserts the footer alone for an empty body.
- `crates/daemon/src/decay_render.rs::tests::correction_properties::apply_corrections_is_disjoint_and_total`
  generates bodies over `[ab ]{0,24}` and up to six optional anchors over
  `[ab ]{1,5}`. It asserts that each correction appears exactly once as a
  marker or a footer entry, that footer indices increase, that a found anchor
  overlapping no other found anchor splices, and that replacing each marker
  with its anchor reproduces the body.
- `crates/daemon/src/decay_render.rs::tests::a_title_only_row_renders_its_heading_and_footer`
  renders an empty `p4` at tier 4 as `## 1-2 · t\n[corrections: k.a = v @3]`.

## Failure scenario

Splicing low offset first shifts later hit ranges and writes a marker into
the middle of another span. Splicing both hits of an overlapping pair
destroys the bytes they share. Dropping a correction whose anchor misses
leaves the stale value unmarked.

## Timing windows and dependencies

None. The function is pure over `(body, corrections)`. Anchor hits at tier 1
depend on `anchor-is-substring-of-stored-p1`; at denser tiers a miss is
expected and footers.

## What a test must construct

An absent anchor, a `None` anchor, an overlapping pair, identical anchors,
nested anchors, an anchor occurring twice, a three-way chain, footer entries
whose key and ordinal order differ from `idx` order, and an empty body; plus a
generated check with an independent restoration oracle.

## Investigation log

### Q: Does low-offset-first splicing fail the table?

- Sources examined: the "two disjoint hits" case.
- Findings: the expected bytes place `[corrected @9: db.port = 2]` at offset 0
  and `[retracted @7: ui.mode]` at the original offset of `mode a`. Splicing
  `port 1` first would lengthen the prefix by 21 bytes and shift the second
  range into the first marker, so the expected string would not match.
- Missing evidence: no mutant was run.
- Conclusion: resolved with answer by reasoning over the fixed bytes.

### Q: Is the restoration oracle independent of the degree rule?

- Sources examined: the property test body.
- Findings: it locates markers by their literal text, restores anchors with
  `replacen`, and checks isolation by recomputing hits with `body.find`. It
  does not call the degree computation. It does not check which member of an
  overlapping cluster splices beyond totality and isolation.
- Missing evidence: none for the catalog's Check.
- Conclusion: resolved with answer; the chain and pair cases are pinned by the
  table, not the generator.

### Q: Do the checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon --all-features --locked --lib -E 'test(decay_render::) | test(correction) | test(every_loaded_claim)'`
  at `c38af85a`.
- Findings: 28 passed, including the table, the property test, and the
  title-only test.
- Missing evidence: none.
- Conclusion: resolved with answer.
