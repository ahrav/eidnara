# RP2.7 fusion identity and arithmetic properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `8e0491225a7292ef077c675d44b94f94a24041d3` (the `main` commit the U1
change was authored against). Method: `../../METHOD.md` and
`property-discovery-and-catalog`.

Source: the RP2.7 specification
([#630](https://github.com/ahrav/eidnara/issues/630)) and its local companion
bundle, whose fusion catalog proposed these obligations as unexercised
`test-only` records. The RP2.7.U1 ticket
([#638](https://github.com/ahrav/eidnara/issues/638)) lands the identity
records and the RP2.7.U2 ticket
([#640](https://github.com/ahrav/eidnara/issues/640)) lands the arithmetic
records under the bundle's slugs.

This part owns identity and arithmetic: the occurrence ranking unit, lane
consolidation, the selection and preparation digests, parent groups, and
weighted reciprocal rank fusion over declared lane rankings. Route budget,
authorization, and application lifecycle are separate parts.

Parent Q1 decisions recorded here: the exact lane is a set whose members tie,
so its declared ranking is occurrence-identifier order; terms are summed in
`f64` in `Lane::ORDER`; the reference oracle sums terms and never evaluates a
closed fraction; a negative-zero weight is admitted as `+0.0`; a parameter set
whose finite inputs would sum to infinity at rank one is refused before any
occurrence is scored; an incomplete lane participates with the entries it
reached, and reporting its completion beside the ranking is the route's
obligation in the query-route part, since `LaneRanking` carries no completion
state; an undeclared lane, including a dense lane reported unavailable,
contributes zero and is listed by `Fused::undeclared_lanes` rather than treated
as an error.

## Observation contract

The observation point is the public API of `crates/retrieval/src/fusion/`,
exported at `crates/retrieval/src/lib.rs:23` and exercised by
`crates/retrieval/tests/identity.rs` and `crates/retrieval/tests/fusion.rs`.
Kernel occurrence encoding is reused through `kernel::source_identity`, never
restated. Each record carries its own reachability class and the evidence for
it.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [fusion-occurrence-identity-never-collapses-payload](#fusion-occurrence-identity-never-collapses-payload) | safety | test-only | always | active | high |
| [fusion-selection-digest-tracks-identity-tuple](#fusion-selection-digest-tracks-identity-tuple) | safety | test-only | always | active | high |
| [fusion-parent-groups-are-not-voters](#fusion-parent-groups-are-not-voters) | safety | test-only | always | active | high |
| [fusion-one-contribution-per-lane-per-occurrence](#fusion-one-contribution-per-lane-per-occurrence) | safety | test-only | always | active | high |
| [fusion-lane-positions-are-assigned-once-from-declared-lane-order](#fusion-lane-positions-are-assigned-once-from-declared-lane-order) | safety | test-only | always | active | high |
| [fusion-score-matches-weighted-rrf-reference](#fusion-score-matches-weighted-rrf-reference) | safety | test-only | always | active | high |
| [fusion-order-is-deterministic-and-permutation-invariant](#fusion-order-is-deterministic-and-permutation-invariant) | safety | test-only | always | active | high |
| [fusion-parameters-are-validated-before-scoring](#fusion-parameters-are-validated-before-scoring) | safety | test-only | always | active | high |
| [fusion-raw-scores-are-retained-and-never-compared](#fusion-raw-scores-are-retained-and-never-compared) | safety | test-only | always | active | high |
| [fusion-union-bound-is-enforced-before-materialization](#fusion-union-bound-is-enforced-before-materialization) | safety | test-only | always | active | high |
| [fusion-fuses-once-before-revalidation](#fusion-fuses-once-before-revalidation) | safety | test-only | always | active | high |

## Records

### fusion-occurrence-identity-never-collapses-payload

Type: safety
Reachability: test-only - `OccurrenceId::parse`, `LaneRanking::consolidate`,
and `DeclaredLanes::admit` are called from `crates/retrieval/tests/identity.rs`
only; `grep -rn 'fusion::' crates --include=*.rs` outside
`crates/retrieval/src/fusion/` finds that test file,
`crates/retrieval/tests/fusion.rs`, and two comments
(`crates/daemon/src/projection_lifecycle.rs:227`,
`crates/daemon/tests/projection_lifecycle.rs:796`), so no route builds a lane
ranking at this base.
Status: active
Exercised: yes - `crates/retrieval/tests/identity.rs`
`equal_payload_bytes_at_different_identities_stay_distinct_ranking_units`,
`a_lane_admits_one_entry_per_occurrence_with_the_lane_own_best_score`,
`consolidation_ignores_probe_order_and_duplication`,
`a_lane_refuses_foreign_scores_non_finite_scores_and_other_encoding_versions`,
`only_the_lowercase_hex_spelling_of_an_identifier_is_admitted`, and
`declared_lanes_hold_one_ranking_per_lane_in_fixed_order`.
Guarantee: The fusion ranking unit is the kernel occurrence identifier; equal
payload bytes at different source, revision, representation, or span identities
stay distinct ranking units, and a lane holds exactly one entry per occurrence
whatever probes or generations discovered it.
Check: `always` - for occurrences with identical payload bytes and differing
tuples, the occurrence identifiers differ and a lane ranking built from both
holds two entries; a ranking built from any multiset of hits holds one entry
per distinct occurrence with the lane's own best raw score, and permuting or
duplicating the hits yields a bit-identical ranking; a non-canonical identifier
spelling, a second ranking for one lane, a hit scored for another lane or with
a non-finite score, or a lane stamped with an encoding
version other than the kernel's is refused before any ranking is admitted. `always` because
the property must hold on every construction, not only at a rare state.
Fault/timing angle: none; the types are pure values.
Required faults and enabling state: Two occurrences sharing `BUFFER` with one
tuple component changed; hits for one occurrence from several probe ordinals
and generations; a fixed-seed shuffle plus duplication of a random hit list;
uppercase, prefixed, truncated, and extended identifier spellings.
Confidence: high - [evidence](evidence/fusion-occurrence-identity-never-collapses-payload.md).
The tests run against the real kernel encoder and the retrieval consolidation
that fusion will consume.
Existing check: `crates/retrieval/tests/lexical_retrieval.rs`
`contributions_follow_the_reference_order_and_survive_probe_duplication_and_permutation`
covers the lexical lane's own per-occurrence dedup, status unaudited; the
kernel `source_identity` tests cover tuple prefix matching, status unaudited.
Impact: A payload digest standing in for an occurrence would merge distinct
evidence, and a probe or generation voting twice would inflate one result.
Open questions: None.

### fusion-selection-digest-tracks-identity-tuple

Type: safety
Reachability: test-only - `SelectionDigest::derive`, `PreparationDigest::derive`,
and `SelectedSpan::new` have no caller outside
`crates/retrieval/tests/identity.rs` (same grep as the record above); the
prepare route that would derive them is RP2.7.U4 work
(`docs/fusion-identity-contract.md:6-7`).
Status: active
Exercised: yes - `crates/retrieval/tests/identity.rs`
`selection_digest_tracks_order_and_membership`,
`selected_spans_normalize_the_whole_buffer_and_refuse_malformed_ranges`, and
`preparation_digest_tracks_every_component_and_never_merges_component_splits`.
Guarantee: The selection digest changes whenever fused order or membership
changes, the preparation digest changes whenever the context revision,
representation, any selected span, or the selection changes, and no two input
tuples with different component splits derive one digest.
Check: `always` - the digest of a selection equals itself and differs from the
digest of any reordering, extension, or truncation; the preparation digest
differs for each single-component change including a span bound, a
whole-buffer versus range selection, span order, span count, and selection; a
whole-buffer range normalizes to the one whole-buffer spelling and a reversed
or out-of-range span is refused; the
pair `("ab", "c")` and `("a", "bc")` derive different digests. `always`
because every derivation must be sensitive to every component.
Fault/timing angle: none; derivation is a pure function.
Required faults and enabling state: A two-occurrence selection and its
reorderings; a preparation input set with one component varied at a time; a
component-split pair.
Confidence: high - [evidence](evidence/fusion-selection-digest-tracks-identity-tuple.md).
Length delimiting is written before every component and a count before every
sequence, following the kernel operation-identity derivation.
Existing check: `crates/kernel/src/envelope.rs` `operation_identity` applies
the same length-prefix discipline to commit intents, status unaudited; none
for selection or preparation digests before this change.
Impact: A stale or foreign preparation could pass as the one prepared, and a
retry could apply an edit the caller never selected.
Open questions:

- Whether the preparation digest also binds the accounting profile is decided
  by RP2.7.U4 with RP2.8; the idempotency key binds it separately today.
  (needs human input)

### fusion-parent-groups-are-not-voters

Type: safety
Reachability: test-only - `ParentGroupKey::derive` has no caller outside
`crates/retrieval/tests/identity.rs` (same grep as the first record); `fuse`,
the only scoring entry point, is called from `crates/retrieval/tests/fusion.rs`
only.
Status: active
Exercised: yes - the identity clauses by `crates/retrieval/tests/identity.rs`
`parent_groups_share_a_parent_across_spans_and_never_replace_occurrences`;
the fusion clause by the type of `retrieval::fusion::fuse`, which accepts only
`DeclaredLanes` of occurrence entries, so no parent key can reach a score, and
by `crates/retrieval/tests/fusion.rs`
`lane_order_probe_duplication_and_entry_order_change_nothing`, whose entry
count equals the distinct occurrence count of the input.
Guarantee: A parent group key is parent identity plus the occurrence's
revision, every span of one source at one representation shares the parent,
a parent identifier never equals an occurrence identifier, and grouping never
substitutes for occurrence identity in a lane ranking.
Check: `always` - whole-buffer and span occurrences of one source, revision,
and representation derive one group key; a revision, representation, or
object change derives another; the parent identifier equals the whole-buffer
lineage identifier and is absent from the occurrence identifier set; a lane
ranking over the spans holds one entry per span; a derived column that
disagrees with the tuple bytes is refused, and no single flipped tuple bit
derives the same key. `always` because the key must be a
pure function of the tuple on every derivation.
Fault/timing angle: none for the identity clauses.
Required faults and enabling state: Occurrences of one canonical claim at a
whole-buffer span and two ranges; the same ranges at a later revision and at
another representation; a tuple presented with an altered revision,
representation, or span.
Confidence: high - [evidence](evidence/fusion-parent-groups-are-not-voters.md).
The identity clauses are exercised and the fusion input type admits no parent.
Existing check: none before this change.
Impact: A parent key used as a voter would let one source outvote another by
span count, and a group spanning revisions would mix bytes from two versions.
Open questions: None. RP2.8 U1 ruled that only `raw_tool_spans` groups;
`docs/properties/selection-packing/identity/catalog.md` records the ruling
and the class-and-representation key built over this one.

### fusion-one-contribution-per-lane-per-occurrence

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`lane_order_probe_duplication_and_entry_order_change_nothing` and
`a_hand_computed_two_lane_example_matches_term_for_term`.
Guarantee: Each lane adds at most one term per occurrence to the fused score,
however many probes or generations discovered the occurrence.
Check: `always` - a fused ranking over doubled and shuffled hits in every lane
is bit-identical, as `(occurrence, position, score bits)` triples, to the
ranking over the original hits; the fused entry count equals the distinct
occurrence count across lanes; the fused score of a duplicated occurrence
equals the single-term oracle and differs from the per-probe sum that adds the
lane term once per duplicate. `always` because the property must hold for
every input multiset. Duplicates with differing raw scores are consolidated at
the lane boundary, exercised by `crates/retrieval/tests/identity.rs`.
Fault/timing angle: none; fusion is a pure function.
Required faults and enabling state: Random lane sets under a fixed seed with
every hit duplicated and the lane list shuffled.
Confidence: high - [evidence](evidence/fusion-one-contribution-per-lane-per-occurrence.md).
Consolidation keys by occurrence before fusion and fusion reads one entry per
occurrence per lane.
Existing check: `crates/retrieval/tests/identity.rs`
`consolidation_ignores_probe_order_and_duplication` for the lane half, status
unaudited.
Impact: A probe or generation voting twice would inflate one result over a
stronger single match.
Open questions: None.

### fusion-lane-positions-are-assigned-once-from-declared-lane-order

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`lane_order_probe_duplication_and_entry_order_change_nothing`,
`empty_lanes_all_zero_weights_and_equal_scores_return_the_declared_result`,
and, for the filter clause,
`raw_scores_survive_and_filtering_keeps_positions_and_scores_without_rescoring`.
Guarantee: Lane positions are assigned once by consolidation as `1..=n` in the
lane's own order, fused positions are assigned once as `1..=n` in fused order,
and neither is recomputed by a later filter.
Check: `always` - every lane contribution's position is the one consolidation
assigned; fused entries carry positions `1..=n` in order; an exact set ranks
its members at positions one to n in identifier order. `always` because a
position is part of the frozen entry.
Fault/timing angle: none.
Required faults and enabling state: Lane sets with ties; an exact set; a
filtered result.
Confidence: high - [evidence](evidence/fusion-lane-positions-are-assigned-once-from-declared-lane-order.md).
Existing check: none before this change.
Impact: A re-ranked position would let the same lane rank change an
occurrence's term between two evaluations.
Open questions: None.

### fusion-score-matches-weighted-rrf-reference

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`a_hand_computed_two_lane_example_matches_term_for_term`,
`closed_fractions_and_another_summation_order_are_detected_as_different_arithmetic`,
`non_calibration_parameters_change_the_order_the_calibration_point_gives`,
and the oracle comparison inside
`lane_order_probe_duplication_and_entry_order_change_nothing`.
Guarantee: `score(o) = sum_lane(weight_lane / (k + rank_lane(o)))` with
one-based ranks, zero for an absent lane, summed in `f64` in `Lane::ORDER`.
Check: `always` - every fused score is bit-identical to an oracle that sums
one term per lane left to right; a hand-computed two-lane example matches
term for term; over a grid of non-calibration parameters whose exact rank
varies, at least one fixture distinguishes the term sum from a closed fraction
and from another summation order, and three hand-written terms show the
declared grouping differs in bits from a regrouping; a `k`-only change between
5 and 60 reverses the order of two occurrences, and a dense-heavy weight set
reverses what equal weights give. `always` because every score must follow the
formula.
Fault/timing angle: none.
Required faults and enabling state: Non-calibration weights and `k`; fixtures
with distinct lane positions.
Confidence: high - [evidence](evidence/fusion-score-matches-weighted-rrf-reference.md).
Existing check: none before this change.
Impact: A different arithmetic shape would produce rankings that RP2.9 cannot
compare against the declared baseline.
Open questions: None.

### fusion-order-is-deterministic-and-permutation-invariant

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`lane_order_probe_duplication_and_entry_order_change_nothing`,
`empty_lanes_all_zero_weights_and_equal_scores_return_the_declared_result`,
and `ties_across_lanes_and_large_tied_sets_fall_to_identifier_order`.
Guarantee: Fused order is descending score then ascending occurrence-identifier
bytes, and it does not depend on lane completion order, hit order, or hit
duplication; empty lanes, all-zero weights, and equal scores yield the
declared result.
Check: `always` - consecutive fused entries have strictly decreasing scores or
equal bits with ascending identifiers; permuted and duplicated inputs give a
bit-identical ranking; no lanes and declared empty lanes give an empty
ranking with the undeclared lanes listed; all-zero weights give every score
`+0.0` in identifier order; a tie between an occurrence ranked only by the
exact lane and one ranked only by the dense lane, and a forty-eight-member
tied set, both fall to identifier order. `always` because reproducibility is a
per-evaluation property.
Fault/timing angle: none.
Required faults and enabling state: Shuffled lane order and doubled hits under
a fixed seed; zero weights; an exact set.
Confidence: high - [evidence](evidence/fusion-order-is-deterministic-and-permutation-invariant.md).
Existing check: `crates/retrieval/tests/lexical_retrieval.rs` order test for
one lane, status unaudited.
Impact: Results that depend on which lane finished first are not
reproducible.
Open questions: None.

### fusion-parameters-are-validated-before-scoring

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`invalid_parameters_are_refused_before_scoring`.
Guarantee: Weights are finite and nonnegative, `k` is positive and finite, and
the maximum score at rank one is finite; anything else is refused when the
parameters are constructed, before any occurrence is scored; a negative-zero
weight is admitted as `+0.0`.
Check: `always` - `FusionParameters::new` refuses NaN, infinite, and negative
weights naming the lane, refuses zero, negative, NaN, and infinite `k`, and
refuses finite weights whose rank-one sum overflows; `-0.0` is stored as
`+0.0`. `always` because the constructor is the only way to obtain parameters.
Fault/timing angle: none.
Required faults and enabling state: Each invalid parameter class; `f64::MAX`
weights with a small `k`.
Confidence: high - [evidence](evidence/fusion-parameters-are-validated-before-scoring.md).
Existing check: none before this change.
Impact: An invalid parameter would produce NaN or infinite scores whose order
is arbitrary.
Open questions: None.

### fusion-raw-scores-are-retained-and-never-compared

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`raw_scores_survive_and_filtering_keeps_positions_and_scores_without_rescoring`
and `a_hand_computed_two_lane_example_matches_term_for_term`.
Guarantee: Each fused entry keeps every lane's own position and raw score
unchanged beside the fused score; raw scores are typed by lane and never
compared across lanes.
Check: `always` - a fused entry's lane contribution equals the lane entry
consolidation produced, and `RawScore` implements no cross-variant ordering.
`always` because retention is part of every entry.
Fault/timing angle: none.
Required faults and enabling state: Lanes with distinctive raw values.
Confidence: high - [evidence](evidence/fusion-raw-scores-are-retained-and-never-compared.md).
Existing check: none before this change.
Impact: A consumer that lost raw scores could not explain or audit a fused
position.
Open questions: None.

### fusion-union-bound-is-enforced-before-materialization

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/fusion.rs`
`the_union_bound_refuses_before_materializing_the_excess`.
Guarantee: The caller-supplied fused union bound is enforced as the union is
built; the refusal names the bound and is raised before the first occurrence
beyond it is materialized.
Check: `always` - a union of six distinct occurrences under a bound of five is
refused with `UnionExceeded { bound: 5 }`, under a bound of six it fuses, and
occurrences already in the union do not consume the bound. That the refusal
precedes the excess allocation is established by inspection of `fuse`, which
checks the union size before inserting each new occurrence; no test observes
the allocation. `always` because the bound protects every allocation.
Fault/timing angle: none.
Required faults and enabling state: Overlapping lane sets whose union exceeds
the bound by one.
Confidence: high - [evidence](evidence/fusion-union-bound-is-enforced-before-materialization.md).
The check runs on each new occurrence before it is inserted.
Existing check: the lexical and dense lanes bound their own candidate counts,
status unaudited.
Impact: An unbounded union would let a wide query allocate without an approved
limit.
Open questions:

- The production bound value is an RP2.9 approval; the route supplies it with
  no default. (needs human input)

### fusion-fuses-once-before-revalidation

Type: safety
Reachability: test-only - `fuse` and `FusionParameters::new` have no caller
outside `crates/retrieval/tests/fusion.rs` (same grep as the first record).
Status: active
Exercised: partial - the filter clause by `crates/retrieval/tests/fusion.rs`
`raw_scores_survive_and_filtering_keeps_positions_and_scores_without_rescoring`;
the once-per-query clause is unexercised, since no test counts `fuse`
invocations and a route that retains its lane results can call `fuse` again
after revalidation without `Fused` observing it.
Guarantee: Fusion runs one time per query; a later revalidation filter removes
entries and leaves every survivor's position and score unchanged.
Check: `always` - `Fused` exposes no path back to lane rankings, so no
consumer can rescore it, and filtering an entry out leaves the survivors'
`(occurrence, position, score bits)` triples equal to their pre-filter values
with a gap where the removed entry was. `always` because rescoring would
change positions on every filter.
Fault/timing angle: none.
Required faults and enabling state: A fused ranking with one entry removed.
Confidence: high - [evidence](evidence/fusion-fuses-once-before-revalidation.md).
Existing check: none before this change.
Impact: Rescoring after revalidation would let eligibility filtering change
relative order and make the selection digest depend on filter timing.
Open questions: None.

## Relationship map

Grouped by shared mechanism, with suspected dominance noted where one property
holding would make another likely to hold. Dominance is a hypothesis, not proof.

- **One identifier behind the selection digest.**
  `fusion-occurrence-identity-never-collapses-payload` is upstream of
  `fusion-selection-digest-tracks-identity-tuple`: `SelectionDigest::derive`
  hashes `OccurrenceId::as_bytes` (`crates/retrieval/src/fusion/identity.rs:266-273`)
  and the digest tests build their selections from distinct synthetic
  identifiers, so an encoder that collapsed two occurrences would hand the
  digest one input where the record expects two and the digest checks would
  pass. Only the first record detects that fault on the selection path.
- **One tuple encoding behind the parent key, with overlapping detection.**
  `ParentGroupKey::derive` consumes the raw kernel tuple and an explicit
  revision, not the occurrence identifier (`identity.rs:176-190`), so
  `fusion-parent-groups-are-not-voters` shares the encoding with the first
  record rather than depending on its digest. Its test also asserts five
  distinct occurrence identifiers and a five-entry lane ranking over the same
  sources (`crates/retrieval/tests/identity.rs:472`, `:478`), so an identifier
  collapse across span, revision, or representation is caught by both records.
  Neither dominates the other.
- **Selection inside preparation.**
  `PreparationDigest::derive` hashes the selection digest as its last component
  (`identity.rs:277-299`), so `fusion-selection-digest-tracks-identity-tuple`
  covers both digests under one record: a selection reordering changes the
  preparation digest through the selection digest, and a span change reaches
  only the preparation digest. The two derivations use distinct domain strings
  (`identity.rs:267`, `:278`), so a selection digest can never be presented as
  a preparation digest. The record's open question on binding the accounting
  profile is a preparation-only concern.
- **Grouping beside, never inside, ranking.**
  `fusion-parent-groups-are-not-voters` and
  `fusion-occurrence-identity-never-collapses-payload` partition the tuple: the
  ranking unit is the full occurrence identifier, the group key is the
  whole-buffer lineage plus revision. The identity clauses are executable, and
  the clause that a group key never enters a fused score holds by the type of
  `fuse`, which accepts only `DeclaredLanes` of occurrence entries; the
  arithmetic records sit downstream of both.
- **Arithmetic downstream of identity.** The arithmetic records (one
  contribution per lane, position assignment, RRF conformance, deterministic
  order, parameter validation, raw-score retention, union bound, fuse-once)
  consume `DeclaredLanes` and `LaneEntry::position`, so every one of them
  depends on the first record here: an identifier collapse would reach `fuse`
  as one occurrence and every arithmetic check would score the wrong unit
  without noticing. Among themselves, position assignment is upstream of RRF
  conformance and deterministic order, and parameter validation is upstream of
  RRF conformance; the union bound, raw-score retention, and fuse-once records
  share no mechanism with the others beyond `Fused`.
