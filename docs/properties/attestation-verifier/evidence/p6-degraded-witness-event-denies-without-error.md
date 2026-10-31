# p6-degraded-witness-event-denies-without-error

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`validate_trigger` (`crates/kernel/src/admission.rs:2807`) looks the trigger up by id, kind (`code_present` or `config_present`), and the subject's `(source_kind, source_id, source_revision)`, requiring the observation and any backing evidence live. Every miss returns `Ok(None)`. `record_admission` (`:998`) calls it at `:1662` and evaluates `EventKind::Other` when it returned `None` (`:1699`). `event_effect` (`:4245`) maps `Other` to the current maturity, no disposition, and `Outcome::Deny`. The trigger id first passes `identity`, which returns an error for a malformed id, so the degradation holds for well-formed ids (refinement R3).

## Failure scenario

A verifier that treats `Ok` from `record_admission` as promotion commits an attestation that stays at `Candidate`.

## Timing windows and dependencies

None.

## What a test must construct

A trusted-class subject and witness events naming a missing observation, an observation of another source, of the other kind, an invalidated observation, and an observation at another revision.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
