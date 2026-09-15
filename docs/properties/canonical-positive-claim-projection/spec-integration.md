# Specification integration

Crosswalk from each stable slug to the specification section that motivates
it, the implementation ticket that owns it, and the code that carries it.
Ticket numbers here are tracking metadata, not names of anything in the tree.

| Slug | Specification section | Ticket | Code |
| --- | --- | --- | --- |
| `canonical-claim-fields-match-fenced-source` | Canonical facts and identity; A1 | #458, #460 | `crates/kernel/src/claim_facts.rs` |
| `canonical-provenance-survives-approved-write-read-path` | Open question: canonical-format owner | #458, #460 | `crates/kernel/src/claim_causality.rs` |
| `claim-capacity-failure-is-atomic` | Bounds, capabilities and stop conditions | #458, #460, #466 | `claim_facts.rs`, `claim_causality.rs` |
| `claim-enablement-requires-approved-evidence` | Acceptance: Enablement | #458, #460, #466 | `crates/daemon/src/projection_gates.rs` (no claim hook yet) |
| `claim-export-predecode-bounds` | Projection, progress and recovery | #458, #460 | `claim_causality.rs`, `crates/kernel/src/source_export.rs` |
| `claim-format-rollback-preserves-canonical-state` | Failure and rollback; KTD2 | #458, #460 | `claim_causality.rs` |
| `claim-replay-preserves-newest-canonical-state` | Projection, progress and recovery; A3 | #458, #460 | `claim_causality.rs` |
| `echo-classification-requires-canonical-causality` | Echo provenance and use authority; A2; U3 | #458, #460, #466 | `claim_causality.rs` |
| `malformed-required-field-stops-projection-progress` | Projection, progress and recovery | #458, #460 | `claim_facts.rs` |
| `occurrence-identity-is-not-payload-or-source-triple` | Canonical facts and identity | #458, #460 | `claim_facts.rs`, `crates/kernel/src/source_identity.rs` |
| `projection-has-no-second-truth-or-policy-authority` | Canonical facts and identity; A4 | #458, #460, #466 | `claim_facts.rs` |
| `retrieved-content-cannot-upgrade-write-authority` | Echo provenance and use authority | #458, #466 | `crates/kernel/src/slice/write.rs` |
| `revision-domains-remain-distinct-and-supported` | Canonical facts and identity | #458, #460, #466 | `claim_facts.rs` |
| `served-sensitivity-and-artifact-policy-govern-egress` | Echo provenance and use authority | #458, #466 | `claim_facts.rs`, `crates/kernel/src/eligibility.rs` |
| `supporting-authority-is-preserved-not-recomputed` | Canonical facts and identity | #458, #460, #466 | `claim_facts.rs`, `crates/kernel/src/admission.rs` |
| `claim-cancellation-preserves-durable-work` | Bounds; Recovery | #460, #466 | pending |
| `claim-consumer-replay-includes-published-history` | Projection, progress and recovery | #460 | pending |
| `claim-disable-preserves-consumer-contract` | Projection, progress and recovery | #460 | pending |
| `claim-export-retention-fence` | Projection, progress and recovery | #460 | pending |
| `claim-local-commit-before-ack` | Projection, progress and recovery | #460 | pending |
| `claim-rebuild-incremental-parity` | A3 | #460 | pending |
| `claim-recovery-converges-within-approved-bound` | Recovery | #460 | pending |
| `claim-tombstone-masks-all-representations` | Projection, progress and recovery | #460 | pending |
| `claim-worker-result-cannot-outlive-identity` | Projection, progress and recovery | #460 | pending |
| `eligible-positive-and-unknown-claims-remain-reachable` | Outcome; Useful-path coverage | #460, #466 | pending |
| `unknown-echo-state-is-policy-neutral` | Echo provenance and use authority; A2 | #460, #466 | pending |
| `bound-project-scope-cannot-be-widened-by-candidate` | Echo provenance and use authority | #466 | pending |
| `candidate-validation-preserves-surface-policy` | U4 | #466 | pending |
| `checkout-applicability-is-revalidated-without-relevance-refresh` | U4 | #466 | pending |
| `eligibility-cache-cannot-change-canonical-verdict` | Echo provenance and use authority | #466 | pending |
| `optional-edits-require-host-capability-and-survival-proof` | Bounds, capabilities and stop conditions | #466 | pending |
| `stale-projection-cannot-authorize-current-use` | U4 | #466 | pending |
| `u5-class-transition-situations-are-witnessed` | U5 | #466 | pending |
| `u5-evaluation-keeps-provenance-and-judgment-separate` | U5 | #466 | pending |
| `u5-rejection-and-unknown-accounting-is-lossless` | U5 | #466 | pending |

## Design decisions carried by the kernel side

- KTD2 inspection outcome: the existing typed `ObservationPayload.detail`
  extension carries a versioned causality detail without a canonical schema,
  epoch, or wire change. The digest contract is preserved because the record is
  an ordinary observation row; its detail is JSON the redactor must leave
  unchanged. No separately approved canonical-format unit was needed for this
  slice.
- Producer authority is read from `commit_log.producer` of the record's commit,
  so a producer string inside a payload has no effect.
- A parent invalidated after a derivation was observed keeps the lineage;
  acquisition evidence must stay live for `DirectObservation`. The asymmetry is
  deliberate: a derivation is a historical fact about the parent as it was, an
  acquisition is a standing claim about retained bytes.
