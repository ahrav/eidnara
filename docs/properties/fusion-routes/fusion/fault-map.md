# Fusion identity fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base:
`8e0491225a7292ef077c675d44b94f94a24041d3`.

| Fault or state | Available seam | Records |
| --- | --- | --- |
| Equal payload, distinct tuple | `kernel::source_identity::encode` over one buffer with one component varied. | fusion-occurrence-identity-never-collapses-payload |
| Duplicate and permuted hits | `LaneRanking::consolidate` over a fixed-seed shuffled list. | fusion-occurrence-identity-never-collapses-payload |
| Non-canonical identifier spelling, foreign encoding version, duplicate lane | `OccurrenceId::parse`, `LaneRanking::consolidate`, `DeclaredLanes::admit`. | fusion-occurrence-identity-never-collapses-payload |
| Single-component digest change, component split, malformed or whole-buffer span spelling | `SelectionDigest::derive`, `PreparationDigest::derive`. | fusion-selection-digest-tracks-identity-tuple |
| Shuffled lane order, doubled hits, non-calibration parameters | `DeclaredLanes::admit`, `fuse`, `FusionParameters::new` under a fixed seed. | fusion-one-contribution-per-lane, fusion-order-is-deterministic, fusion-rrf-formula-conformance |
| Invalid weight or `k`, rank-one overflow | `FusionParameters::new`. | fusion-parameters-validated-before-scoring |
| Union one past the bound | `fuse` with a `NonZeroUsize` bound. | fusion-union-bounded-before-materialization |
| Entry filtered after fusion | `Fused::filter`. | fusion-runs-once, fusion-positions-assigned-once, fusion-raw-scores-retained |
| Altered derived column or damaged tuple byte | `ParentGroupKey::derive` with a revision, representation, or span that disagrees with the tuple, or one flipped tuple bit. | fusion-parent-groups-are-not-voters |

Every seam is a pure function; no fault injection framework is needed for
this part.
