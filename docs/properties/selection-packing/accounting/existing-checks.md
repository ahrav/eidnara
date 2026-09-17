# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `aa69fca2`. Every check
below is `unaudited`: source inspection establishes its presence and
assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/daemon/src/token_cache.rs`, `cached_counts_match_the_tokenizer` | Cached counts equal the tokenizer's on first and second call. | unaudited | Content-keyed before this change; no revision. |
| `crates/daemon/src/token_cache.rs`, `kind_prefixed_and_raw_content_keys_do_not_alias` | Tail hygiene's kind-prefixed key and the raw key never alias. | unaudited | Domain separation of content keys, not of revisions. |
| `crates/daemon/src/token_cache.rs`, `insert_current_rotates_at_capacity` | Generation rotation keeps `current` bounded. | unaudited | Rotation, not revision isolation across rotation. |
| `crates/daemon/tests/packing_required.rs`, `required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation` | Required cost at the limit succeeds and one above fails. | unaudited | Charged raw payload bytes before this change; now charges the rendered delta. |
| `crates/tokenizer` golden tests | The BPE count matches the reference tokenizer except for documented defects. | unaudited | The exact authority's own correctness, not the packer's charging. |

Suspiciously quiet areas: no check compared a per-item charge to a whole-render
delta before this part, and no check exercised a heuristic estimator's label.
