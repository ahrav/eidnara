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
| `claim-cancellation-preserves-durable-work` | Bounds; Recovery | #460, #466 | `crates/daemon/src/search_catchup.rs` (shared) |
| `claim-consumer-replay-includes-published-history` | Projection, progress and recovery | #460 | `crates/daemon/src/claim_sources.rs`, `crates/daemon/src/commit_stream.rs` (shared) |
| `claim-disable-preserves-consumer-contract` | Projection, progress and recovery | #460 | `crates/kernel/src/outbox.rs` (shared); no claim path |
| `claim-export-retention-fence` | Projection, progress and recovery | #460 | `crates/kernel/src/source_hold.rs` (shared) |
| `claim-local-commit-before-ack` | Projection, progress and recovery | #460 | `crates/daemon/src/search_catchup.rs` (shared) |
| `claim-rebuild-incremental-parity` | A3 | #460 | `crates/retrieval/src/claims.rs` |
| `claim-recovery-converges-within-approved-bound` | Recovery | #460 | `crates/daemon/src/search_replacement.rs` (shared); no approved bound |
| `claim-tombstone-masks-all-representations` | Projection, progress and recovery | #460 | `crates/daemon/src/claim_sources.rs`, `crates/retrieval/src/claims.rs` |
| `claim-worker-result-cannot-outlive-identity` | Projection, progress and recovery | #460 | `crates/kernel/src/current_input.rs`, `crates/retrieval/src/vectors.rs` (shared) |
| `eligible-positive-and-unknown-claims-remain-reachable` | Outcome; Useful-path coverage | #460, #466 | `crates/retrieval/src/claims.rs` |
| `unknown-echo-state-is-policy-neutral` | Echo provenance and use authority; A2 | #460, #466 | `crates/retrieval/src/claims.rs` |
| `bound-project-scope-cannot-be-widened-by-candidate` | Echo provenance and use authority | #466 | `crates/kernel/src/eligibility.rs`, `crates/retrieval/src/claims.rs` |
| `candidate-validation-preserves-surface-policy` | U4 | #466 | `crates/kernel/src/eligibility.rs` (`judge_surface_eligibility`), `crates/retrieval/src/claims.rs` (`validate_for_surface`) |
| `checkout-applicability-is-revalidated-without-relevance-refresh` | U4 | #466 | `crates/kernel/src/applicability/` (engine exists; no claim caller) |
| `eligibility-cache-cannot-change-canonical-verdict` | Echo provenance and use authority | #466 | `crates/kernel/src/eligibility.rs`; `crates/daemon/src/kernel_routes/eligibility.rs` (unchanged) |
| `optional-edits-require-host-capability-and-survival-proof` | Bounds, capabilities and stop conditions | #466 | no code; harness integration pending |
| `stale-projection-cannot-authorize-current-use` | U4 | #466 | `crates/retrieval/src/claims.rs` (`validate_for_surface`) |
| `u5-class-transition-situations-are-witnessed` | U5 | #466 | `crates/daemon/tests/claim_sources.rs` (witnesses); manifest pending |
| `u5-evaluation-keeps-provenance-and-judgment-separate` | U5 | #466 | `crates/retrieval/src/claims.rs` |
| `u5-rejection-and-unknown-accounting-is-lossless` | U5 | #466 | `crates/retrieval/src/claims.rs` (`UseAccounting`) |

## Design decisions carried by the final-use gate

- `judge_surface_eligibility` keeps the existing batch judgement and the
  `kernel.eligibility.batch` wire contract unchanged; it pairs each batch
  verdict with the serving view's visibility on the requested surface from the
  one serving read the batch already makes, so the three surfaces cannot come
  from different snapshots. A batch `Ok` therefore permits explicit search with its label
  and grants nothing on `AutoInject` unless the serving view shows the object
  there.
- `validate_for_surface` judges the decision object each claim row names, not
  the descriptor object, because admission-only dispositions are recorded on
  the decision, and submits the row's artifact digest so the artifact egress
  gate applies at the requested destination. Only `Current` candidates reach
  the kernel; the others are denied by their state. The kernel read
  (`judge_surface_eligibility_with_claims`) returns the canonical claim facts
  of every named object from the same snapshot as the verdicts, and each
  permitted row is reclassified against them, so a projection digest is never
  the authority for which artifact is judged, a representation retired since
  classification is denied at the fresh tip, and the lineage accounting is the
  snapshot's. One `EvalBudget` bounds the kernel read. The same call serves
  preselection admission and the final revalidation of packed survivors; the
  second call reads a newer snapshot and denies whatever was restricted since.
- Not wired in this change: a daemon route or plugin path that calls the gate,
  checkout applicability for claim candidates, and the six harness delivery
  witnesses. Those remain open under the delivery ticket.

## Design decisions carried by the projection side

- The projection stores no claim state. `retrieval::claims::classify_live_claims`
  is the snapshot-bound read: it checks the projection identity against the
  kernel's incarnation, reads the live rows, captures the tip and kernel
  incarnation together with `capture_commit_read_target`, reads
  `claim_facts_at` that target (refused under the reader guard if the store
  was restored in between), and maps each row through `classify`, so a
  lagging projection cannot revive a claim the kernel withdrew and a rebuild
  cannot disagree with the projection it replaces. `classify` is the state
  mapping over already-loaded facts: Current, Superseded, Retracted, Hidden, or
  Stale from the registry row, the own and lineage admission rows, the served
  surface, and the live descriptor inventory. It takes no causal class;
  Unknown neutrality is a property of its signature.
- Materialization, tombstones, checkpoints, pending work, acknowledgement,
  retention fences, and recovery reuse the RP2.1 shared paths unchanged. The
  claim-specific checks prove classification and parity over those paths; the
  crash, cancellation, disable, and bounded-recovery campaigns remain the
  shared harness's, and their claim-specific runs wait on RP2.9 bounds.

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
