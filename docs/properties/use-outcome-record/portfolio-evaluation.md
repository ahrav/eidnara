# Portfolio evaluation: use-outcome record

Two independent evaluations bear on this part. The first is the
specification's own fresh-context evaluation of the 20 existing-behavior
records, run when #1145 was authored at `a0e3f2993`; its findings are
copied from the specification's "Portfolio evaluation" section and
dispositioned here. The second is a fresh-context evaluation of this part
at the #1154 branch, run before this file existed by an evaluator that had
not seen the discovery reasoning. It read METHOD, every record, every
evidence file, the specification, and the ticket, and spot-checked 40
`file:line` references. This is a portfolio evaluation, not a test-adequacy
or merge verdict.

## Specification evaluation (#1145, at `a0e3f2993`)

| ID | Finding | Class | Disposition at #1154 |
| --- | --- | --- | --- |
| G1 | A lost CAS attempt leaves no durable effect outside the Memory Store | gap | Queued to #1155 (AC7) as an open question on `uo-composition-freeze-persists-at-every-hard-fold-site` |
| G2 | A pass on a route still bound after `session.delete` (OQ3-10) | gap | Resolved at #1155 ticketing as an accepted loss; recorded on `uo-p-served-set-and-identity-never-rewritten` and `uo-p-served-set-once-per-session-revision` |
| G3 | Commit-sequence monotonicity and the tip as its maximum | gap | Resolved by the `commit_log` DDL (`INTEGER PRIMARY KEY AUTOINCREMENT`, no delete); recorded on `uo-known-as-of-is-the-tip-bound-read-sequence` |
| G4 | The summarizer restart reattach as a third consent source | gap | Confirmed by a read of the reattach path; recorded on `uo-consent-gate-source-is-the-route-frozen-config` (needs human input) |
| G5 | Every written retention-class string is a compiled constant | gap | `use_outcome_capture` is a constant; `canonical` has none; recorded on `uo-expiry-keeps-pinned-and-cited-evidence` (needs human input) |
| G6 | The status surface names no identifiers | gap | Queued to #1157 (AC19); recorded on `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero` |
| R7 | Line reference correction (`admission.rs:1776`) | refinement | Applied: the record cites the binding line at this tip |
| R8 | An as-of read in the `known_as_of` evidence | refinement | Applied in `evidence/uo-known-as-of-is-the-tip-bound-read-sequence.md` |
| R9 | Convergence as a bound: one repeat with the Kernel Ready completes `ok` | refinement | Applied to `uo-p-deleted-at-iff-session-delete-kernel-step` |
| R10 | Convergence as a bound: N records drain in ceil(N / 64) calls | refinement | Applied to `uo-p-sweep-retires-only-expired-uncited-rows` and checked by `an_expiry_backlog_drains_one_page_per_call` |
| R11, R12 | Engine-specific reachability | refinement | Applied: the freeze and subagent records name both engines |
| R13 | The freeze sites' crash arm marked not yet | refinement | Applied |
| R14 | A config seam for the consent-source record | refinement | Applied: `SessionBinding.config` versus `effective_config` |
| R15 | The `source_capture` block's missing state flag | refinement | Applied as lead L4 on the status-counter record |
| B1 | Reachability per record across two engines | bias | Surfaced: a record carries one label; the `Confidence` line names the other engine |
| B2 | Zero liveness records | bias | Surfaced; still true (see E18) |
| B3 | Eight records carry a lead against the plan's wording | bias | Surfaced; the leads stay in the records |

## #1154 evaluation

