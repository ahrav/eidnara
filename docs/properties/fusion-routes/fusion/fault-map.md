# Fusion identity fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base:
`8e0491225a7292ef077c675d44b94f94a24041d3`.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Equal payload, distinct tuple | `kernel::source_identity::encode` over one buffer with one component varied. | fusion-occurrence-identity-never-collapses-payload |
| Duplicate and permuted hits | `LaneRanking::consolidate` over a fixed-seed shuffled list. | fusion-occurrence-identity-never-collapses-payload |
| Non-canonical identifier spelling, duplicate lane, mixed encoding version | `OccurrenceId::parse`, `DeclaredLanes::admit`. | fusion-occurrence-identity-never-collapses-payload |
| Single-component digest change, component split | `SelectionDigest::derive`, `PreparationDigest::derive`. | fusion-selection-digest-tracks-identity-tuple |
| Altered derived column | `ParentGroupKey::derive` with a revision, representation, or span that disagrees with the tuple. | fusion-parent-groups-are-not-voters |

Every seam is a pure function; no fault injection framework is needed for
this part.
