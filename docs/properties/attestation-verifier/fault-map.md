# Attestation verifier: fault map

Fault classes come from the specification's fault map
([#1169](https://github.com/ahrav/eidnara/issues/1169), Fault and state
coverage). Availability names what can construct the fault at the #1170
branch, updated at the #1171 branch for F1, F3, F14, and F15, and at the
#1172 branch for F1, F2, F3, F10, F11, F14, and F15, and at the #1174
branch for F2 and F10.

## Fault classes

| Id | Fault | Availability |
| --- | --- | --- |
| F1 | Trigger absent, foreign, wrong kind, invalidated, at another revision, or with retired backing evidence | constructible: Kernel admission fixtures; for the classifier, a cited evidence row invalidated by `delete_artifact` classifies `evidence_retired`; for the write set, a SQLite trigger on the test store rewrites the witness's kind at insert, and another attestation's witness is named as a trigger |
| F2 | Model decision superseded, retired, or re-disposed between selection and commit | constructible: Kernel slice fixtures; the write-set fixture supersedes, retires, quarantines, and marks stale a model decision after its classification; the pass fixture retires an attestation from the commit poll between the applicability read and the stale mark (#1174) |
| F3 | Evidence or anchor invalidated in that window; `delete_artifact` bypassing the citation guard | constructible: `retire_evidence` and `delete_artifact` in Kernel tests; at the classification step, the classifier fixture store invalidates cited evidence (`delete_artifact`), a cited descriptor (`retire_observation`), and an anchor (`anchors.invalidated_commit_seq` set by SQL) after the requested sequence, which leaves the classification at that sequence unchanged and changes it at the tip, and the AC16 table invalidates evidence and a descriptor before it; the Kernel anchor-read test finds an anchor invalidated by SQL live before that sequence and absent from it on; the write-set fixture invalidates an anchor, retires a descriptor, and deletes the compared evidence row (`delete_artifact`) after classification, each `changed` |
| F4 | Header over budget; header under budget with an over-budget delta base; hash mismatch on decode | constructible: large blobs, `write_delta_pack` in `crates/daemon/tests/support/git_repo.rs`, and a valid object stored under another id |
| F5 | Partial clone or missing object; corrupt commit, tree, or blob | constructible: a deleted loose object stands in for a missing promisor object; a valid object stored under another id, an undecodable object written with `git hash-object --literally`, a loose object whose bytes are not zlib data, a commit or tree with a valid prefix and a malformed remainder, or `write_self_delta_pack`'s self-referencing ref-delta, for corruption |
| F6 | Path absent; gitlink, symlink, or tree at the path; path beneath a gitlink | constructible: `git mktree --missing` in a bare fixture |
| F7 | Deadline before the pass, between decisions, and during a read; budget cancellation | constructible for one claim: a cancelled or expired `EvalBudget`, and a stage-aware `Deadline` double; for the pass, a recording `PassClock` double that passes before decision k + 1 or before a blob read (#1173), or at the applicability page poll or stale-mark commit poll (#1174) |
| F8 | Worktree edit, HEAD move, repack between two verdicts; the repack clause excludes allocation-limit refusals of the commit, a tree, or the blob, whose verdict depends on pack layout | constructible: a linked worktree of the bare fixture, `git update-ref`, `git repack -a -d` |
| F9 | `text eol`, `ident`, encoding, or clean filter attributes at the oid | partial: the LF and CRLF forms are constructible; `ident`, encoding, and clean filters stay false mismatches by the OQ6-8 decision |
| F10 | Operation key replayed with the same and with a different verdict | constructible: the write-set fixture repeats a call, changes the blob id under the same key, and calls the opposite verdict for the same decision and oid; the applicability fixture repeats a stale mark under its key (#1174) |
| F11 | Fault-injected rollback after the admission; crash after commit then fresh open | partial: a SQLite trigger on the test store aborts the citation event, the last write; the crash half is a fresh open after a clean close, which shows durability across an open and not across process termination, since a live store holds its data home's writer lock and a second open of an abandoned store fails |
| F12 | Flag absent, reload true to false, project-tier flag | constructible: a switchable flag reader drives the pass and the task; the shared parity fixture drives the Rust tier merge and both plugin loaders (#1173) |
| F13 | More than N candidates; a starving two-project pattern; a permanently transient head decision | constructible: bare repositories as project roots with N + 2 verifiable documents, 3N absent paths in one project, a head decision citing an unbound repository, and a SQLite trigger that degrades one decision's witness every pass (#1173) |
| F14 | A `Sensitive` evidence row cited through the row FK, the observation FK, or the admission event | constructible: Kernel slice and admission fixtures; the precondition at classification is constructed by the classifier fixture store, whose document evidence is `Sensitive`, cited by the row FK and by events, and named in the input by id only; the write-set fixture commits attestations for a model decision citing the evidence by an event and by its row |
| F15 | Default prose documents with no code-or-config kind | constructible: the OQ6-9 path rule classifies `ARCHITECTURE.md` and `STRUCTURE.md` `CurrentCode` (#1171); the write set attests `ARCHITECTURE.md` under `CurrentCode` and `config.toml` under `CurrentConfig` (#1172) |

## Per-property required faults

| Record | Faults |
| --- | --- |
| `p6-degraded-witness-event-denies-without-error` | F1 |
| `p6-witness-cannot-lift-an-inference-class-subject` | a validated trigger on an inference subject |
| `p6-trigger-must-match-subject-source-triple` | F1 |
| `p6-trigger-sensitivity-composes-into-admission` | F14 |
| `p6-validated-witness-under-trusted-classes-promotes` | none; a positive witness |
| `p6-object-subject-provenance-passes-trusted-classes` | none |
| `p6-policy-digest-pins-admission-policy` | an edit inside a digest region |
| `p6-successor-keeps-identity-and-advances-revision` | a cross-source or unbumped replacement |
| `p6-replaced-decision-is-invalidated-and-unserved` | none |
| `p6-barred-lineage-refuses-supersession` | a barred predecessor or survivor |
| `p6-mark-stale-withdraws-support-and-is-terminal-in-place` | a witness after `MarkStale` |
| `p6-object-applicability-payload-fails-closed` | malformed payloads |
| `p6-failed-cheap-check-is-stale-and-handed-to-repair` | a failed cheap check |
| `p6-blob-read-is-header-gated-before-decode` | F4 |
| `p6-document-descriptor-identity-digest-and-revision` | a document between 64 KiB and 256 KiB |
| `p6-document-republishes-only-on-digest-change-and-retires-on-missing` | edits, removals, renames, unreadable reads |
| `p6-plugin-source-kind-table-is-closed-and-classes-never-raised` | a plugin write under `verifier` |
| `p6-plugin-cannot-replace-a-foreign-source-decision` | a plugin replacement of a foreign decision |
| `p6-derived-reinjection-parents-live-at-write-and-existed-at-read` | F2 |
| `p6-direct-observation-requires-exact-evidence-digest` | F3 |
| `p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search` | F14 |
| `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard` | F3 |
| `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements` | F4; a `refs/replace` entry |
| `p6-new-verifiability-pure-function-of-snapshot` | F8 at the classification step; a later Kernel commit |
| `p6-new-verifier-input-anchor-and-document-citation` | each unverifiable shape; F1 and F3 at the classification step; the F14 precondition |
| `p6-new-attestation-identity-unique-per-decision-oid` | F10 |
| `p6-new-attestation-classes-and-cited-fields` | F14, F15 |
| `p6-new-attestation-sibling-model-stays-candidate` | F2, F3 |
| `p6-new-attestation-verified-only-via-witness` | F1 |
| `p6-new-witness-evidence-id-equals-attestation` | F1, F3 |
| `p6-new-witness-degradation-never-silent` | F1 |
| `p6-new-attestation-write-set-one-outcome` | F11 |
| `p6-new-mismatch-stale-unavailable-silent` | other bytes at the cited path; `Unavailable` verdicts (F5, F6) |
| `p6-new-party-distinct-and-self-refusal` | an attestation in the store; an undecodable anchor on it, so a read before the check fails; a plugin write under `verifier` |
| `p6-new-flag-off-nothing-runs` | F12 |
| `p6-new-pass-bounds-and-accounting` | F7, F10, F11, F13 |
| `p6-new-cursor-never-skips-unresolved` | F7, F13 |
| `p6-new-pass-progress-under-transient-unavailable` | F13 |
| `p6-new-attestation-stale-on-byte-change` | a second descriptor revision of the cited document with other bytes; F2 on the attestation; F7 at the stale mark's commit poll and the page poll; F10; F13 |
| `p6-new-verdict-deterministic` | F8, F9 |
| `p6-new-blob-read-bounded-at-oid` | F4, F5, F6, F7 |

## Coverage checks to add

Coverage checks assert the independent preconditions of a vulnerable window,
never the violation.

- `sometimes(verifier_header_under_budget_and_delta_base_over_limit)`: a
  blob whose header passes the gate while its delta base exceeds the
  allocation limit. Constructed today by
  `a_blob_whose_delta_base_exceeds_the_allocation_limit_is_over_budget`.
- `sometimes(verifier_claim_length_differs_but_reachable_by_line_endings)`:
  a claim whose length differs from the header size inside the reachable
  range, so the blob is read. Constructed today by
  `line_endings_normalize_in_either_direction` and
  `the_header_gates_every_blob_read`.
- `sometimes(verifier_claim_reevaluated_after_worktree_and_head_change)`:
  one claim evaluated before and after a worktree edit and a HEAD move.
  Constructed today by
  `verdicts_survive_a_worktree_edit_a_head_move_and_a_repack`.
- `sometimes(classifier_cited_evidence_invalidated_while_decision_live)`: a
  live `Active` decision whose cited evidence `delete_artifact` invalidated.
  Constructed today by
  `each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written`.
- `sometimes(classifier_tip_differs_from_requested_sequence)`: a commit after
  the requested sequence that changes the decision's classification at the
  tip. Constructed today by
  `classification_at_one_sequence_ignores_the_worktree_head_and_later_commits`.
- `sometimes(write_set_model_changed_after_classification)`: a model
  decision superseded, retired, or re-disposed between its classification and
  the write set's commit. Constructed today by
  `a_model_decision_changed_after_classification_aborts_the_set`.
- `sometimes(write_set_witness_trigger_fails_inside_envelope)`: a witness
  whose trigger fails validation in the envelope that inserted it.
  Constructed today by `a_witness_admitted_below_verified_aborts_the_envelope`.
- `sometimes(witness_event_after_mark_stale)`: a validated witness on a
  `Stale` subject, for
  `p6-mark-stale-withdraws-support-and-is-terminal-in-place`. No test
  constructs it.

## Ranking by cheapest valid oracle

1. The pure `verdict` function: observed-value doubles, no repository.
2. The verifier driver over bare fixtures: `git` plumbing and hand-built
   packs; the `Evaluation` value and the `Deadline` double's poll log are the
   oracle.
3. Kernel admission and slice fixtures for S1, S2, and S7.
   The classifier fixture store for N1 is in this tier: Kernel commits and
   reads with no repository.
4. Source-capture tests for S5, which need the daemon test host.
5. The bounded pass (#1173): a fixture store, bare repositories as project
   roots, a recording `PassClock` double, and a switchable flag reader; the
   counters block and the clock's poll log are the oracle. The applicability
   step (#1174) runs in the same fixture, with a second descriptor revision
   published through the Kernel; the admission rows and receipts are the
   oracle.
