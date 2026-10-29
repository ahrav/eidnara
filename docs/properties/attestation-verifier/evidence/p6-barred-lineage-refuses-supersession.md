# p6-barred-lineage-refuses-supersession

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S2. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`refuse_barred_lineage` (`crates/kernel/src/admission.rs:1395`) refuses the three barred dispositions and is called for the predecessor and for a live replacement survivor. `supersede_decision_refuses_a_quarantined_predecessor` (`crates/kernel/tests/kernel_slice.rs:1187`) covers the predecessor.

## Failure scenario

A quarantined decision is superseded and its lineage returns to service.

## Timing windows and dependencies

None.

## What a test must construct

A quarantined predecessor and a quarantined survivor.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
