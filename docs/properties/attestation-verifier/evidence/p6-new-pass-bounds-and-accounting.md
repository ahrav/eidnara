# p6-new-pass-bounds-and-accounting

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. A proposed obligation: `proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)`.

## Evidence trail

The specification's Property Catalog defines this obligation for N7. [#1173](https://github.com/ahrav/eidnara/issues/1173) lands the code and the check that converts it.

## Failure scenario

A pass looping to exhaustion, double counting, or an effect counted before commit.

## Timing windows and dependencies

See the fault map.

## What a test must construct

F7 between decisions; F13: more than N candidates.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the #1173 pass budget cover blob content or all decoded-object work? (needs human input)

- Sources examined: `Evaluation` and `read_object` in `crates/daemon/src/attestation_verifier.rs`.
- Findings: `Evaluation::bytes_read` counts blob content only. The commit and every tree on the path are read into a discarded counter, so a claim on a 16-component path can decode a commit, 16 trees, and a blob, each up to 256 KiB, while `bytes_read` reports at most 256 KiB.
- Missing evidence: the #1173 owner's choice of what the pass budget bounds.
- Conclusion: needs human input