| ID | Lens | Finding | Class | Disposition |
| --- | --- | --- | --- | --- |
| E1 | Coverage balance | No active record states the insert contract (`Conflict` for a duplicate identity, `InvalidInput` for a malformed spec, no row after a refused commit) | gap | Queued: the ticket fixes the record count at 34, and the contract is the Kernel half of `uo-p-served-set-once-per-session-revision` and `uo-p-served-ids-equal-frozen-composition`; both now cite `duplicate_and_malformed_serves_are_refused_without_rows` as their Kernel-half check, and #1155 activates them |
| E2 | Coverage balance | The digest agreement and the deterministic `use_outcome_id` have no #1154 record | gap | Digest half applied at #1155 through `uo-p-served-ids-equal-frozen-composition`; the deterministic-id half stays open, checked by a Kernel test and stated by no record |
| E3 | Wildcard | The served-before-recorded bound, the secret refusal, and the revision round trip have no record | gap | Applied in part: `uo-p-served-set-and-identity-never-rewritten` now states the served-before-recorded rule, which #1154 also made a DDL CHECK; the secret refusal and the revision round trip are checked by `duplicate_and_malformed_serves_are_refused_without_rows` and `served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve` and queued with E1 |
| E4 | Wildcard | G4 is answerable by one read of the reattach path | gap | Applied: the consent record cites the reattach path's live config read |
| E5 | Coverage balance | Kernel-half checks were missing from `existing-checks.md` and from proposed records | gap | Applied: each proposed record names its Kernel-half check, and the inventory lists them |
| E6 | Harness fit | Sweep scheduling (AC16) has no record and no stated bound | gap | Applied to `uo-p-sweep-retires-only-expired-uncited-rows`: one maintenance pass with the Kernel Ready runs one sweep, checked by the two lifecycle tests |
| E7 | Implementability | The freeze record's `Exercised` claimed folding-engine coverage that the cited tests do not give | refinement | Applied: the folding sites are marked uncovered |
| E8 | Implementability | The dispute test varied only the lineage revision | refinement | Applied: the test now varies source kind and source id, and covers a dispute followed by an approval |
| E9 | Implementability | The `last_transform_at` record did not say what a decreasing write does | refinement | Applied: the update writes `MAX`, and the pass ignores a write failure |
| E10 | Harness fit | Markers are names linked to tests only through this part | refinement | Accepted as the repository convention (no campaign runtime); the three #1154 markers assert their preconditions in the named tests |
| E11 | Coverage balance | Every record is `safety` with `always`-family semantics | refinement | Surfaced with E18 |
| E12 | Implementability | The slug `uo-p-served-set-once-per-session-revision` names the revision while the key is the served-set digest | refinement | Dismissed: #1155 keeps the slug by decision and amends the guarantee to the digest |
| E13 | Harness fit | 26 of 34 evidence files are below the 60-line target | refinement | Accepted: the catalog's evidence boundary states why |
| E14 | Mechanical | Three wrong references and one imprecise reference | refinement | Applied: `served_set`, the `quarantined_tip` assertion, and the decision-row trigger component are cited at their own lines, and the admission writer cites the `self.commit_seq` binding |
| E15 | Mechanical | The freeze record carries one label for sites of two classes | refinement | Surfaced as B1: the record is `default-production` by its additive site, and its `Confidence` line labels the folding sites |
| E16 | Coverage balance | No record covers the admission citing column's integrity | refinement | Applied: #1154 adds the `admission_decisions_use_outcome_immutable` and `admission_decisions_citation_no_delete` triggers, and `uo-p-served-set-and-identity-never-rewritten` covers the foreign key and the trigger with `an_admission_citation_names_a_recorded_use_outcome_and_never_changes` |
| E17 | Reachability | Labelling differs between the null-commit record and the egress record | bias | Surfaced: a record is labelled by whether its guarded state arises in production; a null `commit_seq` never does, while a `sensitive` row presented to the class rule does |
| E18 | Coverage balance | Zero liveness records (spec B2) | bias | Surfaced for the owner |
| E19 | Wildcard | G5 and leads L1, L3, L5 are parked as needs-human-input with no owner date; the existing records were reconstructed from the specification | bias | Surfaced for the owner |

## Verification

The evaluator's mechanical pass (METHOD step 7) found: 34 records, 34 index
rows, and 34 evidence files with identical slug sets; every record has the
twelve fields in order; every relative link and anchor resolves; every
evidence file has the six sections and at least one question; no
`always(!X)` record is paired with a `sometimes(X)` marker; reachability is
labelled on every record. The semantics distribution is in `catalog.md`.
The generator that writes this part re-derives every `file:line` from the
code at the branch tip, so the references above stay checked when a
reviewed fix moves a line.

## Update at #1155

#1155 activates `uo-p-served-set-once-per-session-revision`,
`uo-p-served-ids-equal-frozen-composition`, `uo-p-no-row-when-memory-disabled`,
`uo-p-no-row-for-withheld-composition`, and
`uo-p-producer-matches-request-subagent-flag`, and owns the eight writer
markers. E1 and the digest half of E2 close with those activations: the Kernel
tests check the insert contract, and the writer's tests check the key, the
digest, and the role from the daemon side. The deterministic `use_outcome_id`
half of E2 stays open: `duplicate_and_malformed_serves_are_refused_without_rows`
checks it and no record states it. G1 closes by
`a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`. The R13 crash
arm is constructed by the test-only fault point between the fold's Memory
Store commit and the Kernel write.
