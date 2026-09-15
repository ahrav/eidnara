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

## Biases for a human

- Every record is `test-only` because no production path calls the reader or
  the writer. The reachability labels must be revisited when the projection
  and delivery tickets wire callers.
- The catalog's slugs come from the tickets, not from the unavailable bundle.
  A record here may describe a narrower or wider claim than the bundle did.
- A decision without a claim-specific `decision_kind` is accepted by the facts
  reader; whether an `adr_accepted` approval object counts as a claim is a
  policy question the kernel does not answer.
