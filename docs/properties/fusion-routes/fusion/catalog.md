# RP2.7 fusion identity and arithmetic properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `8e0491225a7292ef077c675d44b94f94a24041d3` (the `main` commit the U1
change was authored against). Method: `../../METHOD.md` and
`property-discovery-and-catalog`.

Source: the RP2.7 specification
([#630](https://github.com/ahrav/eidnara/issues/630)) and its local companion
bundle, whose fusion catalog proposed these slugs as unexercised
`test-only` obligations. The RP2.7.U1 ticket
([#638](https://github.com/ahrav/eidnara/issues/638)) lands the identity
records; the arithmetic records (one contribution per lane, position
assignment, RRF conformance, deterministic order, parameter validation,
raw-score retention, union bound, fuse-once) belong to RP2.7.U2 and enter this
file when that ticket lands.

This part owns identity: the occurrence ranking unit, lane consolidation, the
selection and preparation digests, and parent groups. Route budget,
authorization, and application lifecycle are separate parts.

## Observation contract

The observation point is the public API of `crates/retrieval/src/fusion/`,
exported at `crates/retrieval/src/lib.rs:23` and exercised by
`crates/retrieval/tests/identity.rs`. Kernel occurrence encoding is reused
through `kernel::source_identity`, never restated. Each record carries its own
reachability class and the evidence for it.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [fusion-occurrence-identity-never-collapses-payload](#fusion-occurrence-identity-never-collapses-payload) | safety | test-only | always | active | high |
| [fusion-selection-digest-tracks-identity-tuple](#fusion-selection-digest-tracks-identity-tuple) | safety | test-only | always | active | high |
| [fusion-parent-groups-are-not-voters](#fusion-parent-groups-are-not-voters) | safety | test-only | always | active | medium |

## Records

### fusion-occurrence-identity-never-collapses-payload

Type: safety
Reachability: test-only - `OccurrenceId::parse`, `LaneRanking::consolidate`,
and `DeclaredLanes::admit` are called from `crates/retrieval/tests/identity.rs`
only; `grep -rn 'fusion::' crates --include=*.rs` outside
`crates/retrieval/src/fusion/` finds that test file and two comments
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
`crates/retrieval/tests/identity.rs` (same grep as the first record); the
fused score that must exclude the key does not exist until RP2.7.U2.
Status: active
Exercised: partial - the identity clauses are covered by
`crates/retrieval/tests/identity.rs`
`parent_groups_share_a_parent_across_spans_and_never_replace_occurrences`;
the fusion clause (a parent key never enters the fused score) waits for
RP2.7.U2.
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
Confidence: medium - [evidence](evidence/fusion-parent-groups-are-not-voters.md).
The identity clauses are exercised; the fusion clause is not yet executable.
Existing check: none before this change.
Impact: A parent key used as a voter would let one source outvote another by
span count, and a group spanning revisions would mix bytes from two versions.
Open questions:

- RP2.8 decides whether non-span classes group at all; this record only fixes
  the key. (needs human input)

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
  whole-buffer lineage plus revision. The identity clauses are executable now;
  the clause that a group key never enters a fused score waits for the RP2.7.U2
  arithmetic records, which will sit downstream of both. Until then the record
  stays `partial` and its Confidence `medium`.
- **Deferred dependencies.** The arithmetic records (one contribution per lane,
  position assignment, RRF conformance, deterministic order, parameter
  validation, raw-score retention, union bound, fuse-once) consume
  `DeclaredLanes` and `LaneEntry::position`, so every one of them will depend on
  the first record here. Their entries join this map when RP2.7.U2 lands.
