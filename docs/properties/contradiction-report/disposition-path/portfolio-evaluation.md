# Portfolio evaluation: kernel.commit disposition path

HEAD `3a3b916b7`, 2026-10-25. Independent fresh-context pass over
`catalog.md` (14 records), `system-model.md`, `existing-checks.md`,
`fault-map.md`, and all 14 evidence files; every file:line below was re-read
at HEAD. Short names, index order: classes, maturity, restrictiveness,
no-evidence, evidence-existence, slice-liveness, evidence-scope, subject,
preview, replay, command, cascade, commit-log, reason.

Reference note (PR A, #1160): this document describes the code at
`3a3b916b7` and is kept as the design-time record. Its `file:line`
references were remapped to the PR A branch tip (base `6159fd224`, code
commit `6c4d445e3`) through the git diff between the two revisions; where
PR A changed the cited code, the text says so inline with "PR A:". The
active records in [`dispute-evidence/catalog.md`](../dispute-evidence/catalog.md)
and [`disposition-path/catalog.md`](catalog.md) state the PR A
behavior.

## 1. Harness fit

Routing for `/testing:test-strategy`. No record needs a simulator: the
single `Immediate` writer transaction (`envelope.rs:1383-1386`) turns every
"retired between read and commit" window into two sequential requests, and
all 14 `always` checks hold over per-request state.

| Record | Route | Why |
|---|---|---|
| classes | route-level daemon test | reads the written row; store helpers exist (`kernel_routes.rs:4340`) |
| maturity | kernel unit (evaluator) plus one route row read | `evaluate_admission` is pure (`admission.rs:280-367`) |
| restrictiveness | kernel unit (7x5 grid) plus route cases for lateral moves | table at `admission.rs:4272-4279` |
| no-evidence | route-level, raw JSON body | parse refusal at `mod.rs:463-471` |
| evidence-existence | kernel integration (store API) | the route set `None` at `3a3b916b7`; PR A sets a checked id (`commit.rs:744`) |
| slice-liveness | kernel integration; one route case for the `not_found` mapping | helpers private to `write.rs` |
| evidence-scope | route-level for `not_found`; kernel unit for `scope_id == None` | see R3 |
| subject | route-level daemon test | predicates in `disposition_request` (`commit.rs:706-730`) |
| preview | route-level grid test | re-route from stateful-workload-design (R10) |
| replay | route-level for replay and reuse; TypeScript unit for the reissue | `client.ts:462-466` owns the reissue |
| command | TypeScript unit test | `FakeKernel` exists |
| cascade | route-level, store-API seeding | `seed_approval` (`kernel_routes.rs:4362`) |
| commit-log | route-level daemon test | reads the `commit_log` row |
| reason | kernel unit (store API) | route reason is constant (`commit.rs:100`) |

## 2. Coverage balance against the risk model

Only lower standing: maturity, restrictiveness, cascade, plus the proposed
`dispute-with-evidence-never-raises-standing`. Dense but not redundant:
the first two read different columns of one evaluation; cascade adds the
dependents.

Cite live evidence: evidence-existence, slice-liveness, and evidence-scope
pin the primitives; no-evidence pins that HEAD carries none. The proposed
set owns the new behavior and rests on these facts.

Record who filed it: commit-log (what is stored) and classes (the row keeps
the subject's classes). Missing: a record that `actor`, `cause`, and
`source_kind` are caller-asserted. `resolve_classes` only refuses
over-declaration relative to the asserted `source_kind`
(`commit.rs:413-446`); `RouteScope` holds `harness` and the binding
(`mod.rs:405-412`) but nothing compares them to the intent; `session_id`
is stripped (`mod.rs:461`). Gap G1; the proposed set has only question Q4.

Replay safely: replay, preview, command; per-identity accounting is the
primary oracle.

Zero liveness and zero reachability records is correct for this surface:
the system model finds no liveness claim, `deadline_ms` bounds writer
acquisition only (`commit.rs:88-93`), and the deferred demotions past 4,096
are an open question. Situation coverage lives in 21 `sometimes` markers in
`fault-map.md`, which have no handoff row (B2).

## 3. Implementability

Observability: every column check is readable through the test store
connection at route level. Two checks name kernel-internal functions a
daemon test cannot call: evidence-scope cites `load_object_state`
(`pub(super)`, `envelope.rs:1905`) and cascade cites `validate_approval`;
the route-visible proxies are `not_found` and a later relaxation denied.

Constructibility: F1 to F11 and F13 to F16 are available as the fault map
says. F7 (secret-like text) is listed as available but the detector set was
not read, so no known-detected token exists for commit-log and reason (G3).
The cascade `deferred` branch needs 4,096 dependents (R7). The maturity
record needs an `Enforced` subject (`Enforce` requires evidence and
approval, `admission.rs:291-293`); store-API constructible, in no fixture.

The `sometimes` markers are preconditions a correct system reaches (classes
differ, prior above Candidate, retired id cited); none negates a check.

## 4. Vacuity

- no-evidence: the `evidence_id IS NULL` clause passes on an empty table;
  add a row-existence floor (R9).
- evidence-existence: `admission_write_with_retired_evidence_id` must fire
  before `Exercised` leaves `not yet`; the accept-on-retired branch is the
  whole record.
- command: `confirm` at most once is trivial when `visibility_changes` is
  false; no marker names that precondition (R6).
- cascade: the `deferred` disjunct can never be false in an affordable
  test and should not count as covered (R7).

## 5. Wildcard

- Baseline pins: no-evidence, commit-log, evidence-scope, and the
  accept-retired branch of evidence-existence assert behavior P4 intends to
  change; a test written from them fails on the intended change (B1).
- Replay after move: `memory-mark-command.test.ts:351` pins that a repeat
  reports `applied` with the original verdict while the stored disposition
  differs (`commit.rs:1239-1255`). Safe for the store, possibly misleading
  for the user; no record asks (R5).
- Preview shows the subject's visibility only; a dispute on an approval
  demotes dependents the confirmation never names (`commit.rs:845-856`,
  `changes_to` at `:537-542`). Gap G2.
- Superseded to Disputed is lateral at rank 1 and lands without approval;
  the restrictiveness open question names Stale only (R8).
- Thirteen of fourteen discovery triggers cite a P4 statement; both gaps
  above are unmentioned by the plan (B3).

## Finding disposition

| Id | Class | Finding and action |
|---|---|---|
| G1 | gap | reporter identity is caller-asserted; one record on `actor`/`source_kind` provenance |
| G2 | gap | preview `visibility_changes` excludes cascade dependents; record or open question on preview |
| G3 | gap | no known-detected token for F7; read `crates/secret-scanner`, test a 40/64-hex oid and a path (closes reason's question) |
| G4 | gap | callers of `append_decision_citations` (`write.rs:240`) and id dedup; closes slice-liveness's second question |
| R1 | refinement | evidence-existence: `memory_reviewer/lifecycle.rs:649` is inside `#[cfg(test)]` (module at `:500`) and is an `ObservationSpec`; the only production admission writer with `Some(evidence_id)` is `harness_sources.rs:976` (`EventKind::Other`). Fix Reachability |
| R2 | refinement | slice-liveness: check names `append_decision_events`; the function is `append_decision_citations`, `pub(crate)`, with no caller in `kernel_routes`. Limit the route clause to `require_parents`; label the other clause's reachability |
| R3 | refinement | evidence-scope: split observation into route `not_found` and a kernel-crate `scope_id == None` assertion |
| R4 | refinement | subject: evidence says the retired-stored-approval outcome "was not traced"; record says `Open questions: None`. Add it |
| R5 | refinement | replay: add open question on reporting the original verdict after a move (needs human input) |
| R6 | refinement | command: preview null verdict is `refused/preview invalid(internal)` (`memory-mark-command.ts:227-229`), commit null is `outcome_unknown` (`:240-242`); fix the check; add marker `command_preview_visibility_changes` |
| R7 | refinement | cascade: mark the `deferred` branch `Exercised: not yet - 4,096 dependents` |
| R8 | refinement | restrictiveness: extend the lateral-move question to Superseded to Disputed |
| R9 | refinement | no-evidence: add a row-existence floor to the second clause |
| R10 | refinement | preview handoff: route-level grid test, not stateful-workload-design |
| B1 | bias | baseline pins versus frozen contract |
| B2 | bias | coverage markers outside the record set |
| B3 | bias | plan-anchored discovery |

## Gaps queued

- G1: read `kernel_route_scope` (`mod.rs:415-441`), `IntentRequest`
  (`project.rs:138-146`), `resolve_classes`; one record on what the daemon
  verifies about the intent and `source_kind`.
- G2: read `PreviewRun` (`commit.rs:839-898`); owner decides whether
  cascade visibility is part of the preview contract; at most one record.
- G3: read the `crates/secret-scanner` detectors; name one detected token
  and the result for a path and a hex oid. G4: grep callers of
  `append_decision_citations`. Neither adds a record.

## Biases for a human

- B1. Four records pin HEAD behavior the plan changes (no-evidence: "the
  baseline P4 PR A changes"; commit-log: "until Batch K1"; evidence-scope:
  R2 undefined; evidence-existence: Kernel-side refusal undecided). Decide
  per record: frozen contract, or pre-change pin with a deletion trigger.
- B2. The 21 `sometimes` markers carry all situation coverage and appear in
  no handoff row. Decide whether they become ticketed floors.
- B3. Discovery scope followed the plan's claims. Decide whether the scope
  is the plan (then the catalog is complete) or the risk model (then fund
  one more pass on trust boundaries and preview semantics).

## Verdict

The catalog is fit as specification input once the ten refinements are
applied; R1 and R2 correct factual citations and must land before a ticket
cites those records. The four gaps are bounded to single passes and do not
block; G1 is the only one that adds a record the risk model requires. The
three biases need owner decisions, and B1 changes which tickets get written.

## PR A disposition (#1160)

- G1: added as `commit-actor-cause-and-source-kind-are-caller-asserted`
  in [catalog.md](catalog.md), with its evidence file.
- G2: recorded as an open question on
  `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit`;
  OQ4-10 resolved to document the limit in the route header and the
  command usage.
- G3: recorded in
  [evidence/admission-reason-is-redacted-bounded-and-non-blank.md](evidence/admission-reason-is-redacted-bounded-and-non-blank.md)
  and in the dispute-evidence part's
  `evidence-ref-well-formed-and-redaction-stable` evidence.
- G4: recorded in
  [evidence/slice-write-refuses-non-live-evidence-citations.md](evidence/slice-write-refuses-non-live-evidence-citations.md);
  the callers were read, not changed.
- B1: the pinned records PR A changes are invalidated or re-labelled; the
  commit-log pin stays until PR B.
- B2: the markers the #1160 tests construct are named with their
  constructing tests in [fault-map.md](fault-map.md); the rest stay queued
  with no owning ticket.
