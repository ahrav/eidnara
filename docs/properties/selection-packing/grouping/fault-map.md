# Grouping and optional-scan fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `89c5589e`.

| Fault or state | Available seam | Marker | Records |
| --- | --- | --- | --- |
| Overlapping, adjacent, and disjoint spans of one key | The oracle parent fixture and `group`. | `packing.grouping.overlap_adjacent_disjoint_presented` | packing-groups-carry-only-selected-bytes |
| Disjoint pair with a multibyte character in the gap | The `gap_with_multibyte_character` fixture case and the gap-filling comparison. | `packing.grouping.multibyte_gap_presented` | packing-groups-carry-only-selected-bytes |
| Same parent at another revision or representation | `tool_span` variants in one selected set. | `packing.grouping.other_revision_or_representation_presented` | packing-groups-carry-only-selected-bytes |
| Reversed, empty, overflowing, out-of-parent, disagreeing, or UTF-8-splitting span | `damaged_span` rows. | `packing.grouping.ungroupable_span_presented` | packing-coverage-is-a-per-identity-partition |
| Cost list 11, 4, 6 against remaining 10 | `skip_and_continue` and the daemon entry with `ByteEstimator`. | `packing.scan.non_fitting_group_before_fitting_groups` | packing-optional-scan-skips-and-continues |
| Generated selected sets and cost lists, with byte-flip, end-stretch, reversed-span, and empty-span damage plus whole-object rows | The seeded differential proptest. | `packing.scan.generated_set_compared_to_reference` | packing-groups-carry-only-selected-bytes, packing-optional-scan-skips-and-continues |
| Each optional bound one below the set | `OptionalBounds` fixture values. | `packing.optional.bound_at_limit_plus_one_presented` | packing-optional-bounds-refuse-at-limit-plus-one |
| Optional occurrence duplicated, missing, stale, or kernel-excluded | A repeated request, an unpersisted request, a revision-2 request, a tombstoned row, an unseeded object. | `packing.optional.excluded_optional_presented` | packing-coverage-is-a-per-identity-partition |

Every seam is a pure function or an in-process store; no fault injection
framework is needed for this part. Marker status lives in
`../marker-ledger.md`.
