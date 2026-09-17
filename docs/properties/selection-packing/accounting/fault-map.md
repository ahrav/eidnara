# Accounting fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `aa69fca2`.

| Fault or state | Available seam | Marker | Records |
| --- | --- | --- | --- |
| Rendered prefix longer than the lookback, or a same-class tail run longer than it | Generated fragments totalling more than `DELTA_LOOKBACK_BYTES`; explicit runs of spaces, letters, digits, punctuation, and newlines. | `packing.accounting.render_beyond_delta_window` | packing-charge-equals-rendered-delta |
| Fragments with XML-significant or non-ASCII bytes | The generated fragment alphabet includes `<`, `>`, `&`, quotes, `é`, an ideographic space, and an emoji. | `packing.accounting.escaped_fragment_charged` | packing-charge-equals-rendered-delta |
| Window opening on the apostrophe of an `Other` run | `suffix_anchor` unit tests in `crates/tokenizer/src/lib.rs` place `.'re` and each other contraction at the window start. | `packing.accounting.anchor_window_opens_on_apostrophe` | packing-charge-equals-rendered-delta |
| Token limits swept across exact fills | `consumed_budget_plus_remaining_is_the_token_limit_under_every_profile` runs both phases at every limit from 40 to 400 under each profile. | `packing.accounting.limit_swept_across_exact_fills` | packing-charge-equals-rendered-delta |
| Group admitted with several ranges | `tool_range` spans of one call through `prepare_optional`, under the byte profile, a 250 permille headroom heuristic, and the exact profile. | `packing.accounting.group_wrapper_charged_once` | packing-charge-equals-rendered-delta |
| Two revisions over one content | Two heuristic profiles with different counting functions; a heuristic named after the exact identity and vocabulary digest; the cache-level test with two `AccountingRevision` values and a forced rotation. | `packing.accounting.two_revisions_one_content` | packing-cost-cache-keyed-by-accounting-revision |
| Heuristic profile with headroom | `AccountingProfile::heuristic` with 250 permille. | `packing.accounting.heuristic_profile_charged` | packing-heuristic-counts-never-carry-the-exact-label |
| Struct literal for a charge | The `compile_fail` doctest on `Charge`. | `packing.accounting.exact_label_construction_attempted` | packing-heuristic-counts-never-carry-the-exact-label |
| Rendered bytes or tokens one past the bound; a foreign profile at the optional entry | `AccountingBounds` set from a closed ledger, then reduced by one, directly and through both entries; `RequiredInputs` with another profile. | `packing.accounting.bound_at_limit_plus_one_presented` | packing-accounting-bounds-refuse-at-limit-plus-one |

Every seam is a pure function, an in-process cache, or a constructed profile;
no fault injection framework is needed for this part. Marker status lives in
`../marker-ledger.md`.
