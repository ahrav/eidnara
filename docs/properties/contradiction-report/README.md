# Contradiction report property catalog

This part holds the property catalog for the specification "Consumer-filed
contradiction report (adaptive-discovery P4)": the `kernel.commit`
`disposition` operation extended so a `mark_disputed` or `contradict` event
carries validated evidence (a live Kernel evidence id, or a repository path at
a git object id), records the reporter's derived class, and can only lower a
finding's standing.

Authored at source revision `3a3b916b7cbb34ec681531affeca495189221902`
(the settled plan's baseline is `a0e3f2993`, and every code file the plan
cites is unchanged between the two). Date: 2026-10-25. PR A of
[#1160](https://github.com/ahrav/eidnara/issues/1160) lands the part: every
`file:line` reference is re-verified at the PR A branch tip (base
`6159fd224`, code commit `6c4d445e3`). References were remapped through the
git diff between the two revisions; references to lines PR A changed were
rewritten by hand, and the records PR A makes checkable were re-read
against the code and tests. The design-time companions (`fact-sheet.md`,
`disposition-path/system-model.md`, `disposition-path/portfolio-evaluation.md`,
`dispute-evidence/domain-model.md`, `dispute-evidence/open-questions.md`,
`dispute-evidence/certificate.md`) describe `3a3b916b7` and carry a
reference note saying so. References into
`crates/daemon/src/kernel_routes/commit.rs` and
`crates/daemon/src/kernel_routes/dispute.rs` were verified against the
working tree that adds a module-header revision on top of `6c4d445e3`: the
reason grammar moves from the `dispute` module header into the `commit`
module header. Records are claims under test, not proof.

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
| `disposition-path/` | the daemon route (`Operation::Disposition`, `DispositionEvent`, `disposition_request`, `derive_classes`, preview, scope filtering, refusal mapping, envelope replay); Kernel admission for the five disposition events (`evaluate_admission`, `event_effect`, `disposition_restrictiveness`, `write_admission`, `load_subject_facts`); the shared TypeScript command runner and client intent derivation; evidence liveness primitives; caller-asserted intent fields | 15 (14 active, 1 invalidated) | implemented | `system-model.md`, `existing-checks.md`, `fault-map.md`, `portfolio-evaluation.md`, `evidence/` |
| `dispute-evidence/` | evidence fields on the dispute operation; daemon-side liveness, visibility, and form validation; reporter class; dispute identity; preview parity; structured `reason`; use-outcome link | 10 active (9 implemented, 1 proposed for PR B) | implemented by PR A, one proposed | `existing-checks.md`, `fault-map.md`, `portfolio-evaluation.md`, `evidence/`, `domain-model.md`, `certificate.md`, `open-questions.md` |

Record origin: every record under `disposition-path/catalog.md` is
implemented behavior; PR A invalidates one and re-labels two (see that
catalog's PR A revision note). Under `dispute-evidence/catalog.md`, all
ten records carry `Status: active`; nine are implemented and exercised by
PR A tests; the tenth,
`use-outcome-link-names-own-session-prior-serve`, stays a proposed
obligation with its `Lifecycle:`, `Falsifier:`, and `Legal witness:` lines
and no evidence file until PR B
([#1162](https://github.com/ahrav/eidnara/issues/1162)). The model tool arm
of the dispute surface lands with
[#1161](https://github.com/ahrav/eidnara/issues/1161).

`disposition-path/portfolio-evaluation.md` is the fresh-context evaluation of
the implemented part. Its refinements are applied in `catalog.md`; PR A
resolves its gaps G1 to G4 (a new record, an open question, and two
evidence-file investigations) and the owned coverage markers of bias B2 are
named in `disposition-path/fault-map.md`.
`dispute-evidence/portfolio-evaluation.md` is the fresh-context
evaluation of the dispute-evidence part, with citations corrected to
HEAD `f68e895ca` (after the review-round code commits `511cd6d73` and
`f68e895ca`). Its refinements R1 to R6 are
applied; gap G2 is closed by a route test; gap G1 is an open question on
`cited-evidence-visible-to-bound-project` (needs human input); biases B1
and B2 are surfaced for a human.

## Slug index and specification crosswalk

Specification sections: C = Constraints and Invariants, D = Implementation
Decisions, AC = Acceptance Criteria, OQ = Open Questions.

| Slug | Part | Type, semantics | Specification |
| --- | --- | --- | --- |
| `disposition-admission-copies-the-subjects-prior-classes` | implemented | safety, `always` | C3, C9 |
| `disposition-never-raises-maturity-and-withdraws-effective-support` | implemented | safety, `always` | C1, AC8 |
| `disposition-approval-requirement-follows-the-restrictiveness-order` | implemented | safety, `always` | C1, C2, OQ4-9 |
| `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` | implemented (invalidated by PR A) | safety, `always` | C5, D4 |
| `admission-evidence-citation-is-existence-checked-only` | implemented (re-labelled by PR A: Kernel path only) | safety, `always` | C6, OQ4-5 |
| `slice-write-refuses-non-live-evidence-citations` | implemented | safety, `always` | C6 |
| `evidence-objects-carry-no-scope-and-fail-scoped-lookup` | implemented (re-labelled by PR A) | safety, `always` | C7, OQ4-3 |
| `disposition-subject-is-a-live-admitted-decision-in-the-bound-project` | implemented | safety, `always` | C4 |
| `disposition-preview-writes-nothing-and-reports-the-commit-verdict` | implemented | safety, `always` | C11, AC7 |
| `disposition-envelope-identity-replays-without-a-second-write` | implemented | safety, `always` | C10, AC6 |
| `memory-mark-command-commits-only-the-previewed-operation` | implemented | safety, `always` | C11, D7 |
| `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` | implemented | safety, `always` | C12, C16, OQ4-10 |
| `disposition-commit-log-row-carries-actor-and-cause-only` | implemented (pin until PR B) | safety, `always` | C9, OQ4-4, AC11 |
| `admission-reason-is-redacted-bounded-and-non-blank` | implemented | safety, `always` | C12, D5 |
| `commit-actor-cause-and-source-kind-are-caller-asserted` | implemented (added by PR A, gap G1) | safety, `always` | C9, OQ4-4 |
| `dispute-requires-evidence-from-plugin` | implemented (PR A) | safety, `always` | C5, AC4 |
| `cited-evidence-live-at-dispute-commit` | implemented (PR A) | safety, `always` | C6, AC2 |
| `cited-evidence-visible-to-bound-project` | implemented (PR A) | safety, `always` | C7, OQ4-3, AC2 |
| `evidence-ref-well-formed-and-redaction-stable` | implemented (PR A) | safety, `always` | C8, AC3 |
| `reporter-class-derived-never-asserted-up` | implemented (PR A) | safety, `always` | C9, AC5 |
| `dispute-with-evidence-never-raises-standing` | implemented (PR A) | safety, `always` | C1, AC1, AC8 |
| `dispute-identity-includes-evidence-key` | implemented (PR A) | safety, `always` | C10, D7, AC6 |
| `dispute-preview-writes-nothing` | implemented (PR A) | safety, `always-or-unreached` | C11, AC7 |
| `structured-reason-projects-typed-reporter-data` | implemented (PR A form; typed form PR B) | safety, `always` | C12, D5, D9, AC3 |
| `use-outcome-link-names-own-session-prior-serve` | proposed (PR B) | safety, `always-or-unreached` | D10, AC12 |

## Landing

PR A lands this part on `main` per `METHOD.md`: references re-verified at
the PR A branch tip, the first nine proposed records converted to active,
the pinned records PR A changes invalidated or re-labelled, and the part
added to `../README.md`. PR B converts the use-outcome link record and the
typed form of the structured-reason record, and changes the commit-log pin.
