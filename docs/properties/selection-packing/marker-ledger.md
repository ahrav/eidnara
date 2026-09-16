# Precondition-marker ledger

System: `/local/home/ahrav/scratch/eidnara`. Base:
`cb259ee05a610ad56beea8f3bd2c414e1b3eb771` (the tip of the RP2.7.U1 branch the
U1 change was authored against).

A marker names one independent precondition of a vulnerable window. Names are
constant and globally unique; a check fires a marker by constructing the
precondition and observing it, never by observing the defect. A marker with no
firing check is an open obligation, not a passed one.

| Marker | Harness | Semantics | Records | Fired by |
| --- | --- | --- | --- | --- |
| `packing.identity.byte_twins_share_one_payload_row` | retrieval | `sometimes` | packing-attribution-follows-the-occurrence-row | `crates/retrieval/tests/packing_identity.rs` `byte_twins_share_one_payload_row_and_keep_their_own_attribution`: two raw tool spans with equal bytes persist as one payload row and two occurrence rows. |
| `packing.identity.payload_keyed_collapse_loses_a_twin` | retrieval | `reachable` | packing-attribution-follows-the-occurrence-row | The same test keys the read set by payload reference and observes one survivor whose sensitivity differs from the lost twin's. |
| `packing.identity.forced_payload_collision_presented` | retrieval | `sometimes` | packing-attribution-follows-the-occurrence-row | `crates/retrieval/tests/occurrences.rs` `forced_collisions_refuse_unequal_values_and_replay_keeps_identities`: differing bytes are presented under an existing payload digest through the test-only digest seam. |
| `packing.identity.same_parent_at_another_revision_selected` | retrieval | `sometimes` | packing-group-key-is-never-parent-alone | `crates/retrieval/tests/packing_identity.rs` `grouping_keys_need_parent_revision_and_representation_together`: one tool call's span at revision 1 and revision 2 both derive keys. |
| `packing.identity.same_parent_at_another_representation_selected` | retrieval | `sometimes` | packing-group-key-is-never-parent-alone | The same test derives keys for `tool_output` and `tool_error` of one tool call. |
| `packing.identity.non_grouping_class_selected` | retrieval | `sometimes` | packing-group-key-is-never-parent-alone | `crates/retrieval/tests/packing_identity.rs` `classes_outside_the_grouping_set_yield_the_typed_non_grouping_result`: every class outside the grouping set is derived. |
| `packing.identity.stored_column_disagrees_with_tuple` | retrieval | `sometimes` | packing-group-key-is-never-parent-alone, packing-attribution-follows-the-occurrence-row | `crates/retrieval/tests/packing_identity.rs` `grouping_refuses_columns_that_disagree_with_the_tuple` presents revision, representation, and span against a tuple carrying another value; `reads_refuse_unknown_identities_oversized_selections_and_disagreeing_columns` rewrites each stored column under the tuple, then replaces the tuple with another occurrence's, and reads the row back. |
| `packing.identity.payload_identifier_read_as_occurrence` | retrieval | `reachable` | packing-attribution-follows-the-occurrence-row | `byte_twins_share_one_payload_row_and_keep_their_own_attribution` parses the shared payload identifier as an occurrence identifier and reads it; the read refuses it as unknown. |
| `packing.required.missing_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | `crates/daemon/tests/packing_required.rs` `each_required_fault_yields_exactly_one_class_with_zero_optional_events`: a request names an unpersisted span. |
| `packing.required.stale_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | The same test presents a revision-2 request against a revision-1 row and a tombstoned row. |
| `packing.required.excluded_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | `a_required_occurrence_the_kernel_excludes_is_ineligible_not_missing` judges a row whose object the kernel never saw (`Retracted`); `crates/retrieval/tests/packing_required.rs` `every_verdict_maps_to_exactly_one_failure_class` presents every excluded verdict. |
| `packing.required.corrupt_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | `corrupt_payload_bytes_and_foreign_tuples_are_refused_as_corrupt` rewrites payload bytes and replaces a tuple. |
| `packing.required.oversized_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | `each_required_fault_yields_exactly_one_class_with_zero_optional_events` sets each of the three size bounds below the fixture; `more_requests_than_the_load_bound_are_refused_before_any_read` shows the load bound refusing before the first read. |
| `packing.required.over_budget_required_presented` | daemon | `sometimes` | packing-required-phase-precedes-optional-work, packing-required-bytes-are-charged-untruncated | `required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation` sets the limit one below the required cost. |
| `packing.required.budget_exhausted_before_optional` | daemon | `sometimes` | packing-required-phase-precedes-optional-work | `an_expired_deadline_refuses_the_required_phase_before_any_optional_event` presents an expired and a cancelled `EvalBudget`. |
| `packing.budget.malformed_budget_presented` | daemon | `sometimes` | packing-budget-is-an-integer-never-clamped | `crates/daemon/src/packing.rs` `budgets_are_integers_and_never_clamped` and `the_retained_malformed_budget_clamp_is_not_inherited`. |
| `packing.required.payload_beyond_legacy_cut` | daemon | `sometimes` | packing-required-bytes-are-charged-untruncated | `crates/daemon/src/packing.rs` `the_sixty_four_kib_silent_cut_is_not_inherited`; `crates/daemon/tests/packing_required.rs` `a_required_payload_beyond_the_legacy_cut_is_materialized_and_charged_whole`. |

Unfired markers by harness: none named yet. The OpenCode and Pi sets are
frozen by the Q8 owner before U5a and U5b implement against them.
