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
| Decision scoped to another project | yes, unconstructed | `DecisionSpec.scope_id` naming a scope whose project term differs |
| Descriptor republished at a new revision after binding | yes, unconstructed | `publish_source_descriptor` with `revision: "2"` for the same occurrence |
| Selected class set drifts from the decision-derived set | yes | add a class to `MEMORY_CLASSES` alone (compile error from the `const` assertion in `selection.rs`) or to `decision_derived` alone (`the_selected_classes_are_exactly_the_decision_derived_classes` fails) |

## Required faults per property

| Property | Required faults and states | Constructed |
| --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | production selection open; Remote-eligible decision representation; open activation gate | no |
| `canonical-resolution-refuses-changed-owner-and-target` | supersession after resolution; unregistered owner; stale descriptor revision; stale bound decision revision; wrong-kind owner; Remote destination | yes |

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
