# p2-new-anchor-row-decodes-on-write

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N1, activated by the U1 ticket [#1133](https://github.com/ahrav/eidnara/issues/1133).

## Evidence trail

`Envelope::insert_anchor` (`crates/kernel/src/anchor_write.rs:285`) calls `validate_anchor` (`:235`) and maps every refusal to `InvalidInput`. `validate_anchor` refuses blank, negative, or secret-bearing identity fields, builds the row from the typed `AnchorValue` (`:34`), and runs `validate_anchor_row` (`:226`): the engine's `anchor_row_exceeds_bounds` and then `AnchorCondition::decode`. It then compares the decoded condition to the request (`matches`, `:114`) and refuses value columns or a capture payload holding a detected secret. The stored row is the returned row, column for column. `AnchorValue` has one variant per kind and each variant fills only its kind's columns, so the unowned-column refusal is reachable only through `validate_anchor_row` on a raw row.

## Failure scenario

An uppercase oid stored by the writer would decode as `InvalidOid` and evaluate `Uncertain` forever.

## Timing windows and dependencies

None.

## What a test must construct

Every kind once, asserting the stored row decodes to the requested condition; every refused shape once, asserting `InvalidInput` and zero rows; a raw row with an unowned column through `validate_anchor_row`.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_anchor_write.rs:252`, `:313`, `:389`, `:507`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Typed enum or raw row?

- Sources examined: the record's open question in the specification and D1.
- Findings: The ticket chose the typed enum; the engine decoder still validates the built row.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Can the round-trip refusal fire through the typed API?

- Sources examined: `AnchorValue::row` and `AnchorCondition::decode`.
- Findings: Every value the builder writes is the value the decoder reads back, so `RoundTrip` is a guard against a future builder or decoder change.
- Missing evidence: a test can reach it only by changing one of the two functions.
- Conclusion: resolved with answer: kept as a guard, untested by construction.
