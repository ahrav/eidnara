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
| `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` | same | `Oversized` with identity-only summary, `TooManyClaims`, `DuplicateClaim`, `NotADecision`, `FutureSnapshot`, empty request, `MalformedRequiredField` on three columns, `CorruptCanonicalRow` on a decision row whose class, `invalidated_commit_seq`, or `superseded_by` disagrees with its registry row (one `lifecycle_agrees_sql` predicate shared with descriptors), `InvalidInput` on an empty or over-long id | unaudited |
| `facts_at_a_target_refuse_another_incarnation` | same | `claim_facts_at` equals `claim_facts_as_of` at the target's tip and refuses a target captured from another incarnation | unaudited |
| `corrected_claims_report_succession_and_serving_standing_at_the_snapshot` | same | predecessor invalidation, succession, and `NotLiveAtSnapshot`; successor `NeverAdmitted`; served `Hidden` on every surface | unaudited |
| `a_lineage_admission_binds_every_object_on_the_lineage` | same | lineage row copied; own row unchanged; served `Hidden` on every surface after quarantine | unaudited |
| `facts_survive_reopen` | same | reopen equality | unaudited |
| `occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows` | same | a descriptor detail whose `evidence_id`, `artifact_digest`, `payload_id`, or `lineage_id` differs from the joined row, or an observation `sensitivity_class`, `superseded_by`, or `observation_id`, or a registry `source_revision` or `source_kind`, that disagrees with the row it was admitted with, is `CorruptCanonicalRow`; the read succeeds again once the row is restored | unaudited |
| `a_partial_span_publication_is_outside_the_whole_buffer_inventory` | same | a proper sub-span publication reads `NoDescriptor`; the whole-buffer publication of the same representation is the listed occurrence | unaudited |
| `served_facts_follow_the_cited_evidence_class_read_today` | same | re-ingesting the cited artifact as secret after S changes `served` on a reread of S and no revisioned field | unaudited |
| `a_registry_class_this_build_cannot_read_is_an_error_not_a_secret_default` | same | an unreadable registry class is `CorruptCanonicalRow` against a `secret` decision row and `MalformedRequiredField` against a matching unreadable one, never a `Secret` default | unaudited |

## Projection classification

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `state_follows_the_documented_precedence` | `crates/retrieval/tests/claims.rs` | the full state table with precedence, written by hand, including a successor recorded without invalidation and every Hidden-over-Stale conflict pair | unaudited |
| `causal_class_changes_no_state` | same | Unknown neutrality across every state-relevant fact combination | unaudited |
| `a_lineage_disposition_binds_like_the_own_row` | same | every lineage disposition over an active own row, and a lineage row more restrictive than the own row | unaudited |
| `a_row_outside_the_live_descriptor_inventory_is_retracted` | same | `Retracted` when no live descriptor matches the row's id, class, and representation; a successor still wins | unaudited |
| `live_rows::a_row_whose_columns_disagree_with_its_tuple_is_refused` | same | `CorruptRow` when the revision column or the occurrence id disagrees with the stored tuple | unaudited |
| `live_rows::an_association_created_in_another_commit_than_its_row_is_refused` | same | `CorruptRow` when the association's `created_commit_seq` differs from its occurrence's | unaudited |
| `live_rows::a_second_canonical_object_association_on_one_row_is_refused` | same | `CorruptRow` for a second `canonical_object` association on one occurrence | unaudited |
| `a_candidate_from_another_batch_reads_no_facts` | same | `ClaimCandidateBatch::claim` returns `None` for a candidate whose index is out of range or names another object | unaudited |
| `live_rows::an_association_whose_target_disagrees_with_its_key_or_row_is_refused` | same | `CorruptRow` when the association target, its key, or the occurrence tuple name different objects | unaudited |
| `live_rows::a_projection_from_another_kernel_incarnation_is_refused_before_any_read` | same | `NoIdentity`, then `ForeignKernel` against the kernel's own database identity, before the row bound is checked | unaudited |
| `live_rows::a_kernel_tip_behind_the_projection_checkpoint_is_refused` | same | `NoCheckpoint` without a checkpoint, `KernelBehindProjection` when the captured tip is behind it, and a full read once they agree | unaudited |
| `live_rows::an_exhausted_budget_is_refused_before_the_projection_is_read` | same | a cancelled `EvalBudget` refuses `Kernel(Deadline)` before the row bound | unaudited |
| `live_rows::rows_are_bounded_ordered_and_keyed_to_the_decision_object` | same | `(class, occurrence_id)` order, the decision object key per row, and `TooManyRecords` past the row bound | unaudited |
| `live_rows::a_claim_row_without_its_association_or_with_another_extractor_is_refused` | same | `ExtractionVersionMismatch` for another extractor and `CorruptRow` for a claim row with no `canonical_object` association | unaudited |
| `live_rows::distinct_objects_past_the_facts_bound_are_refused_before_the_kernel_is_read` | same | `TooManyClaims` before the facts read, and the kernel error once the bound admits every distinct object | unaudited |
| `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` | `crates/daemon/tests/claim_sources.rs` | lagging rows classify Superseded, Retracted, Hidden from canonical facts against an independent oracle; catch-up tombstones; quarantine stays live and Hidden; the classified map is equal before and after a causality record on the successor; a fresh rebuild classifies identically | unaudited |

## Adjacent kernel checks the records rely on

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| descriptor identity and export bounds | `crates/kernel/tests/kernel_source_descriptors.rs`, `crates/kernel/tests/kernel_source_export.rs` | tuple encoding, predecode bounds for descriptor pages | unaudited |
| approval chains and served rows | `crates/kernel/tests/kernel_admission.rs` | supporting approval validity, fail-closed policy revision | unaudited |
| observation dependencies | `crates/kernel/tests/kernel_slice.rs` | dependency insertion and correction | unaudited |
| `Retired memory-plane identifiers` step of the `gates` job | `.github/workflows/ci.yml` | rejects retired identifiers (`claim_mirror`, `claim_intent`, `claim_operation`, `claim_lane`, `claim_snapshot`, `claim_compat`, `public_claim_id`, `snapshot_vector`, `revision_locator`, `compat_read`, `dual_lane`, `mirror_row`, `mcm_`, and their prose spellings) in production content, tracked pathnames, and whole index blobs; excludes crate `tests/`, `*.test.ts`, `docs/`, `NOTICE`, and `*.md` | unaudited |

## None found

- No production caller of `claim_facts_as_of`, `record_claim_causality`, or
  `classify_live_claims`, so no route-level check exists.
- No claim-specific process-crash cut, recovery bound, or disable path; the
  RP2.1 shared checks named in the records cover the mechanisms the claim
  rows travel through.
- No production path calls `record_claim_causality`, so no fixture exercises a
  real producer's acquisition or derivation flow end to end.

## Suspiciously quiet areas

- The A4 acceptance (no deleted claim machinery identifiers in production) is
  gated by the `Retired memory-plane identifiers` CI step above, not by a
  repository test. Its token list is fixed in the workflow; a retired name
  outside that list, or one introduced under a new spelling, is not caught.
