# Portfolio evaluation: dispute evidence on kernel.commit

Independent fresh-context pass at HEAD `16660c3dd` over `catalog.md` (ten
records, nine with evidence files), `existing-checks.md`, `fault-map.md`,
and the nine evidence files. The cited code was re-read at that HEAD:
`commit.rs`, `dispute.rs`, `envelope.rs`, `slice/write.rs`,
`admission.rs`, `kernel_routes.rs`, `memory-mark-command.ts` and its
test. Short names, index order: requirement, liveness, visibility, form,
reporter, standing, identity, preview, reason, use-outcome.

Citation note: every `file:line` below is corrected to HEAD `f68e895ca`.
The review-round code commit `511cd6d73` adds lines to `dispute.rs` and
appends route tests to `kernel_routes.rs`; `f68e895ca` then removes five
lines from `dispute.rs`'s `hex_value`. Quoted wrong citations keep their
original numbers. The finding text is the evaluator's;
the Disposition column and the dispositions under "Gaps queued" are
added after it.

## Scope

This pass evaluates the portfolio as specification input. It does not
judge whether the existing tests are adequate. That verdict belongs to
`/testing:invariant-test-review`. Each finding is a gap (a missing
property or question), a refinement (a correction to an existing record),
or a bias (a framing decision for a human).

## Citation sample

About 45 `file:line` citations were checked. Most are exact, or are off
by a doc-comment line. Examples: `commit.rs:12-63`, `:206-234`,
`:719-721`, `:725-726`, `:742-757`, `:1045-1060`, `:1109-1123`,
`:1149-1157`; `dispute.rs:49-79`, `:103-143`, `:150-163`, `:209-213`;
`envelope.rs:548-615` (the read, renamed `evidence_cited_in` since);
`scope.rs:710-716`;
`lib.rs:14529`; `admission.rs:4168-4175`, `:4532`; all eight route test
lines from `kernel_routes.rs:5723` to `:6199`; and all seven TypeScript
test lines.

The following were wrong at `16660c3dd`:

- `existing-checks.md`, the `dispute.rs` unit-test table. Every line was
  off by 17. The listed lines were 252, 266, 300, 328, 340, 356, 376 and
  390. At `f68e895ca` the tests are at 232, 246, 314, 342, 354, 370, 390
  and 404.
- `catalog.md`, form record, `Exercised:`. It cited `dispute.rs:300`,
  `:328`, `:340` and `:266`. At `f68e895ca` these are 314, 342, 354 and
  246.
- `catalog.md`, reason record, `Existing check:`. It cited `:390`. At
  `f68e895ca` the test is at 404.
- `commit.rs:1066-1068` is the comment that interprets a conflict. The
  receipt lookup itself runs inside `KernelStore::commit_before`. This is
  minor.

## 1. Harness fit

No record needs a simulator. Each `always` predicate is per-request state
under one writer transaction.

| Record | Route | Why |
|---|---|---|
| requirement | route test with raw bodies, plus TS parser | refusal happens before any transaction (`commit.rs:1117-1123`) |
| liveness | route test; kernel lock harness for the queued case | `retire_evidence` store API |
| visibility | route test with two projects; kernel test for the edge citer | asserted-edge branch has no test |
| form | `dispute.rs` unit tests plus route | pure predicates |
| reporter | route test, per request | see R4 |
| standing | kernel evaluator grid plus route | `evaluate_admission` is pure |
| identity | TS unit plus route replay | `memoryMarkOperationId` is pure |
| preview | route test with the four-count helper | `assert_refused_without_writing` (`kernel_routes.rs:5703`) |
| reason | `dispute.rs` unit plus route parse-back | pure grammar |
| use-outcome | deferred to PR B | no code at HEAD |

## 2. Coverage balance

The ten records cover four concerns:

- Evidence presence and form: requirement, form, reason.
- Evidence admissibility: liveness, visibility.
- Standing and reporter: standing, reporter.
- Replay safety: identity, preview.

The balance against the risk model is uneven in one place. The
admissibility pair guards the dispute operation in isolation. It does not
cover the other operations in the same envelope, which can create the
citer the check looks for (G1).

