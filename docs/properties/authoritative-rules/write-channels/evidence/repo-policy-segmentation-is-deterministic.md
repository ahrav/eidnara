# repo-policy-segmentation-is-deterministic

## Discovery trigger

Plan U5 step 3 ("segment deterministically: top-level list items and
blank-line paragraphs outside fenced code; drop fences and headings as units;
prefix each unit with the nearest heading as `[Heading]` and nested files with
`[<dir>]`"), KTD2, key decision "no model in any authority path: repository
rules are segmented deterministically", assumption "repository rule object
ids are `rule_<sha256(source_id:rev:unit_digest)>`", test scenario "identical
bytes produce identical units and ids (fixture-based golden test)". Serves
R2, R8, and R13 (parity needs identical ids across harnesses).

## Evidence trail

Verified at HEAD: nothing implements this. Related existing facts:

- `crates/kernel/src/slice/write.rs:388-393`: a successor cannot reuse the
  predecessor's object id, which is why the id must include `rev`.
- `write.rs:139-162` `insert_decision`: a duplicate object id within one
  envelope or against a live row fails at the registry insert (SQLite
  constraint), poisoning the envelope.
- `crates/daemon/src/project_docs.rs` is named by the plan as the pattern for
  size and UTF-8 handling (not read for this pass; claim under test).

Plan-only: the grammar, labels, normalization, and id formula.

## Failure scenario

Legal witness: bytes

```
# Rules
- Preserve single-baseline storage.
- Pass `--locked` to Cargo.

## Notes
```text
- not a rule
```
A paragraph rule.
```

yield three units: `[Rules] Preserve single-baseline storage.`, `[Rules] Pass
`--locked` to Cargo.`, `[Notes] A paragraph rule.`; the fenced list item and
both headings yield none; the same bytes on a second run yield the same three
`(label, text, digest, id)` tuples in the same order.

Smallest violating traces:

1. The segmenter uses a `HashMap` keyed by heading to group units and emits
   in map order: two runs, two orders; ids are order-independent but
   reconciliation diffs and rendered order differ.
2. The fence tracker treats ```` ``` ```` and `~~~` differently from
   CommonMark; a list item inside a tilde fence becomes a rule.
3. The id hashes `label:text` instead of `text`: a heading rename changes
   every unit's digest and supersedes the whole file as "changed" (see the
   reconciliation record's open question).
4. Two identical list items in one file produce one id; the second
   `insert_decision` fails; the whole file's envelope is poisoned and nothing
   for that file commits.

## Timing windows and dependencies

None; pure function.

## What a test must construct

- Golden fixtures under `crates/daemon/tests/fixtures/` (plan) with expected
  `(label, text, digest, id)` output; a test running segmentation twice and
  comparing; a fuzz or property test that permutes irrelevant whitespace per
  the normalization spec and asserts digest stability, and that changes one
  character and asserts digest change.
- Fixtures: headings only; fences with list markers inside; nested lists
  (only top-level items are units, nested lines fold into the parent item or
  are dropped, as the spec decides); CRLF; trailing spaces; a duplicate item.
- Id recomputation: for each emitted unit assert `id ==
  "rule_" + hex(sha256(source_id + ":" + rev + ":" + digest))`.

## Investigation log

### Q: Are duplicate units within a file handled?
- Sources examined: plan U5 steps 3 and 4; `write.rs:139-162`.
- Findings: the id formula makes duplicates collide; the Kernel insert would
  fail; no plan statement.
- Missing evidence: a rule (drop duplicates, or disambiguate by ordinal).
- Conclusion: unresolved, resolvable at implementation; affects parity and
  must be in the golden fixtures.

### Q: Does the label participate in the unit digest?
- Sources examined: plan U5 step 3 ("prefix each unit with the nearest
  heading").
- Findings: "prefix" suggests the label is part of the rendered text; whether
  it is hashed is not stated.
- Missing evidence: design statement.
- Conclusion: unresolved, resolvable at implementation (recorded in
  open-questions.md as OQ-N2-5).
