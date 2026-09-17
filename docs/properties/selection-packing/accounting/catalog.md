# RP2.8 rendered-delta accounting properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `aa69fca2` (the tip of the RP2.8 U3 branch the U4a change was authored
against). Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)), whose packing and
accounting contract and acceptance rows AC7 and AC8 name these obligations,
and the RP2.8 U4a ticket ([#634](https://github.com/ahrav/eidnara/issues/634))
that lands their executable checks.

This part owns the accounting profile, the revision-keyed cost cache, the
rendered-delta charge each admitted item carries, and the rendered-bytes and
estimated-tokens bounds. Serialization through the exact-byte guard and
overflow repair are the U4b part.

## Observation contract

The profile and charge types are `crates/daemon/src/packing/accounting.rs`;
the render ledger is `crates/daemon/src/packing/render.rs`; the cache is
`crates/daemon/src/token_cache.rs`. `crates/daemon/tests/packing_accounting.rs`
exercises them directly and through `prepare_required`;
`crates/daemon/tests/packing_optional.rs` observes the ledger the optional
phase closes. The exact profile counts through `tokenizer::estimate_tokens`;
the test profile counts one token per rendered byte so limits sit on byte
boundaries.

## Q5 rulings

Recorded by the repository owner at the U4a change:

- The accounting revision is the length-prefixed triple of the constructing
  authority, the profile identity, and its revision source: for the exact
  profile the SHA-256 of the embedded Claude BPE vocabulary blob
  (`5:exact;10:claude-bpe;64:<digest>`), for a heuristic its stated
  degradation (`9:heuristic;<n>:<identity>;<m>:<degradation>`). The authority
  tag keeps a heuristic that repeats the exact identity and digest from
  sharing the exact cache key. No manifest version takes part.
- The daemon owns the accounting profile end to end; the wire carries only the
  profile identity and revision for the harness to echo and the daemon to
  validate at apply. The binding is `AccountingBinding` in
  `crates/daemon/src/edit_receipts.rs`, answered on `retrieval.prepare`,
  required on `retrieval.apply`, and part of the preparation digest; its
  records are `docs/properties/fusion-routes/application/catalog.md`
  `apply-stale-preparation-is-rejected-before-edit`.
- The separator the legacy composer emits before the memory block is a
  declared exclusion named `separator-before-memory-block`, reported by every
  profile's `declared_uncharged`; the packer's own render has no uncharged
  byte.
- For packed required spans the 64 KiB per-line cut is a defect, not a bound:
  the packing path never cuts; the legacy memory-line render keeps its own cut
  outside this ticket.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-charge-equals-rendered-delta](#packing-charge-equals-rendered-delta) | safety | test-only | always | active | high |
| [packing-cost-cache-keyed-by-accounting-revision](#packing-cost-cache-keyed-by-accounting-revision) | safety | default-production | always | active | high |
| [packing-heuristic-counts-never-carry-the-exact-label](#packing-heuristic-counts-never-carry-the-exact-label) | safety | test-only | always | active | high |
| [packing-accounting-bounds-refuse-at-limit-plus-one](#packing-accounting-bounds-refuse-at-limit-plus-one) | safety | test-only | always | active | high |

## Records

### packing-charge-equals-rendered-delta

Type: safety
Reachability: test-only - `Ledger` is filled by `prepare_required` and
`prepare_optional`, which have no production caller at this base (`grep -rn
'prepare_required\|prepare_optional' crates --include=*.rs` finds the daemon
packing module and its tests).
Status: active
Exercised: yes - `crates/daemon/tests/packing_accounting.rs`
`every_charge_equals_the_whole_render_delta_and_every_byte_is_charged`,
`a_tail_run_longer_than_the_lookback_still_charges_the_whole_render_delta`,
and `consumed_budget_plus_remaining_is_the_token_limit_under_every_profile`;
`crates/daemon/tests/packing_optional.rs`
`optional_groups_are_admitted_by_skip_and_continue_over_the_remaining_budget`,
`same_parent_spans_group_and_are_charged_as_one_merged_range`, and
`a_groups_priced_cost_equals_its_charged_entries_and_the_budget_covers_the_closed_render`;
`crates/daemon/tests/packing_required.rs`
`required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation`;
`crates/daemon/src/packing/render.rs`
`ledger_deltas_bypass_the_shared_cache_and_tokenize_a_bounded_tail`;
`crates/tokenizer/src/lib.rs`
`suffix_anchor_is_a_true_piece_start_when_the_window_opens_on_an_apostrophe`
and `suffix_anchor_is_the_last_piece_start_in_the_window`.
Guarantee: Every item the packer admits is charged the estimate of the
rendered prefix plus item minus the estimate of the rendered prefix, including
wrappers, separators, and escapes; a group wrapper is charged once at the
group's first admitted member as its own ledger entries; the cost the scan
deducts for a group equals the headroom-adjusted sum of the entries the
ledger charges for it; the token budget covers the closed render, so the
budget consumed equals the closed ledger's headroom-adjusted total; the sum
of charges equals the whole-render estimate; every rendered byte belongs to
exactly one charge, and a charge gap is a safety failure.
Check: `always` - for generated fragment sequences, including non-ASCII and
XML-significant bytes, under the exact profile, a linear byte profile, and a
non-linear heuristic with headroom, each appended fragment's charge equals the
whole-prefix delta computed over the whole render, the sum of charges equals
the estimate of the final text, and the entries' byte counts sum to the
render's length; some generated renders exceed the lookback so the
piece-anchored path is the one compared, and a tail run of one character
class longer than the lookback still matches; a group's scan cost equals the
byte length of its whole fragment under the byte profile and the wrapper
appears as `GroupOpen` and `GroupClose` entries; under the byte profile, a
non-linear heuristic with 250 permille headroom, and the exact profile, each
admitted group's cost equals the headroom-adjusted sum of its `GroupOpen`,
`Range`, and `GroupClose` entries and the token limit minus the remaining
budget equals the closed ledger's headroom-adjusted total; at every token
limit from 40 to 400 and one generous limit, under each profile, an admitted
render's headroom-adjusted total plus the remaining budget is the limit and
the sweep reaches zero, one, and two admitted groups; the anchor is a piece
start of the full scan when the window opens on the apostrophe of an `Other`
run, and it is the last piece start when three or more pieces fit the window.
`always` because one under-charged item is an over-budget edit.
Fault/timing angle: none.
Required faults and enabling state: Generated fragments with XML-significant
bytes and lengths up to 2500, so escapes and multi-piece boundaries occur; a
window opening on an apostrophe; token limits swept across exact fills.
Confidence: high - [evidence](evidence/packing-charge-equals-rendered-delta.md).
The oracle recomputes the delta over the whole prefix, independent of the
production anchor; the exact profile's anchor is the last piece start the
tokenizer's own scanner trusts in the window and moves forward only once the
tail outgrows `ANCHOR_ADVANCE_BYTES`, and heuristics are re-estimated over the whole render
because they promise no locality. A group is priced by `Ledger::stage` as the
entries it would be charged as and admitted by `Ledger::commit` of that same
pricing, so the deducted cost and the charged entries are one computation;
the block close is reserved from the optional budget before the scan and
settled at its charged value.
Existing check: none before this change; the U2 reservation charged raw
payload bytes.
Impact: A charge that omits a wrapper or an escape lets the serialized body
exceed the provider limit the caller trusted.
Open questions: None.

### packing-cost-cache-keyed-by-accounting-revision

Type: safety
Reachability: default-production - every existing caller of
`cached_estimate_tokens` and `count_with_digest` now keys under the exact
tokenizer revision.
Status: active
Exercised: yes - `crates/daemon/src/token_cache.rs`
`a_count_cached_under_one_revision_is_not_served_under_another` and the
retained cache tests; `crates/daemon/tests/packing_accounting.rs`
`a_count_cached_under_one_revision_is_not_served_under_another_profile` and
`a_heuristic_impersonating_the_exact_revision_shares_neither_revision_nor_cache_entry`.
Guarantee: A count cached under one accounting revision is never served under
another; the same content under the same revision hits; generation rotation
and clearing preserve both.
Check: `always` - two revisions counting the same content return their own
counts on the first and every later call; rotating `current` into `previous`
keeps each revision's entry; the exact revision string is
`5:exact;10:claude-bpe;64:` plus a 64-hex vocabulary digest, two component
pairs that would collide under a plain separator derive different revisions,
and a heuristic constructed from the exact identity and vocabulary digest has
a different revision and its counts are never served under the exact label. `always` because a
cross-revision hit is a silent mis-charge.
Fault/timing angle: Concurrent misses may count the same content twice; both
insert the same value under the same key.
Required faults and enabling state: Two profiles with different revisions and
different counting functions over one content; a heuristic whose identity and
degradation repeat the exact profile's identity and vocabulary digest.
Confidence: high - [evidence](evidence/packing-cost-cache-keyed-by-accounting-revision.md).
Existing check: `crates/daemon/src/token_cache.rs`
`kind_prefixed_and_raw_content_keys_do_not_alias` covered content-key domain
separation before this change; it now runs under the revisioned key.
Impact: A tokenizer or profile change would serve stale counts to every
session until the cache rotated.
Open questions: None.

### packing-heuristic-counts-never-carry-the-exact-label

Type: safety
Reachability: test-only - no production heuristic profile exists at this base.
Status: active
Exercised: yes - `crates/daemon/tests/packing_accounting.rs`
`a_heuristic_count_carries_its_authority_and_headroom_and_never_the_exact_label`;
the doctests on `Charge` in `crates/daemon/src/packing/accounting.rs`: a
passing example that reaches `AccountingProfile::charge` through the same
paths, and a `compile_fail` struct literal.
Guarantee: Every count carries the authority of the profile that produced
it; a heuristic profile's charges are `Heuristic` with a named degradation and
headroom in permille; `with_headroom` adds the headroom rounded up and adds
nothing for the exact authority; a `Charge` cannot be constructed outside the
profile, so the exact label cannot be forged.
Check: `always` - a heuristic profile's charge reports its degradation and
headroom, and `with_headroom` on 3 tokens at 250 permille is 4; the exact
profile's authority is `Exact` and its headroom adds nothing; the struct
literal for `Charge` fails to compile while the passing doctest beside it
proves the paths resolve, because stable rustdoc ignores the `E0451` code and
a `compile_fail` block alone would also pass on a stale path. `always` because
an exact label on a heuristic count would claim provider proof the count does
not have.
Fault/timing angle: none.
Required faults and enabling state: A heuristic profile with a non-zero
headroom.
Confidence: high - [evidence](evidence/packing-heuristic-counts-never-carry-the-exact-label.md).
Existing check: none before this change.
Impact: A heuristic count labeled exact would pass U5a's whole-invocation
validation with no headroom.
Open questions:

- Whether heuristic headroom is an approved limit or a policy constant is
  RP2.9's Q4; tests use fixture values. (needs human input)

### packing-accounting-bounds-refuse-at-limit-plus-one

Type: safety
Reachability: test-only - same as the first record.
Status: active
Exercised: yes - `crates/daemon/tests/packing_accounting.rs`
`rendered_bytes_and_estimated_tokens_bounds_refuse_at_limit_plus_one`,
`the_required_phase_refuses_a_render_beyond_the_accounting_bounds`, and
`the_optional_phase_refuses_a_render_beyond_the_accounting_bounds_and_a_foreign_profile`.
Guarantee: The rendered-bytes and estimated-tokens bounds admit a render at
their supplied limit and refuse one past it with an `AccountingExceeded`
naming the bound, the value, and the limit, under each enabled profile, at the
end of the required phase and at the end of the optional phase; the token
bound compares the headroom-adjusted total, the same quantity the budget
consumed. The optional phase refuses a profile other than the one the required
ledger was charged under.
Check: `always` - a closed ledger at exactly both limits is admitted; each
bound reduced by one refuses with its own name and values; through the
required entry and through the optional entry the refusal is
`PreparationRefusal::Accounting`; a foreign profile at the optional entry is
`PreparationRefusal::ProfileMismatch`. `always` because every bound is a
fail-closed limit.
Fault/timing angle: none.
Required faults and enabling state: Each bound set one below the render in
isolation.
Confidence: high - [evidence](evidence/packing-accounting-bounds-refuse-at-limit-plus-one.md).
Existing check: none before this change.
Impact: An unbounded render emits over-budget bytes.
Open questions:

- The approved numeric values belong to RP2.9; tests use fixture values and
  claim no production approval. (needs human input)

## Relationship map

- `required/` charges the block open and each required fragment through this
  part's `Ledger` and reserves them against the integer budget; its
  `OverBudget` refusal reports `charged` as the headroom-adjusted ledger
  total, the quantity the delta record fixes. The `remaining` it hands the
  optional phase is the budget less that total.
- `grouping/` decides which groups the scan visits; this part prices each
  group as its wrapper and range entries (the delta record's wrapper clause)
  and commits exactly the priced entries, so the scan's deducted cost equals
  the ledger's charge. A group whose priced sum is unrepresentable refuses the
  phase with `OptionalCostOverflow`, which `grouping/` maps and this part does
  not own.
- Within this part, the revision record keys the shared cache the exact
  profile's `charge` reads; the delta record's ledger bypasses that cache by
  design. The authority record keeps a heuristic's charge, headroom, and
  revision distinct from the exact profile's, and the bounds record compares
  the headroom-adjusted total those charges produce.
- The block close is reserved at optional entry and re-priced after the
  admitted groups; a required render that cannot cover the reserve refuses
  as `OverBudget`, and a close priced above its reserve refuses as
  `CloseOverBudget`. Both are exercised by
  `crates/daemon/tests/packing_optional.rs` and carry no record here; the
  exact profile's delta is local to the tail, so only a heuristic whose count
  depends on the whole prefix can drift.
- `identity/` is not consumed directly: the ledger charges bytes the grouping
  already attributed and never re-derives a key.
