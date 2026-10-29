# Attestation verifier: fault map

Fault classes come from the specification's fault map
([#1169](https://github.com/ahrav/eidnara/issues/1169), Fault and state
coverage). Availability names what can construct the fault at the #1170
branch.

## Fault classes

| Id | Fault | Availability |
| --- | --- | --- |
| F1 | Trigger absent, foreign, wrong kind, invalidated, at another revision, or with retired backing evidence | constructible: Kernel admission fixtures |
| F2 | Model decision superseded, retired, or re-disposed between selection and commit | constructible: Kernel slice fixtures; the verifier write set lands with #1172 |
| F3 | Evidence or anchor invalidated in that window; `delete_artifact` bypassing the citation guard | constructible: `retire_evidence` and `delete_artifact` in Kernel tests |
| F4 | Header over budget; header under budget with an over-budget delta base; hash mismatch on decode | constructible: large blobs, `write_delta_pack` in `crates/daemon/tests/support/git_repo.rs`, and a valid object stored under another id |
| F5 | Partial clone or missing object; corrupt commit, tree, or blob | constructible: a deleted loose object stands in for a missing promisor object; a valid object stored under another id, an undecodable object written with `git hash-object --literally`, a loose object whose bytes are not zlib data, a commit or tree with a valid prefix and a malformed remainder, or `write_self_delta_pack`'s self-referencing ref-delta, for corruption |
| F6 | Path absent; gitlink, symlink, or tree at the path; path beneath a gitlink | constructible: `git mktree --missing` in a bare fixture |
| F7 | Deadline before the pass, between decisions, and during a read; budget cancellation | constructible for one claim: a cancelled or expired `EvalBudget`, and a stage-aware `Deadline` double; between decisions with #1173 |
| F8 | Worktree edit, HEAD move, repack between two verdicts; the repack clause excludes allocation-limit refusals of the commit, a tree, or the blob, whose verdict depends on pack layout | constructible: a linked worktree of the bare fixture, `git update-ref`, `git repack -a -d` |
| F9 | `text eol`, `ident`, encoding, or clean filter attributes at the oid | partial: the LF and CRLF forms are constructible; `ident`, encoding, and clean filters stay false mismatches by the OQ6-8 decision |
| F10 | Operation key replayed with the same and with a different verdict | constructible with #1172 |
| F11 | Fault-injected rollback after the admission; crash after commit then fresh open | constructible with #1172 |
| F12 | Flag absent, reload true to false, project-tier flag | constructible with #1173 |
| F13 | More than N candidates; a starving two-project pattern; a permanently transient head decision | constructible with #1173 |
| F14 | A `Sensitive` evidence row cited through the row FK, the observation FK, or the admission event | constructible: Kernel slice and admission fixtures |
| F15 | Default prose documents with no code-or-config kind | constructible with #1172 (OQ6-9) |

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
| `p6-new-verifiability-pure-function-of-snapshot` | F8 at the classification step |
| `p6-new-verifier-input-anchor-and-document-citation` | each unverifiable shape |
| `p6-new-attestation-identity-unique-per-decision-oid` | F10 |
| `p6-new-attestation-classes-and-cited-fields` | F14, F15 |
| `p6-new-attestation-sibling-model-stays-candidate` | F2 |
| `p6-new-attestation-verified-only-via-witness` | F1 |
| `p6-new-witness-evidence-id-equals-attestation` | F1, F3 |
| `p6-new-witness-degradation-never-silent` | F1 |
| `p6-new-attestation-write-set-one-outcome` | F11 |
| `p6-new-mismatch-stale-unavailable-silent` | F5, F6 |
| `p6-new-party-distinct-and-self-refusal` | an attestation in the store |
| `p6-new-flag-off-nothing-runs` | F12 |
| `p6-new-pass-bounds-and-accounting` | F7, F13 |
| `p6-new-cursor-never-skips-unresolved` | F7, F13 |
| `p6-new-pass-progress-under-transient-unavailable` | F13 |
| `p6-new-attestation-stale-on-byte-change` | a descendant commit changing the cited bytes |
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
4. Source-capture tests for S5, which need the daemon test host.
5. The bounded pass (#1173), which needs a test host, a controlled clock, and
   tiered flags.
