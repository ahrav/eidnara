# Fault map

## Fault classes

| Class | Available today | How |
| --- | --- | --- |
| Process restart | yes | `Fixture::reopen` in `crates/kernel/tests/kernel_claim_facts.rs` |
| Out-of-band row corruption | yes | direct SQLite writes to `observations` and `admission_decisions` (the registry is append-only) |
| Evidence retirement | yes | `Envelope::retire_evidence` |
| Object retirement and correction | yes | `retire_observation`, `retire_decision`, `correct_decision` |
| Duplicate commit | yes | repeated `CommitIntent` |
| Projection lag behind the kernel | yes | write to the kernel without running the materializer or catch-up |
| Fresh rebuild | yes | `Corpus::bootstrap` on a second data home |
| Power loss | no | out of scope for this crate; see `crates/kernel/tests/cas_fault_injection.rs` for the SIGKILL harness |

## Required faults per property

| Property | Required faults and states | Constructed |
| --- | --- | --- |
| `canonical-claim-fields-match-fenced-source` | later commit on the same lineage; snapshot before creation | yes |
| `canonical-provenance-survives-approved-write-read-path` | reopen | yes |
| `claim-capacity-failure-is-atomic` | over-bound request; over-bound parent list | yes |
| `claim-enablement-requires-approved-evidence` | a production caller | no caller exists |
| `claim-export-predecode-bounds` | bound below a real record | yes |
| `claim-format-rollback-preserves-canonical-state` | higher detail version | yes |
| `claim-replay-preserves-newest-canonical-state` | duplicate commit; replacement record | yes |
| `echo-classification-requires-canonical-causality` | forgery; evidence retirement; parent retirement; record retirement | yes |
| `malformed-required-field-stops-projection-progress` | corrupted enum column | yes |
| `occurrence-identity-is-not-payload-or-source-triple` | one of three representations published | yes |
| `projection-has-no-second-truth-or-policy-authority` | none | yes |
| `retrieved-content-cannot-upgrade-write-authority` | forged kind, id, and object prefixes | yes |
| `revision-domains-remain-distinct-and-supported` | higher detail version | yes |
| `served-sensitivity-and-artifact-policy-govern-egress` | none for the served half | yes |
| `supporting-authority-is-preserved-not-recomputed` | revoked supporting approval | yes |
| `claim-tombstone-masks-all-representations` | correction and retirement with a lagging projection | yes |
| `claim-rebuild-incremental-parity` | catch-up then fresh bootstrap | yes |
| `unknown-echo-state-is-policy-neutral` | equal facts, different classes | yes |
| `eligible-positive-and-unknown-claims-remain-reachable` | served claim without a record | yes |
| `claim-local-commit-before-ack` | crash between local commit and acknowledgement | RP2.1 harness |
| `claim-consumer-replay-includes-published-history` | lost or skipped acknowledgement | yes |
| `claim-export-retention-fence` | released or expired hold | RP2.1 tests |
| `claim-worker-result-cannot-outlive-identity` | tombstone between dispatch and publication | RP2.1 tests |
| `claim-cancellation-preserves-durable-work` | cancellation at each boundary | RP2.1 tests |
| `claim-disable-preserves-consumer-contract` | lagging consumer at disable | no |
| `claim-recovery-converges-within-approved-bound` | approved bound plus crash cuts | no |
| `candidate-validation-preserves-surface-policy` | labeled claims validated on two surfaces | yes |
| `stale-projection-cannot-authorize-current-use` | quarantine and correction between two validations | yes |
| `eligibility-cache-cannot-change-canonical-verdict` | duplicated ordered candidates, two reads | yes |
| `u5-rejection-and-unknown-accounting-is-lossless` | rejected Unknown and permitted genuine in one batch; one representation purged | yes |
| `u5-evaluation-keeps-provenance-and-judgment-separate` | equal policy, different classes | yes |
| `u5-class-transition-situations-are-witnessed` | frozen RP2.9 manifest | partial |
| `bound-project-scope-cannot-be-widened-by-candidate` | foreign-project digest over the same candidates | yes |
| `checkout-applicability-is-revalidated-without-relevance-refresh` | checkout change with a wired engine | no |
| `optional-edits-require-host-capability-and-survival-proof` | harness with and without capability | no |

## Coverage checks to add

- `claim-enablement-requires-approved-evidence`: a caller inventory once a
  daemon path calls the reader or writer.
- `claim-recovery-converges-within-approved-bound`: a claim campaign on the
  process-crash harness once RP2.9 approves a bound.
- `claim-disable-preserves-consumer-contract`: a claim consumer disable path
  once the pending-consumer transition is decided.
- `checkout-applicability-is-revalidated-without-relevance-refresh`: a daemon
  path that loads applicability inputs for claim candidates and calls the
  kernel engine, with a git fixture carrying overlapping and disjoint edits.
- `optional-edits-require-host-capability-and-survival-proof` and the six
  delivery witnesses: OpenCode and Pi harness runs observing real host and
  provider output.

## Leverage ranking

1. Out-of-band row edits: cheapest valid oracle for version, malformed, and
   conflict cases; already used.
2. Public-API refusals through `commit`: exercise the poison path with no
   internal seam.
3. Reopen: one line per test, covers durability of the roundtrip.
