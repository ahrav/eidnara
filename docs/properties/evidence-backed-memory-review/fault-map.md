# Fault map

## Fault classes

| Class | Available today | How |
| --- | --- | --- |
| Decision supersession after resolution | yes | `Envelope::correct_decision` between `resolve_descriptor` and `proposal_target` |
| Decision retirement | yes | `Envelope::retire_decision` |
| Owner never registered | yes | publish a descriptor whose identity names an unknown decision id |
| Owner of the wrong kind | yes | publish a descriptor whose identity names an evidence or observation object |
| Stale bound revision | yes | construct a `CanonicalSource` whose `decision_source_revision` disagrees with the live row |
| Remote destination on a Sensitive artifact | yes | `EvidenceBroker` with `ArtifactDestination::Remote` |
| Production selection open | no | `PRODUCTION_SELECTION_OPEN` is a constant; no runtime switch exists, and no Remote-eligible representation exists to select |
| Decision reclassified Sensitive after resolution | no | no test helper reclassifies a live decision; the broker folds `decision.sensitivity` on every read |
| Decision scoped to another project | yes | `DecisionSpec.scope_id` naming a scope whose project term differs, read under a broker bound to the descriptor's project |
| Descriptor republished at a new revision after binding | yes, unconstructed | `publish_source_descriptor` with `revision: "2"` for the same occurrence |
| Selected class set drifts from the decision-derived set | yes | add a class to `MEMORY_CLASSES` alone (compile error from the `const` assertion in `selection.rs`) or to `decision_derived` alone (`the_selected_classes_are_exactly_the_decision_derived_classes` fails) |
| Crash after transfer, before completion | yes | `Fixture::kernel_half` in `memory_reviewer_settlement.rs` commits the Kernel half and returns without completing |
| Sweep at the earlier of run and queue deadlines | yes | `MemoryStore::expire_memory_reviewer_work` at the chosen instant |
| Late completion after a sweep terminal | yes | `Settlement::settle` after `expire_memory_reviewer_work` |
| Sweep inside the settlement window | yes | `Settlement::before_completion_for_test` running `expire_memory_reviewer_work` |
| Takeover without a losing settlement | yes | `take_over_memory_reviewer_receipt` after `kernel_half` |
| Stored owner or class changed after selection | yes | direct SQLite update of `candidates.provenance_witness` or `sensitivity_class` |
| Broker, aliases, and transcript lost | yes | drop the run's `EvidenceBroker`; construct a fresh one with an empty hold id |
| Dependency record edited or removed | yes | direct SQLite `replace` or `json_remove` on the witness |
| Unterminated, cancelled, or `not_dispatched` marker before a run | yes | `dispatch_memory_reviewer_attempt` without a terminal, a cancelled first run, or a recheck clock past the attempt deadline |
| Uncited member retired after selection | yes | `retire_observation` on a disclosed but uncited source |
| Selection dated at or after the queue deadline | yes | `read_selected_review_input` with `selected_at >= deadline`; the ledger cannot record one |

## Required faults per property

| Property | Required faults and states | Constructed |
| --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | production selection open; Remote-eligible decision representation; open activation gate | no |
| `canonical-resolution-refuses-changed-owner-and-target` | supersession after resolution; unregistered owner; stale descriptor revision; stale bound decision revision; wrong-kind owner; owner in another project's scope; Remote destination | yes |
| `private-result-transfer-preserves-queue-expiry` | crash after transfer; sweep at the earlier deadline; sweep inside the settlement window; late completion; takeover orphaning a hold; selection at the deadline; selected read past the queue deadline and at the hold's expiry; changed owner or class after selection | yes |
| `receipt-selection-fences-private-generation-results` | takeover to generation 2 after a generation-1 Kernel envelope; adopt and read at generation 2 | yes |
| `durable-private-result-recovers-without-model-refire` | broker lost; record edited; record removed; member retired; completed marker without a row | yes |
| `unknown-dispatch-does-not-authorize-resend` | unterminated marker; cancelled marker; `not_dispatched` marker | yes |
| `uncited-owner-lineage-remains-read-authority` | uncited member retired after selection; fabricated canonical member | yes |

## Coverage checks to add

- When a Remote-eligible representation lands: a coverage marker asserting the
  independent preconditions (production selection open, activation open, a
  descriptor whose artifact egress is `Allowed` for Remote) so the campaign can
  show the positive situation occurred rather than the branch merely compiled.
- A reclassification fault: a live decision moved to `Sensitive` after the run
  bound it, asserted at the broker read as `PolicyBlocked` with zero bytes.

## Priority ranking

1. Supersession after resolution: one Kernel commit, exact refusal code, covers
   the target-binding invariant end to end.
2. Remote refusal of the canonical artifact: one broker read, proves the gate's
   reason.
3. Wrong-kind and unregistered owners: cheap descriptor publications.
