# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `d3d7663b`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/packing_identity.rs`, `reads_refuse_unknown_identities_oversized_selections_and_disagreeing_columns` | `read_selected` refuses unknown identifiers and rows whose columns or tuple disagree. | unaudited | Read-level refusal only; no failure class, budget, or trace. |
| `crates/retrieval/tests/dense_oracle.rs`, `authority_moved_reports_an_incarnation_change_before_a_snapshot_change` | `judge_occurrences` returns one disposition per candidate at one snapshot. | unaudited | Candidates come from `live_candidates`, not from a required set. |
| `crates/kernel/src/applicability/checkout.rs` `EvalBudget` tests | An expired deadline or cancellation makes `is_exhausted` true. | unaudited | The budget primitive, not a phase that polls it between stages. |
| `crates/daemon/src/m0_compose.rs` `trim_user_profile_to_budget` | Trims a profile to a floating budget clamped to at least one token. | unaudited | The precedent the packer must not inherit; it answers malformed budgets. |
| `crates/daemon/src/memory_render.rs` `render_memory_line` | Renders a memory line cut at 64 KiB. | unaudited | The precedent the packer must not inherit; the cut is silent. |
| `crates/daemon/src/packing/mod.rs` `materialized_bytes_are_never_printed` | `MaterializedRequired` and `RequiredMaterialization` render the byte length under `Debug`, never the bytes. | unaudited | A logging rule, not a required-phase class, budget, or trace claim; no record here owns it. |

Suspiciously quiet areas: no check before this part refused a floating budget,
and no check charged payload bytes through an injected estimator.
