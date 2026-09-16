# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base:
`cb259ee05a610ad56beea8f3bd2c414e1b3eb771`. Every check below is `unaudited`:
source inspection establishes its presence and assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/retrieval/tests/occurrences.rs`, `forced_collisions_refuse_unequal_values_and_replay_keeps_identities` | A forced payload or tuple collision is refused with the typed error and writes no row; replay in another order inserts nothing. | unaudited | Persistence only; nothing reads the twins back or resolves attribution. |
| `crates/retrieval/tests/identity.rs`, `parent_groups_share_a_parent_across_spans_and_never_replace_occurrences` | Spans of one source share a parent-plus-revision key; a revision, representation, or object change derives another; a disagreeing column is refused. | unaudited | The RP2.7 key omits class and representation as components and has no non-grouping result. |
| `crates/retrieval/tests/identity.rs`, `equal_payload_bytes_at_different_identities_stay_distinct_ranking_units` | Byte twins keep two occurrence identifiers and two lane entries. | unaudited | Ranking units, not stored rows or attribution. |
| `crates/retrieval/src/eligibility.rs`, `judge_occurrences` callers in `crates/retrieval/tests/dense_oracle.rs` | Candidates are judged in one kernel batch and reported in order. | unaudited | Candidates come from `live_candidates`, not from a selected read. |

Suspiciously quiet areas: no check read a stored occurrence without loading its
payload bytes before this part, and no check derived a key for a class outside
the grouping set.
