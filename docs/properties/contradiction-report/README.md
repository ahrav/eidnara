# Contradiction report property catalog

This part holds the property catalog for the specification "Consumer-filed
contradiction report (adaptive-discovery P4)": the `kernel.commit`
`disposition` operation extended so a `mark_disputed` or `contradict` event
carries validated evidence (a live Kernel evidence id, or a repository path at
a git object id), records the reporter's derived class, and can only lower a
finding's standing.

Inspected source revision: `3a3b916b7cbb34ec681531affeca495189221902` (HEAD at
authoring; the settled plan's baseline is `a0e3f2993`, and every code file the
plan cites is unchanged between the two). Date: 2026-10-25. Every `file:line`
reference was verified at that revision by the pass that wrote it; the
fresh-context portfolio evaluation re-read every citation in the implemented
part and the two it found off are corrected inline (refinements R1 and R2).
Records for implemented behavior are claims under test, not proof. Records for
proposed behavior describe obligations PR A and PR B must make checkable; none
is exercised at HEAD.

The published specification issue reference is recorded here after
publication: [https://github.com/ahrav/eidnara/issues/1159](https://github.com/ahrav/eidnara/issues/1159).

The record contract is [`../METHOD.md`](../METHOD.md). The evidence scope was
user-confirmed as: the plan set (`00-index`, P3, P4 plans) plus repository
code, tests, `docs/host-wire-protocol.md`, `docs/memory-reviewer-operations.md`,
and `docs/properties` at HEAD; no tracker issues, incident history, or git
history lens. `fact-sheet.md` is the verified fact sheet both passes started
from; their corrections to it are stated in `disposition-path/system-model.md`
and `dispute-evidence/domain-model.md` section 0.

## Parts

| Part | Surfaces | Records | Origin | Companions |
| --- | --- | --- | --- | --- |
| `disposition-path/` | the daemon route (`Operation::Disposition`, `DispositionEvent`, `disposition_request`, `derive_classes`, preview, scope filtering, refusal mapping, envelope replay); Kernel admission for the five disposition events (`evaluate_admission`, `event_effect`, `disposition_restrictiveness`, `write_admission`, `load_subject_facts`); the shared TypeScript command runner and client intent derivation; evidence liveness primitives | 14 | implemented | `system-model.md`, `existing-checks.md`, `fault-map.md`, `portfolio-evaluation.md`, `evidence/` |
| `dispute-evidence/` | evidence fields on the dispute operation; daemon-side liveness, visibility, and form validation; reporter class; dispute identity; preview parity; structured `reason`; use-outcome link | 10 | proposed | `domain-model.md`, `certificate.md`, `open-questions.md` |

Record origin: every record under `disposition-path/catalog.md` is implemented
behavior at HEAD. Every record under `dispute-evidence/catalog.md` is headed
`(PROPOSED)` and carries a `Lifecycle: proposed obligation` line, a falsifier,
and a legal witness; its `Confidence` is `low - proposed; no implementation
evidence`. The proposed part carries no portfolio result; `certificate.md` is
the domain model's output certificate and `open-questions.md` records every
conflict between the plan and repository evidence with a recommended default
and the alternative.

`disposition-path/portfolio-evaluation.md` is the fresh-context evaluation of
the implemented part. Its ten refinements are applied in `catalog.md` and
marked inline; its gaps G1 to G4 and biases B1 to B3 are carried into the
specification's open questions.

## Slug index and specification crosswalk

Specification sections: C = Constraints and Invariants, D = Implementation
Decisions, AC = Acceptance Criteria, OQ = Open Questions.

| Slug | Part | Type, semantics | Specification |
| --- | --- | --- | --- |
| `disposition-admission-copies-the-subjects-prior-classes` | implemented | safety, `always` | C3, C9 |
| `disposition-never-raises-maturity-and-withdraws-effective-support` | implemented | safety, `always` | C1, AC8 |
| `disposition-approval-requirement-follows-the-restrictiveness-order` | implemented | safety, `always` | C1, C2, OQ4-9 |
| `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` | implemented (pin; PR A deletes or re-labels) | safety, `always` | C5, D4 |
| `admission-evidence-citation-is-existence-checked-only` | implemented (pin on the accept-retired branch) | safety, `always` | C6, OQ4-5 |
| `slice-write-refuses-non-live-evidence-citations` | implemented | safety, `always` | C6 |
| `evidence-objects-carry-no-scope-and-fail-scoped-lookup` | implemented (pin) | safety, `always` | C7, OQ4-3 |
| `disposition-subject-is-a-live-admitted-decision-in-the-bound-project` | implemented | safety, `always` | C4 |
| `disposition-preview-writes-nothing-and-reports-the-commit-verdict` | implemented | safety, `always` | C11, AC7 |
| `disposition-envelope-identity-replays-without-a-second-write` | implemented | safety, `always` | C10, AC6 |
| `memory-mark-command-commits-only-the-previewed-operation` | implemented | safety, `always` | C11, D7 |
| `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` | implemented | safety, `always` | C12, C16, OQ4-10 |
| `disposition-commit-log-row-carries-actor-and-cause-only` | implemented (pin until PR B) | safety, `always` | C9, OQ4-4, AC11 |
| `admission-reason-is-redacted-bounded-and-non-blank` | implemented | safety, `always` | C12, D5 |
| `dispute-requires-evidence-from-plugin` | proposed (PR A) | safety, `always` | C5, AC4 |
| `cited-evidence-live-at-dispute-commit` | proposed (PR A) | safety, `always` | C6, AC2 |
| `cited-evidence-visible-to-bound-project` | proposed (PR A) | safety, `always` | C7, OQ4-3, AC2 |
| `evidence-ref-well-formed-and-redaction-stable` | proposed (PR A) | safety, `always` | C8, AC3 |
| `reporter-class-derived-never-asserted-up` | proposed (PR A) | safety, `always` | C9, AC5 |
| `dispute-with-evidence-never-raises-standing` | proposed (PR A) | safety, `always` | C1, AC1, AC8 |
| `dispute-identity-includes-evidence-key` | proposed (PR A) | safety, `always` | C10, D7, AC6 |
| `dispute-preview-writes-nothing` | proposed (PR A) | safety, `always-or-unreached` | C11, AC7 |
| `structured-reason-projects-typed-reporter-data` | proposed (PR A; typed form PR B) | safety, `always` | C12, D5, D9, AC3 |
| `use-outcome-link-names-own-session-prior-serve` | proposed (PR B) | safety, `always-or-unreached` | D10, AC12 |

## Landing

PR A lands this part on `main` per `METHOD.md`: re-verify every `file:line` at
the PR's branch tip, convert the first nine proposed records to active as their
checks become executable, delete or re-label the four pinned records whose
behavior PR A changes, and add the part to `../README.md`. PR B converts the
tenth record and the typed form of the structured-reason record. Until then
this part lives on the specification branch named in the specification.
