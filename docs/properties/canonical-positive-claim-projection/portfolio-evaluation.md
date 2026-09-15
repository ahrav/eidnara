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

## Final-use gate change

| Finding | Class | Disposition |
| --- | --- | --- |
| Candidates were submitted without their artifact digest, so the artifact egress gate never ran and a `LocalOnly` artifact passed at `Remote` | gap | fixed: the live row carries `source_artifact_digest` and submits it; the remote case is asserted |
| The serving view was read twice per judgement | refinement | fixed: `ServedClass` carries all three surfaces from the one read `egress_candidates_tx` already makes |
| No budgeted variant of the surface judgement | gap | fixed: `judge_surface_eligibility_within_budget`; `validate_for_surface` takes an `EvalBudget` |
| `expect` on the verdict zip in library code; lineage looked up by index into a batch the caller might not have passed | gap | fixed: one aligned pass with an error on a short verdict list; lineage keyed by object id |
| Eligibility errors spelled as `Facts(Kernel(_))` | refinement | fixed: `ClaimCandidateError::Eligibility` |
| `labeled: bool` paraphrased the kernel's visibility | refinement | fixed: `Permitted(SurfaceVisibility)` |
| Per-surface visibility, `WrongScope`, `Remote`, `AutoSearch`, and a proper subset revalidation were untested | gap | fixed: an `adr_accepted` seed with an `Automatic` row, a foreign project, a remote destination, all three surfaces, and a subset |
| `ValidatedCandidate` dropped the candidate's facts index | refinement | fixed: it holds the candidate |
| Cacheability of the surface batch was undocumented | refinement | fixed: `is_reusable` on both batch types |
| `SurfaceHidden` is unreachable on `ExplicitSearch` | bias | kept, documented on `judge_surface_in_tx` |
| `validate_for_surface` duplicates the shape of `eligibility::judge_occurrences`, which judges the descriptor object | bias | kept for this change: the two judge different objects on purpose; converging them belongs with the descriptor path's owner |

### Review comments on the final-use gate

Codex review of the pull request; each code fix landed behind a test that
failed first.

| Finding | Class | Disposition |
| --- | --- | --- |
| The row's `artifact_digest` was submitted as read from the projection, so a corrupt or forged row digest with no evidence rows passed the local artifact gate | gap | fixed: a permitted row is reclassified against the kernel's occurrence inventory from the same snapshot; a digest the inventory does not list for the occurrence is `Stale` |
| A descriptor retired between classification and validation left the row `Current` and the decision object `Ok`, so the withdrawn representation was permitted | gap | fixed: `judge_surface_eligibility_with_claims` returns the claim facts with the verdicts from one snapshot, and the reclassification denies a row whose occurrence is no longer listed |
| `unknown_objects` came from the classification batch while the verdicts came from the fresh snapshot | refinement | fixed: the accounting reads causality from the validation snapshot; `validate_for_surface` no longer takes the batch |

## Biases for a human

- Every record is `test-only` because no production path calls the reader or
  the writer. The reachability labels must be revisited when the projection
  and delivery tickets wire callers.
- The catalog's slugs come from the tickets, not from the unavailable bundle.
  A record here may describe a narrower or wider claim than the bundle did.
- A decision without a claim-specific `decision_kind` is accepted by the facts
  reader; whether an `adr_accepted` approval object counts as a claim is a
  policy question the kernel does not answer.
