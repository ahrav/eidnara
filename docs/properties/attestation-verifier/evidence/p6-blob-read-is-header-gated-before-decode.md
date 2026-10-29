# p6-blob-read-is-header-gated-before-decode

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S4. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`read_selection` (`crates/daemon/src/git_sources.rs:390`) reads every header with `try_find_header` (`:442`) and charges it before the second loop decodes any object with `find_object` (`:456`), maps an allocation refusal through `exceeds_allocation` (`:354`), charges the decoded length, and refuses a hash mismatch. The verifier reuses `open`, `object_format`, `parse_oid`, and `exceeds_allocation`; this ticket widens `exceeds_allocation` to `pub(crate)`.

## Failure scenario

A delta-packed object is materialized before its header is judged.

## Timing windows and dependencies

None.

## What a test must construct

A selection with one oversized header after a small object.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the U1 verifier follow the same order?

- Sources examined: `observe` (`crates/daemon/src/attestation_verifier.rs:377`) and `crates/daemon/tests/attestation_verifier.rs:438`.
- Findings: Yes: the header read and `header_verdict` precede the one blob read, and the test asserts no blob poll after an over-budget header.
- Missing evidence: none.
- Conclusion: resolved with answer: covered for the verifier by `p6-new-blob-read-bounded-at-oid`.