The distribution is 8 `always`, 2 `always-or-unreached`, and no
`sometimes`, liveness, or reachability records. That fits a
request-scoped validator. Situation coverage lives in the seven
fault-map markers. None of the seven exists in code: `git grep` finds no
marker name outside `docs/` (R5).

Two records were partial while the model tool arm waited on #1161; both
now read `Exercised: yes` with the tool arm's tests named. The bypass
through Kernel API writers is an open question
in requirement and liveness, marked "needs human input". It is recorded
consistently.

## 3. Implementability

- **Reporter.** The check compares the reason's taint with
  `derive_classes(commit.source_kind)`. `commit_log` stores no
  `source_kind` (`open-questions.md` C2). The admission row's
  `source_kind` column holds the subject's facts, not the request's. The
  check can only be evaluated per request in a route test (R4).
- **Requirement.** The second clause says "every route-written row of the
  other three disposition events". For those events the constant reason
  `kernel.commit` is the only marker of a route-written row, so the clause
  is circular as a store scan. It works as a per-request route oracle
  (R4).
- **Visibility.** The check is stated "at `r.commit_seq`". A store-wide
  check therefore needs as-of reconstruction of citer liveness and scope
  terms. A per-request check needs nothing extra.
- **Liveness.** A queued retirement needs a second writer blocked on the
  lock. The fault map lists this as unavailable, which is correct.

## 4. Wildcard

- **Self-citation.** `insert_decision` and `insert_observation` accept a
  caller `evidence_id` (`commit.rs:274`, `:314`). The Kernel checks only
  that the row is live (`require_parents`, `slice/write.rs:1095-1107`).
  The tests' `project_evidence` helper uses exactly this path to make
  evidence citable (`kernel_routes.rs:5684-5698`).
  - `apply` runs operations in order inside one transaction
    (`commit.rs:899-1039`). I infer, without having tested it, that a
    single envelope holding `insert_decision` citing E, then a dispute
    citing E, passes `require_visible_evidence`.
  - The insert also distinguishes a live id (`available`) from an unknown
    id (`not_found`). That is the existence oracle `open-questions.md`
    Q1(b) says the uniform `not_found` closes.
  - The visibility record's Guarantee and Impact sentences overstate what
    the check enforces. I did not determine whether evidence ids are
    guessable (G1).
- **Approved relaxation.** The route accepts `approval_object_id` on a
  dispute (`commit.rs:158-159`). A valid approval permits a move toward a
  less restrictive disposition (`admission.rs:321-334`). An approved
  `mark_disputed` against a `contradicted` prior lands `disputed`. That
  breaks the standing check
  `restrictiveness(r) >= restrictiveness(p)`. The grid test fixes
  `approval_valid: false` (`admission.rs:4580`), and no route test sends
  an approval with a dispute (R1).
- **Whitespace in paths.** The command splits arguments on `/\s+/`
  (`memory-mark-command.ts:94-97`). A path containing a space cannot be
  cited from the command. The route accepts such paths
  (`kernel_routes.rs:5880`); the usage text names the limit since
  `511cd6d73` (`memory-mark-command.ts:52`), which closed R2. Whether the
  command should accept quoted or escaped paths stays open on the form
  record.

## Finding disposition

