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

## Projection classification

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `state_follows_the_documented_precedence` | `crates/retrieval/tests/claims.rs` | the full state table with precedence, written by hand | unaudited |
| `causal_class_changes_no_state` | same | Unknown neutrality across every state-relevant fact combination | unaudited |
| `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` | `crates/daemon/tests/claim_sources.rs` | lagging rows classify Superseded, Retracted, Hidden from canonical facts against an independent oracle; catch-up tombstones; quarantine stays live and Hidden; a causality record changes no state; a fresh rebuild classifies identically | unaudited |

## Final-use validation

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `final_use_is_judged_per_surface_from_current_canonical_policy` | `crates/daemon/tests/claim_sources.rs` | explicit search permitted with label while AutoInject is `SurfaceHidden` on the same batch `Ok`; a foreign project digest denies every candidate `WrongScope`; genuine and Unknown permitted alike; separate overlapping rejected and Unknown sets; ordered duplicates; two reads agree; quarantine and correction deny at classification and at revalidation of earlier survivors | unaudited |
| `a_purged_representation_splits_row_verdicts_and_both_accounting_sets_keep_the_object` | `crates/daemon/tests/claim_sources.rs` | a purge of one representation's artifact between selection and handoff denies that row and leaves the others permitted; the object is in both the permitted and the rejected set | unaudited |

## Adjacent kernel checks the records rely on

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| descriptor identity and export bounds | `crates/kernel/tests/kernel_source_descriptors.rs`, `crates/kernel/tests/kernel_source_export.rs` | tuple encoding, predecode bounds for descriptor pages | unaudited |
| approval chains and served rows | `crates/kernel/tests/kernel_admission.rs` | supporting approval validity, fail-closed policy revision | unaudited |
| observation dependencies | `crates/kernel/tests/kernel_slice.rs` | dependency insertion and correction | unaudited |

## None found

- No production caller of `claim_facts_as_of`, `record_claim_causality`, or
  `classify_live_claims`, so no route-level check exists.
- No daemon route calls `validate_for_surface`; no harness delivers a claim, so
  the six independent delivery witnesses the delivery ticket requires do not
  exist; no claim path invokes the checkout applicability engine.
- No claim-specific process-crash cut, recovery bound, or disable path; the
  RP2.1 shared checks named in the records cover the mechanisms the claim
  rows travel through.
- No production path calls `record_claim_causality`, so no fixture exercises a
  real producer's acquisition or derivation flow end to end.

## Suspiciously quiet areas

- The A4 acceptance (no deleted claim machinery identifiers in production) has
  no repository test.
