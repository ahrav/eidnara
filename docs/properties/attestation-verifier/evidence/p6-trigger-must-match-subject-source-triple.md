# p6-trigger-must-match-subject-source-triple

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

The SQL in `validate_trigger` (`crates/kernel/src/admission.rs:2807`) selects the observation by `object_id`, `object_kind='observation'`, live registry and observation rows, `source_kind`, `source_id`, `source_revision`, and `observation_kind`, and requires `observed.evidence_id IS NULL` or a live backing evidence row. The trusted-class precondition comes from `automatic_ceiling` (`:4302`): under other classes a validated trigger still cannot raise maturity.

## Failure scenario

An observation from another source or revision validates the trigger, so a witness that checked other bytes promotes the subject.

## Timing windows and dependencies

The trigger or its evidence invalidated in an earlier commit or earlier in the same envelope.

## What a test must construct

Triggers that differ from the subject in exactly one of source kind, source id, source revision, observation kind, or liveness, plus one with retired backing evidence.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does `validate_trigger` compare the observation's evidence with anything on the subject?

- Sources examined: `validate_trigger` and C7 of the specification.
- Findings: No. It joins `evidence_meta` only to require live backing evidence and to fold its class.
- Missing evidence: none.
- Conclusion: resolved with answer: evidence equality is the verifier's own check (`p6-new-witness-evidence-id-equals-attestation`).
