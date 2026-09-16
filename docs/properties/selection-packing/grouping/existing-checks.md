# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `89c5589e`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/identity.rs`, `parent_groups_share_a_parent_across_spans_and_never_replace_occurrences` | Spans of one source share a parent key; a revision or representation change derives another. | unaudited | Fixes the key only; no merge, order, or coverage clause. |
| `crates/retrieval/tests/packing_identity.rs`, `grouping_keys_need_parent_revision_and_representation_together` | The grouping key needs class, parent, revision, and representation together. | unaudited | Key equality, not merging. |
| `crates/daemon/src/m0_compose.rs`, `trim_memories_to_budget` (through `render_m0` tests in `crates/daemon/src/lib.rs`) | Memories that do not fit are skipped and later smaller memories are still admitted. | unaudited | The precedent for the rule; its budget is a clamped float. |

Suspiciously quiet areas: no check merged byte ranges before this part, and
no check compared a packer against an independent reference.
