# Portfolio evaluation

The records in this part were reconstructed from the specification and the
code, so no lens passes produced them and no fresh-context evaluation of a
discovery portfolio ran. In its place, each of the two changes that introduced
the code was reviewed before commit by an independent reviewer that had not
seen the design reasoning, through five lenses: architecture, complexity,
testing, over-engineering, and language design. This file records what those
reviews found and how each finding was dispositioned.

## Causality change

| Finding | Class | Disposition |
| --- | --- | --- |
| Writer and reader selected "the live record" with different predicates | gap | fixed: one `record_rows_sql` statement serves both |
| `source_kind = claim_causality` was not reserved, so a squatter row could be superseded or wedge a subject | gap | fixed: `uses_causality_namespace` covers source kind; test asserts refusal |
| The class was derived from the detail alone, never cross-checked with the guarded `evidence_id` column or the `derived_from` rows | gap | fixed: `causal_class_at` requires agreement, else `Unknown(Malformed)` |
| The caller-supplied operation carried no information | refinement | fixed: derived from succession |
| `ContentRefused` was unreachable | refinement | fixed: removed |
| `check_acquisition` duplicated the descriptor evidence check | refinement | fixed: `cas::exact_evidence` shared |
| `EvidenceUnavailable`, `SubjectMismatch`, `EvidenceNotExact`, retired subject, scope and sensitivity inheritance were untested | gap | fixed: cases added |
| Weak `matches!` oracles on class values | refinement | fixed: whole-value comparison |
| Retirement stays open through the generic writer | bias | kept, documented in the module: `Unknown` grants nothing |
| Artifact reclamation does not hold artifacts for causality records | bias | kept, documented: replay needs no bytes |

## Facts change

| Finding | Class | Disposition |
| --- | --- | --- |
| Own and lineage row selection restated the serving view's SQL | gap | fixed: `served_own_decision_sql` and `served_lineage_decision_sql` wrap the digest-guarded definitions |
| Descriptor liveness ignored registry timestamps and evidence liveness | gap | fixed: the inventory applies the export's rule |
| A duplicated id in one request produced two different answers | gap | fixed: `DuplicateClaim` |
| `Sensitivity::from_stored` turned a malformed class into `Secret` | gap | fixed: fallible decode, `MalformedRequiredField` |
| Decision row not cross-checked with its registry row | gap | fixed: `CorruptCanonicalRow` on disagreement |
| `served: None` conflated "retired" with "never admitted" | refinement | fixed: `ServedStanding` |
| `RevisionDomains` and duplicated decision fields restated other fields | refinement | fixed: removed; the domains are named in the module doc |
| Lineage admission, served absence, all three surfaces, `promoted_memory`, optional decision fields, and occurrence fields were untested | gap | fixed: cases added |
| `NotAClaim` overstated the check | refinement | fixed: `NotADecision` |
| The catalog said no summary is returned for an oversized payload while the code returns an identity-only summary | gap | fixed in the record; the code was kept |

A post-commit review of the facts change through the security, business-logic,
duplication, and dead-code lenses added three findings:

| Finding | Class | Disposition |
| --- | --- | --- |
| The occurrence inventory reported `evidence_id`, `artifact_digest`, `payload_id`, and `lineage_id` from the JSON detail while liveness was judged on the joined evidence row; `live_source_descriptors` and `source_export::preflight` refuse that disagreement | gap | fixed: `load_occurrences` compares the detail with the joined columns and the registry with the observation timestamps, `CorruptCanonicalRow` on disagreement; test rewrites each field out of band |
| The descriptor liveness predicate was restated by hand rather than taken from `Descriptors::LiveAtEnd`, the export's single definition | refinement | fixed: `load_descriptor` formats `Descriptors::LiveAtEnd.predicate` into its statement |
| `load_decision` compared the decision class with the registry class after `ObjectRow` had decoded an unreadable registry value as `Secret`, so a `secret` decision row masked the unreadable registry row | gap | fixed: the stored class texts are compared, then decoded strictly; test inserts unreadable registry rows |

## Projection change

| Finding | Class | Disposition |
| --- | --- | --- |
| `classify` matched two of seven dispositions and let `Disputed`, `Rejected`, and `Contradicted` fall through to the served rule | gap | fixed: exhaustive match; `Rejected`, `Contradicted`, `Quarantined` are `Hidden`; `Disputed` follows the served row |
| Revision skew was documented as the primary `Stale` input though the registry never changes a revision | refinement | fixed: documented as a corruption guard; `Disposition::Stale` is the real input |
| Candidates dropped the served surfaces and cloned admission and causality per row | refinement | fixed: candidates index the batch's `ClaimFacts` |
| `family='id'` literal and no `extraction_version` check on the association join | gap | fixed: `Family::Id.keyword()` bound; mismatch refused |
| The daemon oracle restated the classifier's predicates | gap | fixed: literal expected states per phase from the ticket text |
| No `Stale`, `Disposition::Superseded`, bound, or corrupt-row coverage | gap | fixed: `MarkStale` claim in the daemon test; refusal tests over the baseline schema |
| `TooManyObjects` duplicated `TooManyClaims`; `Kernel` and `Facts(Kernel)` spelled one failure two ways | refinement | fixed: both removed |
| `created_commit_seq` selected and never read | refinement | fixed: removed |
| `max_rows` is a result bound, not a work bound: the live-claim query sorts before it limits | bias | kept, documented on the bound; the sibling `live_candidates` shares the plan |
| Near-duplicate of `eligibility::live_candidates` | bias | kept: the claim read needs the association join and both claim classes; widening the sibling's class filter is a follow-up for its own owner |

