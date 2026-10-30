# Attestation verifier: portfolio evaluation

## Source of this evaluation

The specification ([#1169](https://github.com/ahrav/eidnara/issues/1169),
Portfolio evaluation) records a fresh-context evaluation over its 20
existing-behavior records through four lenses: harness fit, coverage balance,
implementability, and wildcard. It re-verified every cited `path:line` and 60
cited test names at `3a3b916b7`, invalidated no record, and returned 6 gaps,
10 refinements, 3 biases, and 4 reference corrections. The evaluation's
working files are not durable (specification N4), so this file restates its
findings and records their disposition as applied by the U1 ticket
([#1170](https://github.com/ahrav/eidnara/issues/1170)). The evaluation did
not cover the 18 proposed obligations; they enter as designed records with
falsifiers. No new fresh-context evaluation ran for this part.

## Refinements

The specification lists the refinements inline in one sentence of eight
items; the relabel item covers three records, which brings the count to ten.
The numbering below follows the sentence's order.

| Id | Refinement | Disposition |
| --- | --- | --- |
| R1 | State both document caps (64 KiB publication, 256 KiB read) | applied to `p6-document-descriptor-identity-digest-and-revision` |
| R2 | Pin that after `MarkStale` a validated `CodeObserved` returns `Outcome::Promote` while effective maturity, disposition, and visibility are unchanged, so `Outcome` alone is never the oracle | applied to `p6-mark-stale-withdraws-support-and-is-terminal-in-place`; the disposition half is verified from `event_effect`, and the maturity and visibility halves stay an open question on that record |
| R3 | Scope the `Other`-degradation guarantee to a well-formed trigger id | applied to `p6-degraded-witness-event-denies-without-error` |
| R4 | State the trusted-class precondition in the trigger record | applied to `p6-trigger-must-match-subject-source-triple` |
| R5 | Relabel three records that mix production and test-only clauses | applied by labelling every record from its own evidence; the specification body does not name the three records, so this part cannot say which three the evaluation meant. The records whose notes separate the clauses are `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements`, `p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search`, and `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard` |
| R6 | Add the missing `Exercised` notes | applied: every record carries one |
| R7 | Reword `sometimes` markers as fixture coverage floors | applied to `p6-validated-witness-under-trusted-classes-promotes` |
| R8 | Resolve two open questions with G3 and G4 | applied: the opener question resolves on `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements` (the U1 driver uses the daemon opener); the plugin retire question is recorded on `p6-new-party-distinct-and-self-refusal` and in `p6-plugin-cannot-replace-a-foreign-source-decision`'s impact as G4 |

## Reference corrections

| Id | Correction | Disposition |
| --- | --- | --- |
| C1 | `claim_causality.rs:36` is `:38` | applied: `MAX_DERIVATION_PARENTS` is `crates/kernel/src/claim_causality.rs:38` |
| C2 | Two `checks.rs` arm ranges shifted by two lines | applied: the `FileExists` arm is `crates/kernel/src/applicability/checks.rs:285` and the `ConfigKey` arm `:296` at the branch tip; the specification's original ranges are not recoverable |
| C3 | `kernel_routes.rs:1888, 5131` call a sibling fixture | applied: the records cite the tests (`:1875`, `:5120`) and their `code_observation` fixture (`:887`) |
| C4 | A preamble wording | applied: the catalog's reachability section states no blanket claim and each record carries its own evidence |

Line drift between `3a3b916b7` and the branch tip moved several references:
`crates/kernel/src/admission.rs` unit tests by 197 lines,
`crates/daemon/src/source_capture_tests.rs` by 28 lines,
`crates/kernel/tests/kernel_slice.rs` by one line, the `observe_code` fixture
from `lib.rs:14043` at `3a3b916b7` to
`crates/daemon/src/lib.rs:14406`, and the test-support guard
the specification placed at `lib.rs:13690` (K9) to `:14041`. Each record cites
the current line.

## Gaps

| Id | Gap | Disposition |
| --- | --- | --- |
| G1 | Document evidence is at least `Sensitive` and anything citing it by FK serves no wider than `ExplicitSearch` | added as `p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search` |
| G2 | `delete_artifact` bypasses the citation guard | added as `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`; relabelled `test-only` because `delete_artifact` has no production caller |
| G3 | The Kernel checkout open has no allocation limit and honors replacement refs | added as `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements` |
| G4 | A plugin `RetireDecision` can retire an attestation | re-queued; owner #1174: the #1172 ticket resolves OQ6-15 as no new mechanism and documents the plugin `mark_stale` and retire exposure in U4 |
| G5 | No record on the served form of an unpromoted trusted-class attestation (`ExplicitLabeled`) | closed by #1172 with no record: no verifier write commits an unpromoted attestation, because the envelope rolls back below `Verified` (`p6-new-witness-degradation-never-silent`), and the plugin route cannot write under `verifier` (`p6-new-party-distinct-and-self-refusal`), so the state has no producer |
| G6 | No record pinning that `artifact_reference` is a content-store path (D2) | queued; owner #1174 (U4 part pass). #1171 does not address it: the classifier reads no `artifact_reference`, and its claim comes from the descriptor detail and the evidence row |
| G7 | The two records #1170 activates, `p6-new-verdict-deterministic` and `p6-new-blob-read-bounded-at-oid`, have no fresh-context four-lens evaluation | queued; owner U1 ([#1170](https://github.com/ahrav/eidnara/issues/1170)): evaluate harness fit, coverage balance, implementability, and wildcard, and record each finding's disposition |
| G8 | The two records #1171 activates, `p6-new-verifiability-pure-function-of-snapshot` and `p6-new-verifier-input-anchor-and-document-citation`, have no fresh-context four-lens evaluation | queued; owner U2 ([#1171](https://github.com/ahrav/eidnara/issues/1171)): evaluate harness fit, coverage balance, implementability, and wildcard, and record each finding's disposition |

## Biases for a human

- Surface selection followed the plan's touch list, while G1 to G3 sit in
  files the plan does not name.
- Uniform `high` confidence carries no information. This part lowers
  `p6-mark-stale-withdraws-support-and-is-terminal-in-place` to `medium` and
  every proposed record to `low`.
- Cross-catalog P2 citations (`p2-git-open-is-isolated-and-allocation-bounded`,
  `p2-decision-parents-must-be-live-at-insert`,
  `p2-citing-decision-carries-at-least-its-evidence-class`) were accepted in
  prose where METHOD treats them as claims under test. They stay records of
  `../cited-capture-anchors/`; this part cites them without re-authoring.

## Findings from the U1 implementation

- AC3's length shortcut conflicts with AC1's normalized clause: a CRLF-form
  claim against an LF blob has a length different from the header size and
  must `Match`. The driver decides `Mismatch` with zero bytes read only when
  no line-ending form can reach the claimed length, and reads the blob
  otherwise (`p6-new-verdict-deterministic`). This is recorded for the owner
  as a deviation from AC3's literal wording.
- `over_budget` from the allocation limit depends on the pack layout of the
  commit, a tree on the path, or the blob, so a repack can change it, while
  the ticket classes `over_budget` as terminal. The question was recorded for the #1173 owner,
  who kept it terminal per `(decision, oid)` for the process lifetime
  (`p6-new-verdict-deterministic`).
- The verifier maps an allocation refusal on any object, including a commit
  or tree, to `over_budget`. No test constructs a commit or tree above
  256 KiB.
- gix follows a delta chain inside one pack without a hop bound and bounds
  only cross-pack recursion. The driver decodes each object held in the
  repository's own packs itself, after `git_sources::OwnPacks` walks the
  entry's ofs- and ref-delta bases with a hop bound of the index's entry
  count. An id whose every holding own pack revisits an entry, names a base
  outside that pack, or does not decode reads a valid loose copy or is
  `object_unreadable(class)`; the object store never reads an id an own index
  holds. An object in an alternate object directory, or in an
  own pack whose index or pack is not a regular file, reads through the object
  store, outside that walk.
- A raw digest match counts only when the header size equals the claimed
  length, so a claim with the right digest and a wrong length is `Mismatch`.

## Findings from the U2 write-set implementation

- The Kernel derives the causality record's ids from the attestation's
  object id and the commit sequence, so the causality id is a function of
  `(model object id, oid)` and the commit, not of the pair alone.
- The ticket keys the two verdicts' writes separately, `attest:` and
  `stale:`. The write set refuses a recorded key for the other verdict on
  the same decision and oid as `Conflict` inside the envelope, so "the same
  key with the opposite verdict" reads as the same `(decision, oid)` pair.
- The stale reason names the cited sha256 and the git blob id: a mismatch the
  header size decides reads no content, so the blob id is the observed value
  every mismatch carries. `Evaluation` gains `blob_oid` for this and for the
  witness detail.
- The witness admission's degradation is constructed by a SQLite trigger on
  the test store that rewrites the witness's kind at insert; no production
  seam exists for it.
