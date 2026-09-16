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
| `packing.identity.stored_column_disagrees_with_tuple` | retrieval | `sometimes` | packing-group-key-is-never-parent-alone, packing-attribution-follows-the-occurrence-row | `crates/retrieval/tests/packing_identity.rs` `grouping_refuses_columns_that_disagree_with_the_tuple` presents revision, representation, and span against a tuple carrying another value; `reads_refuse_unknown_identities_oversized_selections_and_disagreeing_columns` rewrites each stored column of a `raw_tool_spans` row under the tuple, relabels the row to a non-grouping class, then replaces the tuple with another occurrence's, and reads the row back; `non_grouping_rows_whose_columns_disagree_with_the_tuple_are_refused` does the same column rewrites for a persisted row of every non-grouping class. |
| `packing.identity.payload_identifier_read_as_occurrence` | retrieval | `reachable` | packing-attribution-follows-the-occurrence-row | `byte_twins_share_one_payload_row_and_keep_their_own_attribution` parses the shared payload identifier as an occurrence identifier and reads it; the read refuses it as unknown. |

Unfired markers by harness: none named yet. The OpenCode and Pi sets are
frozen by the Q8 owner before U5a and U5b implement against them.