## Post-review pass

Findings from the six-track review of the projection change and how each was
dispositioned. Every code fix landed behind a test that failed first.

| Finding | Class | Disposition |
| --- | --- | --- |
| `classify` read `superseded_by` only under `invalidated_commit_seq`, while the kernel's `judge` and `token_check` read `superseded_by` first; the registry trigger admits a successor without an invalidation, so that shape classified `Current` here and `Superseded` in the kernel | gap | fixed: `superseded_by` is checked first; `successor recorded without invalidation` pins it |
| The enum doc ranked `Stale` above `Hidden` while the code hid a rejected, contradicted, or quarantined admission before the revision guard; the kernel's `visibility_row` serves `Stale` labeled and those three on no surface, so `Hidden` is the more restrictive state and must win | gap | fixed: precedence is `Retracted`, `Superseded`, `Hidden`, `Stale`, `Current`, with variant order as the source of truth; every Hidden-over-Stale conflict pair is pinned |
| `claims` was documented as first-seen order but a `BTreeSet` sorted the ids | refinement | fixed: dedup preserves first-seen order; the daemon test asserts a nonlexical witness |
| `kernel.tip()` ran before `max_claims` was enforced, so the bound test named a guarantee the code lacked | refinement | fixed: the bound is checked before any kernel read; the test proves it with a failing tip |
| The catalog credited the daemon test with showing a causality record leaves a state unchanged, but the record was committed before the successor had any projection rows | gap | fixed: the test classifies after catch-up, records causality, classifies again, and asserts equality; the record wording follows the test |

Codex review of the pushed branch added three findings:

| Finding | Class | Disposition |
| --- | --- | --- |
| The live-claim query trusted the association's `target_id` alone, so an association whose key and occurrence tuple named claim A while its target named claim B classified A from B's facts; `exact::lookup::AssociationRow::decode` checks the same agreement | gap | fixed: the query also selects the key and the tuple, and refuses `CorruptRow` unless key, target, and the tuple's identity field name one object; `an_association_whose_target_disagrees_with_its_key_or_row_is_refused` pins both disagreements |
| `classify_live_claims` read the kernel's tip and facts without comparing the projection identity's `kernel_incarnation_id` to the kernel it was given, unlike `coverage::observe` and the exact lookup, so a reused object id in another incarnation could classify from an unrelated history | gap | fixed: the caller names the kernel's incarnation as the sibling readers do; `NoIdentity` and `ForeignKernel` refuse before any row is read |
| `ClaimCandidateBatch::claim` indexed `claims` unchecked, so a candidate from another batch read another object's facts or panicked from a method returning `Option` | gap | fixed: checked index and the facts must name the candidate's object; `a_candidate_from_another_batch_reads_no_facts` |
| `classify` read only `own_admission`, while the serving view folds the lineage row into every surface; a lineage-scoped `MarkStale` served labeled and classified `Current` here | gap | fixed: own and lineage dispositions are read with one precedence; `a_lineage_disposition_binds_like_the_own_row` |
| `classify` never consulted `facts.occurrences`, so a row whose descriptor lost its evidence (an artifact deletion under an active decision) classified `Current` until catch-up tombstoned it | gap | fixed: a row absent from the live descriptor inventory is `Retracted`; `a_row_outside_the_live_descriptor_inventory_is_retracted`; the daemon test's `Current` rows prove the projection ids match the inventory |
| A second `canonical_object` association on one occurrence would have doubled its candidate rows | gap | covered by the key and tuple check above: the second row's key cannot match the tuple; `a_second_canonical_object_association_on_one_row_is_refused` pins it |
| `spec-integration.md` attributed the snapshot read to `classify` | refinement | fixed in the record: `classify_live_claims` is the snapshot-bound read, `classify` the mapping over loaded facts |

## Biases for a human

- Every record is `test-only` because no production path calls the reader or
  the writer. The reachability labels must be revisited when the projection
  and delivery tickets wire callers.
- The catalog's slugs come from the tickets, not from the unavailable bundle.
  A record here may describe a narrower or wider claim than the bundle did.
- A decision without a claim-specific `decision_kind` is accepted by the facts
  reader; whether an `adr_accepted` approval object counts as a claim is a
  policy question the kernel does not answer.
