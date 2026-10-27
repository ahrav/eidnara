# p2-new-anchor-row-decodes-on-write

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N1, activated by the U1 ticket [#1133](https://github.com/ahrav/eidnara/issues/1133).

## Evidence trail

`Envelope::insert_anchor` (`crates/kernel/src/anchor_write.rs:405`) calls `validate_anchor` (`:296`) and maps every refusal to `InvalidInput`. `validate_anchor` refuses blank, negative, or secret-bearing identity fields, builds the row from the typed `AnchorValue` (`:35`), and runs `validate_anchor_row` (`:287`): the engine's `anchor_row_exceeds_bounds` and then `AnchorCondition::decode`. It then compares the decoded condition to the request (`matches`, `:149`), refuses a capture naming a commit other than the anchor's own oids (`has_unanchored_capture`, `:128`), because the ladder reads captures by anchored oid only, and refuses value columns or capture fields holding a detected secret or failing the scan. `capture_holds_secret` (`:209`) scans each capture field as its own text before JSON encoding. The encoded payload writes an absent `tree_oid` or `patch_id` as `null`, and the scanner's context safelist (`crates/secret-scanner/src/rules.rs:46`) suppresses a candidate near a `:null` assignment, so a scan of the encoded payload accepts a secret beside an absent field. The stored row is the returned row, column for column. `AnchorValue` has one variant per kind and each variant fills only its kind's columns, so the unowned-column refusal is reachable only through `validate_anchor_row` on a raw row.

## Failure scenario

An uppercase oid stored by the writer would decode as `InvalidOid` and evaluate `Uncertain` forever.

## Timing windows and dependencies

None.

## What a test must construct

Every kind once, asserting the stored row decodes to a condition written out literally from the test inputs, with captures on both git kinds and one non-`Normal` class; every refused shape once, including a capture for another commit, a secret inside a capture, a secret in each optional capture field, a secret in a capture whose `tree_oid` or `patch_id` is absent, and a keyed credential whose separator is a tab, asserting `InvalidInput` and zero rows; a raw row with an unowned column through `validate_anchor_row`.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_anchor_write.rs:311`, `:383`, `:459`, `:684`, read at the branch that adds this part (base `3a3b916b7`).
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

### Q: Does a scan of the encoded capture payload see every secret a capture holds?

- Sources examined: `encode_anchor_captures` (`crates/kernel/src/anchor.rs:234`), the context safelist at `crates/secret-scanner/src/rules.rs:46`, and `validate_anchor` run on the test's `SECRET` in `changed_paths`.
- Findings: No. With both optional fields present the payload scan refused the secret; with `patch_id`, `tree_oid`, or both absent it accepted the capture and the payload stored the secret verbatim, because each absent field encodes as `:null`. JSON escaping also hid a keyed credential separated by a tab or a newline, and one written in JSON quotes inside a path.
- Missing evidence: none.
- Conclusion: resolved with answer: `validate_anchor` scans each capture field before encoding, and `invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause` covers each shape.