| Id | Class | Finding and action | Disposition |
|---|---|---|---|
| G1 | gap | Visibility check is satisfiable by self-citation through route inserts; insert answers leak evidence existence. Add one record or an open question on visibility (needs human input) | Queued as an open question (needs human input) on `cited-evidence-visible-to-bound-project`. The test `an_insert_citing_evidence_makes_it_citable_later_in_the_same_commit` (`kernel_routes.rs:6516`) shows a dispute citing uncited live evidence alone answers `not_found`, and the same dispute after an `insert_decision` citing that evidence in one envelope lands with the id on the row. Under the resolved OQ4-3 definition, evidence is citable once this project cites it, in the same envelope included. The insert path's live-versus-unknown answer is pre-existing behavior outside #1160 |
| G2 | gap | No test sends `approval_object_id` on a dispute; add the route case alongside R1 | Closed by `a_dispute_relaxes_a_contradicted_prior_only_with_a_valid_approval` (`kernel_routes.rs:6459`): without the approval the dispute is `denied: true` and the decision stays `contradicted`; with a valid approval it answers `denied: false`, lands `disputed`, and the row reads `outcome = deny` with the evidence id |
| R1 | refinement | Standing: condition the restrictiveness clause on `approval_object_id IS NULL`, or state the approved-relaxation clause; fix the Guarantee | Applied: the standing Guarantee and Check hold the restrictiveness clause unless the row names a valid approval, matching AC8 "no relaxation without a valid approval" |
| R2 | refinement | Form: add an open question on whitespace paths in the command, next to the `%XX` one | Applied: open question on the form record; at `511cd6d73` the usage names the limit (`memory-mark-command.ts:52`) |
| R3 | refinement | Fix the `dispute.rs` line numbers in `existing-checks.md` and in the form and reason records | Applied to every `dispute.rs` line number in both parts, at `f68e895ca` |
| R4 | refinement | Reporter and requirement: state the checks as per-request route oracles; name the request fields they read | Applied: both Check lines are per-request route oracles naming `source_kind`, `asserted_source_class`, `asserted_taint_class` (`commit.rs:115-119`) and the operation's `event`, `evidence_id`, `evidence_ref` (`commit.rs:155-172`) |
| R5 | refinement | Fault map: rename "Constructed by" to "Precondition reached by" and state that no marker exists in code | Applied in `fault-map.md` |
| R6 | refinement | Scope says "Nine records are active" but ten carry `Status: active`; `active (proposed)` is not a schema value; use-outcome's `default-production (PR B)` is not verifiable at HEAD (METHOD rule 4) | Applied: the Scope and the part README count ten active records, nine implemented and one proposed; the index Status reads `active`; the use-outcome Reachability line states the label is unverified at HEAD |
| B1 | bias | Tests and records share a design source | Surfaced for a human (below) |
| B2 | bias | Part boundary drawn at the dispute operation | Surfaced for a human (below) |

## Gaps queued

- **G1.** Read `commit.rs:899-1039` and `slice/write.rs:1095-1107`.
  Write one route test: in one envelope, `insert_decision` citing
  evidence that only project B cites, then a dispute citing it. Record
  the result. An owner then decides whether visibility means "already
  cited by this project" or "citable by this project", and whether
  inserts should get the same check.
  - Disposition: the route test landed as
    `an_insert_citing_evidence_makes_it_citable_later_in_the_same_commit`
    (`kernel_routes.rs:6516`). It uses uncited live evidence rather than
    evidence only project B cites; the insert path accepts any live
    evidence id (`require_parents` checks liveness only), so the result
    covers both. The owner question stays open on
    `cited-evidence-visible-to-bound-project` (needs human input).
- **G2.** One route case: a dispute with a valid approval against a
  `contradicted` prior. Assert the landed disposition and `denied: false`.
  No new record is needed. It closes R1.
  - Disposition: closed by `kernel_routes.rs:6459`.

## Biases for a human

- **B1.** The route tests are named after the records. For example,
  `a_dispute_reporter_class_is_derived_and_never_asserted_up` matches the
  slug `reporter-class-derived-never-asserted-up`. "Exercised: yes"
  therefore confirms the design against itself. Decide whether "yes"
  stands before `/testing:invariant-test-review` audits these tests.
- **B2.** The part covers the dispute operation and its fields. The
  envelope composes operations in one transaction, and G1 lives in that
  composition. The Kernel API bypass in requirement and liveness is the
  same boundary choice. Decide whether the part covers the envelope or
  only the dispute operation. If it covers the envelope, fund one
  cross-operation pass.

## Verdict

The catalog works as specification input once R1 and R3 land. R1
corrects a check that is false for a reachable input. R3 fixes citations
a ticket would copy. G1 is the only finding that weakens a stated
security property. It needs an owner decision before the visibility
record's Guarantee is cited as a cross-project boundary. The other
refinements are wording and schema fixes.
