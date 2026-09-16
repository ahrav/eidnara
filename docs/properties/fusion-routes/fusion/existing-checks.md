# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base:
`8e0491225a7292ef077c675d44b94f94a24041d3`. Every check below is `unaudited`:
source inspection establishes its presence and assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/kernel/src/source_identity.rs`, `identity_matching_requires_the_complete_exact_prefix` | Tuple prefix matching fails on any damaged byte or wrong field. | unaudited | Covers the encoder, not a consumer's ranking unit. |
| `crates/retrieval/tests/lexical_retrieval.rs`, `contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation` | Lexical contributions keep one entry per occurrence under probe permutation and duplication. | unaudited | Lexical lane only; no cross-lane declaration. |
| `crates/kernel/src/envelope.rs`, `operation_identity` | Commit receipt identity length-delimits components. | unaudited | Commit intents, not selections or preparations. |

Suspiciously quiet areas: no check exercised a dense or exact lane declared as
a ranking before this part, no check derived a parent identifier, and no
check fused two lanes.
