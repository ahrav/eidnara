# Selection identity fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base:
`cb259ee05a610ad56beea8f3bd2c414e1b3eb771`.

| Fault or state | Available seam | Marker | Records |
| --- | --- | --- | --- |
| Equal payload bytes at two identities | `persist_occurrences` over one buffer with two tool calls. | `packing.identity.byte_twins_share_one_payload_row` | packing-attribution-follows-the-occurrence-row |
| Attribution keyed by payload reference | A map from `PayloadRef::payload_id` over the read set. | `packing.identity.payload_keyed_collapse_loses_a_twin` | packing-attribution-follows-the-occurrence-row |
| Payload identifier presented as an occurrence identifier | `OccurrenceId::parse` over `PayloadRef::payload_id`, then `read_selected`. | `packing.identity.payload_identifier_read_as_occurrence` | packing-attribution-follows-the-occurrence-row |
| Differing bytes under one payload digest | `persist_occurrences_with_digests_for_test` (`test-support`). | `packing.identity.forced_payload_collision_presented` | packing-attribution-follows-the-occurrence-row |
| Same parent, other revision | `Grouping::derive` over the tool call at revision 2. | `packing.identity.same_parent_at_another_revision_selected` | packing-group-key-is-never-parent-alone |
| Same parent, other representation | `Grouping::derive` over `tool_error`. | `packing.identity.same_parent_at_another_representation_selected` | packing-group-key-is-never-parent-alone |
| Class outside the grouping set | `Grouping::derive` over every `OccurrenceClass::ALL` member. | `packing.identity.non_grouping_class_selected` | packing-group-key-is-never-parent-alone |
| Stored column, tuple bit, or whole tuple disagrees | `Grouping::derive` with an altered revision, representation, or span, or one flipped tuple bit; a raw connection rewriting the stored revision, span, representation, or class column of a persisted row of any class, or replacing the tuple with another occurrence's, before `read_selected`. | `packing.identity.stored_column_disagrees_with_tuple` | packing-group-key-is-never-parent-alone, packing-attribution-follows-the-occurrence-row |

Every seam is a pure function or an in-process SQLite store; no fault
injection framework is needed for this part. Marker status lives in
`../marker-ledger.md`.
