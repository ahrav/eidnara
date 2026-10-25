# il-literal-classified-contract-or-vocabulary

## Discovery trigger

Issue #1105 Part B S2 (R5, OQ12): every literal of the layer carries one class, and
the variation-point owner found that two classes do not fit Eidnara's own
identifiers.

## Evidence trail

- [`literals.md`](../literals.md) lists 256 distinct string literals at 755
  occurrences in the production source of nine files. 156 name a field, a
  tool, an argument key, or a discriminator value, in 167 rows (39
  vocabulary, 34 Eidnara-owned, 94 contract); 100 are excluded with a reason.
- The table was built by extracting every string literal outside top-level
  `#[cfg(test)]` regions and items, assigning each occurrence to one row, and
  checking that every occurrence appears in exactly one row.
- The distinctive vocabulary subset equals the gate's literal set
  (`scripts/forbid-tool-literals.sh:16`).
- Eidnara-owned identifiers defined more than once: `synthetic_todo_`
  (`crates/daemon/src/injection.rs:20`, `crates/daemon/src/codec/opencode.rs:1074`);
  the `eidnara_note` actions (`crates/daemon/src/tool_vocabulary.rs:37`,
  `crates/daemon/src/lib.rs:12854`, `:13171`, `:18609`,
  `crates/daemon/src/conditional_note_evaluation.rs:1905`).

## Failure scenario

A new literal lands with no row; the next move into or out of the module
follows habit, and the gate's set drifts from the classification.

## Timing windows and dependencies

None; static.

## What a test must construct

A review of [`literals.md`](../literals.md) whenever an interpretation-layer
file gains a string literal.

## Investigation log

### Q: Are Eidnara-owned identifiers a third class?

- Sources examined: OQ12; `tool_vocabulary.rs:1-18`; U4's documentation.
- Findings: yes; U4's module doc and gate treat them as neither contract nor
  vocabulary.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should the duplicated Eidnara-owned spellings be defined once?

- Sources examined: the sites above.
- Findings: each spelling is repeated across files with no shared constant.
- Missing evidence: an owner decision.
- Conclusion: needs human input.

### Q: What confidence does the classification carry?

- Sources examined: the extraction and its checks; the rows for generic words
  such as `/metadata/truncated` and `content`.
- Findings: the extraction, the one-row-per-occurrence check, and the gate-set
  equality are mechanical. A class for a generic word, such as vocabulary for
  the OpenCode truncation pointer that U4 placed in the module, is a
  judgement the table records.
- Missing evidence: a recurring check that re-runs the classification.
- Conclusion: resolved with answer: medium; `Exercised` is `partial`.
