# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review.

## Kernel claim facts and causality

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `direct_observation_rests_on_live_exact_evidence` | `crates/kernel/tests/kernel_claim_causality.rs` | digest, malformed digest, missing, inexact evidence, revision, and subject refusals; refusals land no commit; evidence retirement refused while cited; record retirement; retired subject; snapshot-bound class | unaudited |
| `records_inherit_the_subjects_scope_and_sensitivity` | same | stored scope and registry sensitivity of a record | unaudited |
| `derived_reinjection_rests_on_exact_live_parents` | same | every parent refusal; `derived_from` dependencies; parent retirement keeps lineage; correction derives `Correct` | unaudited |
| `forged_records_and_copied_strings_grant_nothing` | same | kind, source kind, id, and object namespace refusals; look-alike observation replaces nothing; generic correction refused | unaudited |
| `replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown` | same | receipt replay; replacement; same-commit duplicate; unsupported version; subject mismatch; parents disagreeing with dependencies; malformed detail; evidence invalidation; out-of-band conflict | unaudited |
| `oversized_payloads_read_as_unknown_and_records_survive_reopen` | same | `Oversized` with row identity; `NotFound`; `FutureSnapshot`; reopen equality | unaudited |
| `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` | `crates/kernel/tests/kernel_claim_facts.rs` | field copy against the writer's report including scope and cited evidence, served visibility against `visible_as_of` on three surfaces, occurrence inventory for two classes with every field, exclusions, snapshot binding, `missing` | unaudited |
| `causality_in_facts_equals_the_causal_reader_at_every_snapshot` | same | facts and `causal_class_as_of` agree at three snapshots | unaudited |
| `supporting_approval_is_copied_with_its_validity_at_the_snapshot` | same | approval copied with validity; revocation row copied; out-of-band approval reads invalid | unaudited |
| `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` | same | `Oversized` with identity-only summary, `TooManyClaims`, `DuplicateClaim`, `NotADecision`, `FutureSnapshot`, empty request, `MalformedRequiredField` on three columns, `CorruptCanonicalRow` on a decision row disagreeing with its registry row | unaudited |
| `corrected_claims_report_succession_and_serving_standing_at_the_snapshot` | same | predecessor invalidation, succession, and `NotLiveAtSnapshot`; successor `NeverAdmitted`; served `Hidden` on every surface | unaudited |
| `a_lineage_admission_binds_every_object_on_the_lineage` | same | lineage row copied; own row unchanged; served `Hidden` on every surface after quarantine | unaudited |
| `facts_survive_reopen` | same | reopen equality | unaudited |
| `occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows` | same | a descriptor detail whose `evidence_id`, `artifact_digest`, `payload_id`, or `lineage_id` differs from the joined row is `CorruptCanonicalRow`; the read succeeds again once the payload is restored | unaudited |
| `a_registry_class_this_build_cannot_read_is_an_error_not_a_secret_default` | same | an unreadable registry class is `CorruptCanonicalRow` against a `secret` decision row and `MalformedRequiredField` against a matching unreadable one, never a `Secret` default | unaudited |

## Adjacent kernel checks the records rely on

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| descriptor identity and export bounds | `crates/kernel/tests/kernel_source_descriptors.rs`, `crates/kernel/tests/kernel_source_export.rs` | tuple encoding, predecode bounds for descriptor pages | unaudited |
| approval chains and served rows | `crates/kernel/tests/kernel_admission.rs` | supporting approval validity, fail-closed policy revision | unaudited |
| observation dependencies | `crates/kernel/tests/kernel_slice.rs` | dependency insertion and correction | unaudited |

## None found

- No production caller of `claim_facts_as_of` or `record_claim_causality`, so
  no route-level or daemon-level check exists.
- No production path calls `record_claim_causality`, so no fixture exercises a
  real producer's acquisition or derivation flow end to end.

## Suspiciously quiet areas

- The A4 acceptance (no deleted claim machinery identifiers in production) has
  no repository test.
