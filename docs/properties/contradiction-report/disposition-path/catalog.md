# Property catalog: kernel.commit disposition path (existing behavior)

System: authored at `3a3b916b7` (2026-10-25); references re-verified at
the PR A branch tip of [#1160](https://github.com/ahrav/eidnara/issues/1160)
(base `6159fd224`, code commit `6c4d445e3`). Method:
`docs/properties/METHOD.md` and `/testing:property-discovery-and-catalog`.
Companion files: [system-model.md](system-model.md),
[existing-checks.md](existing-checks.md), [fault-map.md](fault-map.md),
[portfolio-evaluation.md](portfolio-evaluation.md), `evidence/<slug>.md`.

## Scope

The `kernel.commit` `disposition` operation as implemented at HEAD:

- the daemon route `crates/daemon/src/kernel_routes/commit.rs`
  (`Operation::Disposition`, `DispositionEvent`, `disposition_request`,
  `derive_classes`, preview, scope filtering), refusal mapping in
  `kernel_routes/state.rs`, envelope parsing in `kernel_routes/mod.rs`, and
  the receipt replay in `crates/kernel/src/envelope.rs`;
- the Kernel admission evaluation for the five disposition events in
  `crates/kernel/src/admission.rs` (`evaluate_admission`, `event_effect`,
  `disposition_restrictiveness`, `write_admission`, `load_subject_facts`);
- the TypeScript runner `packages/opencode-plugin/src/shared/memory-mark-
  command.ts` and the intent derivation in
  `shared/kernel-client/client.ts`;
- evidence liveness semantics the P4 work depends on: `evidence_meta`
  liveness, `require_live_evidence` and `require_optional_live` in
  `crates/kernel/src/slice/write.rs`, scope-less evidence objects in
  `envelope.rs`, and the admission path's existence-only FK check.

Proposed P4 behavior at authoring (evidence fields, reporter data,
use-outcome link) is cataloged in
[`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md). Plan
statements enter only as claims under test.

Revision note (2026-10-25, specification synthesis): the fresh-context
[portfolio evaluation](portfolio-evaluation.md) produced refinements R1 to
R10; all ten are applied in this revision and marked inline. Gaps G1 to G4
and biases B1 to B3 were carried into the specification's Open Questions.

Revision note (PR A, #1160): PR A adds evidence to disputes. Its effect on
this part:

- `disposition-operation-carries-no-evidence-and-refuses-unknown-fields`
  is `Status: invalidated`; its replacements are in the dispute-evidence
  part.
- `evidence-objects-carry-no-scope-and-fail-scoped-lookup` and
  `admission-evidence-citation-is-existence-checked-only` are re-labelled:
  both guarantees still hold for the Kernel, and the route now gates
  dispute evidence by its own visibility and liveness check.
- `disposition-commit-log-row-carries-actor-and-cause-only` stays a pin of
  the commit-log shape until PR B
  ([#1162](https://github.com/ahrav/eidnara/issues/1162)).
- G1 adds `commit-actor-cause-and-source-kind-are-caller-asserted`; G2 is
  an open question on
  `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit`;
  G3 and G4 are recorded in the evidence of
  `admission-reason-is-redacted-bounded-and-non-blank` and
  `slice-write-refuses-non-live-evidence-citations`.
- The `Exercised:` fields name the PR A tests that now construct states
  these records listed as missing.

## External-evidence scope

User-confirmed, recorded verbatim: "the plan set (00-index, P3, P4 plans)
plus repository code, tests, docs/host-wire-protocol.md,
docs/memory-reviewer-operations.md, and docs/properties at HEAD. No tracker
issues, no incident history, no git history lens."

References consulted and why:

- `docs/plans/2026-10-24-adaptive-discovery/04-contradiction-report-plan.md`:
  source of R1 to R7, KTD1 to KTD4, and the claims under test.
- `docs/plans/2026-10-24-adaptive-discovery/00-index.md`: D3 ("authority
  never derives from model confidence"), D4 (Batch K1), D5.
- `docs/plans/2026-10-24-adaptive-discovery/03-use-outcome-record-plan.md`:
  the join of later `MarkDisputed`/`Contradict` events by `commit_seq` and
  the sweep that keeps rows "a live admission event's evidence" cites.
- [`../fact-sheet.md`](../fact-sheet.md): verified fact sheet; every line it
  cites was re-read at HEAD, corrections noted in `system-model.md`.
- `docs/host-wire-protocol.md` 7.4 and 7.8: confirms no `kernel.*` method
  literal is listed; `kernel.commit` is handler business semantics.
- `docs/memory-reviewer-operations.md`: baseline-rotation consequence of a
  schema change; read for context, no record depends on it.
- `docs/properties/METHOD.md` and `docs/properties/AGENTS.md`: record
  schema and rules.

## Claims under test (from the plan set)

| Claim | Source | Record |
|---|---|---|
| The report never raises maturity or relaxes a disposition | P4 R4 | `disposition-never-raises-maturity-and-withdraws-effective-support`, `disposition-approval-requirement-follows-the-restrictiveness-order` |
| Active to Disputed needs no approval and lands regardless of source class | P4 R4 | `disposition-approval-requirement-follows-the-restrictiveness-order`, `disposition-admission-copies-the-subjects-prior-classes` |
| MarkDisputed forces maturity to Candidate | P4 Problem Frame | `disposition-never-raises-maturity-and-withdraws-effective-support` (effective maturity only) |
| The event is recorded under the subject's prior source class | P4 Problem Frame | `disposition-admission-copies-the-subjects-prior-classes` |
| Evidence is dropped at the disposition path | P4 Problem Frame | `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` (invalidated by PR A) |
| Authority never derives from model confidence | Index D3 | `disposition-never-raises-maturity-and-withdraws-effective-support`, `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` |

## Reachability classes

Every record is labelled individually. The shared evidence for
`default-production` on the route is one fact: the method match at
`crates/daemon/src/lib.rs:14529` dispatches `"kernel.commit"` to
`handle_kernel_commit` with no `#[cfg]` attribute, in a default build with
no configuration. The TypeScript command is registered in both harnesses:
`packages/pi-plugin/src/index.ts:550` and
`packages/opencode-plugin/src/hooks/context/command-handler.ts:413,471`.
Records on Kernel internals name the production writer that reaches them.

## Index

| Slug | Type | Reachability | Priority | Status | Exercised | Semantics |
|---|---|---|---|---|---|---|
| `disposition-admission-copies-the-subjects-prior-classes` | safety | default-production | high | active | partial | always |
| `disposition-never-raises-maturity-and-withdraws-effective-support` | safety | default-production | high | active | partial | always |
| `disposition-approval-requirement-follows-the-restrictiveness-order` | safety | default-production | high | active | partial | always |
| `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` | safety | default-production | medium | invalidated | not yet | always |
| `admission-evidence-citation-is-existence-checked-only` | safety | default-production | high | active | not yet | always |
| `slice-write-refuses-non-live-evidence-citations` | safety | default-production | medium | active | partial | always |
| `evidence-objects-carry-no-scope-and-fail-scoped-lookup` | safety | default-production | high | active | not yet | always |
| `disposition-subject-is-a-live-admitted-decision-in-the-bound-project` | safety | default-production | high | active | partial | always |
| `disposition-preview-writes-nothing-and-reports-the-commit-verdict` | safety | default-production | medium | active | partial | always |
| `disposition-envelope-identity-replays-without-a-second-write` | safety | default-production | high | active | partial | always |
| `memory-mark-command-commits-only-the-previewed-operation` | safety | default-production | medium | active | partial | always |
| `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` | safety | default-production | high | active | partial | always |
| `disposition-commit-log-row-carries-actor-and-cause-only` | safety | default-production | medium | active | partial | always |
| `admission-reason-is-redacted-bounded-and-non-blank` | safety | default-production | low | active | partial | always |
| `commit-actor-cause-and-source-kind-are-caller-asserted` | safety | default-production | high | active | partial | always |

Semantics distribution: 15 `always` (one invalidated), 0
`always-or-unreached`, 0 `sometimes`, 0 `reachable`, 0 `unreachable`.
Coverage markers (`sometimes`) that keep these from passing vacuously are
listed per record in [fault-map.md](fault-map.md); they are
preconditions, never the violation.

## Records

### disposition-admission-copies-the-subjects-prior-classes

Type: safety
Reachability: default-production - `lib.rs:14529` dispatch; the five
`DispositionEvent` variants parse from any plugin `kernel.commit` body.
Status: active
Exercised: partial - route tests land all five events under
`source_kind: "assistant"` on a route-written (`ModelInference`,
`AssistantInference`) subject and quarantine an `ExplicitUser` approval
(`kernel_routes.rs:4398`, `:5493`). PR A adds two route tests that read
the written row's `(source_class, taint_class)` after a dispute whose
reporter classes differ from the subject's and assert them equal to the
prior row's: `kernel_routes.rs:5723` (route-written subjects disputed
under `source_kind: "user"` and `"assistant"`) and `:6199` (an
`ExplicitUser`/`UserExplicit` approval disputed under `assistant`). The
other three events are not driven against a subject with differing
classes.
Guarantee: Every admission row a disposition operation writes carries the
subject's prior `source_class` and `taint_class`, never the caller's
derived pair.
Check: `always` - for each `DispositionResult` the route returns with
`commit_seq = c`, the `admission_decisions` row with `subject_object_id =
object_id AND commit_seq = c AND event_kind = event` has `source_class`
and `taint_class` equal to the latest prior row's, and the request the
route built had `source_class == prior.source_class && taint_class ==
prior.taint_class` (`commit.rs:725-726`). Every evaluated write must
satisfy it, so `always`.
Fault/timing angle: a subject whose prior classes differ from the caller's
derived classes (an `ExplicitUser`/`UserExplicit` approval, a
`TrustedLocalCode` observation-promoted decision) disputed by an
`assistant` or `user` source kind; a prior whose own row is inconsistent
(`effective_maturity > historical_maturity`) is refused instead
(`admission.rs:268-273`).
Required faults and enabling state: a live admitted decision in the bound
project whose prior `(source_class, taint_class)` is not the caller's
derived pair; a disposition request from a plugin `source_kind`.
Confidence: high - [evidence](evidence/disposition-admission-copies-the-subjects-prior-classes.md).
Read `disposition_request` (`commit.rs:694-736`), the class-equality
refusal (`admission.rs:268-273`), and `write_admission` column binding
(`admission.rs:1767-1768`).
Existing check: `crates/daemon/tests/kernel_routes.rs:4398`
(`disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`)
asserts disposition and outcome only; `kernel_routes.rs:5723` and `:6199`
assert the row's classes for disputes; `admission.rs:4912`
(`restrictive_disposition_and_origin_classification_are_sticky`) asserts
the class-mismatch refusal at the evaluator. Status unaudited.
Impact: if the caller's class were written, a model-filed dispute would
re-label a user-authored decision's lineage as model inference, and the
P4 reporter-class design (OQ4-1) would have no stable subject class to
distinguish itself from.
Open questions:
- The `commit.rs:7-10` header says every written row is "admitted under
  the derived classes"; the disposition row is admitted under the
  subject's. PR A's dispute section of the same header states the dispute
  case ("the row's own class columns keep the subject's prior classes",
  `commit.rs:40-41`); the general sentence is unchanged. Which wording is
  the contract for the other events? (needs human input)

### disposition-never-raises-maturity-and-withdraws-effective-support

Type: safety
Reachability: default-production - `lib.rs:14529`; every disposition event
reaches `evaluate_admission` through `record_admission`
(`admission.rs:982-987`).
Status: active
Exercised: partial - evaluator unit tests cover the withdrawal
(`admission.rs:5732`, `:5313`); kernel integration tests cover
`MarkStale` on a `CodeObserved` subject (`kernel_admission.rs:2735`).
PR A route tests read `maturity` and `effective_maturity` after a dispute:
`kernel_routes.rs:5723` (historical kept, effective `candidate`) and
`:6199` (an approval above Candidate keeps its historical maturity and
falls to effective `candidate`); the evaluator grid
`admission.rs:4532` covers both dispute events over every maturity pair.
No route test reads the maturity columns after `mark_stale`,
`explicit_reject`, or `quarantine`.
Guarantee: A disposition event never raises `maturity` or
`effective_maturity`, and a resulting non-Active disposition sets
`effective_maturity` to `candidate` while leaving `maturity` unchanged.
Check: `always` - for each disposition row written at `commit_seq = c`
with prior row `p`: `maturity == p.maturity`, `rank(effective_maturity) <=
rank(p.effective_maturity)`, and `disposition != 'active' implies
effective_maturity == 'candidate'`. Derived from `event_effect` returning
`current` for all five events (`admission.rs:4150-4163`), `historical =
current.max(target)` (`:337`), and the Candidate forcing (`:345-349`).
Holds on every write, so `always`.
Fault/timing angle: a subject at `Verified`, `Approved`, or `Enforced`
effective maturity with a valid supporting approval; a subject whose
stored approval chain is mid-revocation in the same envelope.
Required faults and enabling state: a live admitted decision above
`Candidate` effective maturity (needs a `CodeObserved` trigger or an
approval, both store-API only in tests) followed by any of the five events
from the route.
Confidence: high - [evidence](evidence/disposition-never-raises-maturity-and-withdraws-effective-support.md).
Read `evaluate_admission` lines `:264-388` and `event_can_raise_support`
(`:4189-4199`), which lists no disposition event.
Existing check: `admission.rs:5732`
(`non_active_dispositions_withdraw_effective_support`), `:5313`
(`an_event_that_names_a_disposition_keeps_its_own_outcome`),
`kernel_admission.rs:2735` (`dispositions_and_sensitivity_only_reduce_
visibility`). Status unaudited.
Impact: a disposition that raised maturity would let a plugin promote
content, violating index D3; one that left effective maturity in place
would keep a disputed `Verified` decision serving on automatic surfaces.
Open questions:
- The plan says "forces maturity to `Candidate`"; the column it forces is
  `effective_maturity`. Is the preserved `maturity` history intended to be
  visible to P3/P4 readers? (needs human input)

### disposition-approval-requirement-follows-the-restrictiveness-order

Type: safety
Reachability: default-production - `lib.rs:14529`; `approval_object_id` is
an optional request field (`commit.rs:155-159`).
Status: active
Exercised: partial - route tests cover Active to each of the five
(`kernel_routes.rs:4398`), Quarantined to Stale denied and then permitted
with a scoped approval (`:4458`), Stale to Stale admitted (`:4551-4565`);
evaluator tests cover Rejected/Contradicted/Quarantined to Stale/Disputed
(`admission.rs:5208`, `:5407`). PR A drives Stale to Disputed through the
route (`kernel_routes.rs:6146`) and Rejected, Contradicted, and
Quarantined to Disputed denied without approval (same test). Disputed to
Stale and Disputed to Contradicted are not driven by any test.
Guarantee: A disposition transition lands without approval exactly when
`disposition_restrictiveness(requested) >=
disposition_restrictiveness(prior)`; a strict decrease without a valid
approval is recorded as `Deny` with the disposition unchanged and
`denied: true`.
Check: `always` - for each disposition result `r` with prior `p`: if
`restrictiveness(requested(event)) >= restrictiveness(p.disposition)` then
`r.disposition == requested(event) && r.denied == false`; else if the
cited or stored approval is invalid then `r.disposition == p.disposition
&& r.outcome == "deny" && r.denied == true` and one new
`admission_decisions` row exists; else `r.disposition ==
requested(event)`. Table at `admission.rs:4168-4175`, rule at `:321-330`,
`denied` derivation at `commit.rs:762-782`. Every evaluation, so `always`.
Fault/timing angle: the approval's authority changing between preview and
commit; an approval quarantined earlier in the same envelope
(`kernel_routes.rs:5493`); lateral moves at equal rank.
Required faults and enabling state: subjects in each of the six non-Active
dispositions; an approval object both valid and invalid (revoked,
out-of-scope, non-`adr_accepted`); every ordered pair of events.
Confidence: high - [evidence](evidence/disposition-approval-requirement-follows-the-restrictiveness-order.md).
Read the restrictiveness table, `relaxes_disposition`, and both route and
evaluator tests.
Existing check: `kernel_routes.rs:4458`
(`a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one`),
`admission.rs:5208` (`leaving_a_restricted_disposition_requires_authority`),
`admission.rs:5407` (`relaxing_a_restrictive_disposition_requires_approval`).
Status unaudited.
Impact: a relaxation that landed without approval would let a plugin undo
a quarantine; a tightening that required approval would block the P4
report path entirely.
Open questions:
- R4 says "tightening only"; Stale to Disputed and Superseded to Disputed
  are lateral moves at equal rank (1) that land without approval, and the
  reverse moves do too. Is that the intended contract for a consumer-filed
  report on a stale or superseded finding? (needs human input; portfolio
  refinement R8)

### disposition-operation-carries-no-evidence-and-refuses-unknown-fields

Type: safety
Reachability: default-production - `lib.rs:14529`; strict parsing applies
to every body after envelope keys are stripped (`mod.rs:461-473`).
Status: invalidated
Exercised: not yet - the record was invalidated by PR A before a test
pinned it. The strict-parsing half still holds; no test sends an unknown
key on a disposition operation.
Guarantee: At `3a3b916b7`: a disposition operation body that carries any
key other than `op`, `object_id`, `event`, `approval_object_id` is refused
before any read or write, and every admission row the route writes for a
disposition has `evidence_id IS NULL` and `trigger_object_id IS NULL`.
Invalidated by PR A (#1160): the operation now accepts `evidence_id` and
`evidence_ref` (`commit.rs:160-167`), and a dispute row carries the cited
`evidence_id` (`commit.rs:731`). The replacing guarantees are
`dispute-requires-evidence-from-plugin` (only disputes carry evidence;
other events keep `evidence_id IS NULL` and the constant reason) and
`evidence-ref-well-formed-and-redaction-stable` in
[`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md).
Unknown keys other than the two new fields, and a caller-supplied
`reason` (`#[serde(skip)]`, `commit.rs:168-171`), are still refused by
`deny_unknown_fields` (`commit.rs:137`); `trigger_object_id` stays `None`
for every disposition (`commit.rs:729`).
Check: `always` - retained for history: a body whose disposition
operation holds a key outside `op`, `object_id`, `event`,
`approval_object_id`, `evidence_id`, `evidence_ref` answers the
`invalid_params` transport error with no change to `commit_log`,
`admission_decisions`, or `operation_receipts` counts, and every
route-written disposition row has `trigger_object_id IS NULL`. The
`evidence_id IS NULL` clause no longer holds for dispute rows.
Fault/timing angle: version skew: a client built for PR A sending
`evidence_ref` to a daemon older than PR A receives the transport error
`invalid_params`; the TypeScript client's state mapping decides what the
user sees.
Required faults and enabling state: a body with an unknown key; a body
with the new fields against an older daemon; a replay of a stored receipt
whose `result` predates `dispositions` (`commit.rs:1315-1351` tolerates
the missing key).
Confidence: high - [evidence](evidence/disposition-operation-carries-no-evidence-and-refuses-unknown-fields.md).
Read the `deny_unknown_fields` attributes, the new fields, and the
request builder at the PR A tip.
Existing check: `commit.rs:1315`
(`the_result_payload_round_trips_and_tolerates_an_unreadable_receipt`)
covers receipt shape only. No test sends an unknown key on a disposition
operation. Status unaudited.
Impact: none at the PR A tip beyond the retained unknown-key clause; the
record stays as the history of the baseline PR A changed.
Open questions:
- How does the TypeScript client map a `kernel.commit` `invalid_params`
  transport error for the `/eidnara-memory-mark` user: `refused` with
  which state key? Not read in this pass.

### admission-evidence-citation-is-existence-checked-only

Type: safety
Reachability: default-production - production writers that set
`evidence_id` on an `AdmissionEvent`: `crates/daemon/src/harness_sources.
rs:976` (`EventKind::Other`) is the only production writer; the
`memory_reviewer/lifecycle.rs:649` citation is inside `#[cfg(test)]`
(module at `:500`) and is an `ObservationSpec`, so it is not a writer of
this path (portfolio refinement R1). Re-labelled at the PR A tip: the
disposition route now sets `Some(evidence_id)` for a dispute
(`commit.rs:731`), but only after `require_visible_evidence` found the row
live and cited by the bound project in the same transaction
(`commit.rs:719-721`, `:742-759`). The route therefore never reaches the
accept-retired branch or the dangling-id FK branch; both remain reachable
from `harness_sources.rs:976` and from any store-API caller of
`record_admission` (`commit.rs:55-58`).
Status: active
Exercised: not yet - no test writes an admission row citing a retired
(`invalidated_commit_seq IS NOT NULL`) evidence row, and no test writes
one citing a nonexistent id and asserts the mapped refusal.
Guarantee: `write_admission` accepts any `evidence_id` that passes
`identity()` and names an existing `evidence_meta` row, live or retired;
a nonexistent id fails the FK and surfaces as `KernelError::Conflict`.
Check: `always` - for an `AdmissionRequest` with `evidence_id = Some(e)`:
if `e` has a secret-like substring then `InvalidInput`; else if no
`evidence_meta` row has `evidence_id = e` then the INSERT fails with
`SQLITE_CONSTRAINT` mapped to `Conflict` (`lib.rs:212-224`), which the
route would report as `invalid/already_exists` (`commit.rs:1065-1073`); else
the row is written with `evidence_id = e` whether or not
`invalidated_commit_seq IS NULL`. No liveness predicate exists between
`admission.rs:1690-1695` and the INSERT at `:1748-1786`. Holds on every
write, so `always`.
Fault/timing angle: an evidence row retired by the retention sweep between
the caller's read and the commit; a dangling id the FK rejects after the
subject's facts were loaded, inside one transaction.
Required faults and enabling state: a retired `evidence_meta` row; a never-
existing id; a Kernel API writer that passes `Some(evidence_id)`. The
route refuses both rows before the write (`not_found`,
`crates/daemon/tests/kernel_routes.rs:5801`), so a route test cannot
construct this record's enabling state.
Confidence: high - [evidence](evidence/admission-evidence-citation-is-existence-checked-only.md).
Read `write_admission`, the FK in `schema.rs:163`, `foreign_keys=ON`
(`schema.rs:244`, `open.rs:1035`), and `map_sqlite`.
Existing check: none for liveness. `kernel_admission.rs:4211`
(`a_trigger_whose_evidence_is_gone_cannot_admit`) covers the trigger
query (`validate_trigger`, `admission.rs:2731`), a different path. Status
unaudited.
Impact: PR A places the liveness check in the daemon (KTD2), so any other
writer can still record a dispute citing retired evidence, and a reader of
a dispute row cannot tell from the Kernel alone whether the citation was
checked; the `v=1` reason prefix marks the rows the route checked
(`commit.rs:55-58`).
Open questions:
- Should the Kernel refuse a retired `evidence_id` on `write_admission`,
  or is the daemon the only intended gate (KTD2)? (needs human input)
- Resolved by PR A: the daemon check answers `not_found` for a dangling
  or retired id (`commit.rs:48-49`, `:747-750`), before the Kernel's FK
  path could answer `already_exists`.

### slice-write-refuses-non-live-evidence-citations

Type: safety
Reachability: default-production - `kernel.commit` `insert_decision` and
`insert_observation` accept `evidence_id` (`commit.rs:274`, `:314`) and
reach `require_parents` (`write.rs:1095-1108`); decision events reach
`require_live_evidence` (`write.rs:1151-1178`).
Status: active
Exercised: partial - `kernel_admission.rs:4211` covers a trigger whose
evidence is gone; no route test inserts a decision citing a retired
evidence id and asserts `not_found` with no write.
Guarantee: A slice write that cites an `evidence_id` succeeds only if that
row has `invalidated_commit_seq IS NULL` at commit time; otherwise the
write returns `NotFound` and nothing in the envelope lands.
Check: `always` - for every `insert_decision` and `insert_observation`
call (route-reachable through `require_parents`) and every
`append_decision_citations` call (`write.rs:240`, `pub(crate)`, reached
only from `review_acceptance.rs:1088`, not from `kernel_routes`;
portfolio refinement R2) with cited ids `E`: the call returns `Ok`
only if `COUNT(*) FROM evidence_meta WHERE evidence_id IN E AND
invalidated_commit_seq IS NULL == |E|`; otherwise `Err(NotFound)` and the
transaction rolls back (`require_live`, `write.rs:1138-1149`;
`require_live_evidence`, `:1151-1178`). Every write, so `always`.
Fault/timing angle: an evidence row retired by the `harness_capture` or
`curator_capture` sweep between the client's read and the commit; a
citation of an id that never existed (same `NotFound`).
Required faults and enabling state: a retired evidence row in the same
store; a route commit citing it on `insert_decision`.
Confidence: high - [evidence](evidence/slice-write-refuses-non-live-evidence-citations.md).
Read the three helpers and `require_parents`.
Existing check: `kernel_admission.rs:4211`
(`a_trigger_whose_evidence_is_gone_cannot_admit`) for the trigger path;
none found for `require_parents` on a route insert. Status unaudited.
Impact: its semantics (retired is `NotFound`, mapped to
`invalid/not_found`) are the model the PR A dispute check follows.
Open questions:
- Resolved by PR A: the daemon's dispute check calls neither helper. It
  calls the new Kernel read `Envelope::evidence_citations`
  (`crates/kernel/src/envelope.rs:532-577`) inside the writer transaction
  and answers `not_found` for a retired row, the same name this record's
  helpers give (`cited-evidence-live-at-dispute-commit` in
  [`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md)).
- Resolved (portfolio gap G4): `append_decision_citations` has one caller,
  `admit` in `review_acceptance.rs:1088`, which deduplicates its support
  list first (`review_acceptance.rs:628-629`); the inventory is in the
  evidence file. The callers were read, not changed.

### evidence-objects-carry-no-scope-and-fail-scoped-lookup

Type: safety
Reachability: default-production - every route path that resolves an
object id goes through `scoped_object_state` (`commit.rs:676-691`):
subjects, approvals, chain members, tokens, replaced ids, projected
dependencies.
Status: active
Exercised: not yet - no test names an evidence object id as a disposition
subject, approval, token, or dependency and asserts `not_found`.
Guarantee: An evidence object's `ObjectState.scope_id` is `None`, so
`scoped_object_state` answers `NotFound` for every evidence object id
under every project binding.
Check: `always` - for every `evidence_meta` row `e` and its
`object_registry` row `o`: `load_object_state(o.object_id).scope_id ==
None` (`object_state_sql`, `envelope.rs:1881`), `ScopeFilter::matches(None,
..) == Ok(false)` (`project.rs:199-206`), and `scoped_object_state(..,
o.object_id)` returns `Err(NotFound)`. Every evidence row, so `always`.
Observation split (portfolio refinement R3): the route-level test observes
`invalid(not_found)` for a request naming an evidence object id in a scoped
position; the `scope_id == None` clause is a kernel-crate assertion, since
`load_object_state` is `pub(super)` (`envelope.rs:1905`).
Fault/timing angle: none in time; the gap is structural.
Required faults and enabling state: a live evidence row ingested through
`kernel.artifact.ingest.*` or the store API; a route request naming its
`object_id` in any scoped position.
Confidence: high - [evidence](evidence/evidence-objects-carry-no-scope-and-fail-scoped-lookup.md).
Read `object_state_sql`, the doc comment at `envelope.rs:1903-1904`, and
`ScopeFilter::matches`. Tables with `scope_id`: `asserted_edges`,
`decisions`, `observations`, `propositions`, `scope_term`, `scopes`
(`schema.rs`).
Existing check: none found.
Impact: an evidence object id cannot be a disposition subject, approval,
token, or projected dependency under any binding. Re-labelled at the PR A
tip: the guarantee still holds, and dispute evidence no longer depends on
it. The route judges a cited `evidence_id` through its live citers
(`require_visible_evidence`, `commit.rs:742-759`, over
`Envelope::evidence_citations`, `envelope.rs:532-577`), which is
`cited-evidence-visible-to-bound-project` in
[`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md).
Open questions:
- Resolved by PR A: "visible to the caller's project scope" means cited by
  a live decision, observation, decision event (through its live
  decision), or asserted edge whose scope the bound project's filter
  accepts (`commit.rs:17-22`). Evidence cited only by unscoped rows is not
  citable; that sub-case stays open in the dispute-evidence record.

### disposition-subject-is-a-live-admitted-decision-in-the-bound-project

Type: safety
Reachability: default-production - `lib.rs:14529`; `disposition_request`
runs for every disposition in both preview and commit.
Status: active
Exercised: partial - route tests cover retired (`not_found`), unadmitted
(`admission_policy`), missing (`not_found`), foreign-project approval
chains of three shapes (`kernel_routes.rs:4888`, `:4982`, `:4568`,
`:4635`); no test targets a live, admitted object of kind `observation`
in scope, nor a subject whose stored approval (none cited) is out of
scope.
Guarantee: A disposition is judged only against an object that is live,
of kind `decision`, scoped to the bound project, and has a prior
admission; every approval it rests on, cited or stored, and every chain
member, is in scope; otherwise the request is refused before any row is
written.
Check: `always` - for each disposition operation: `scoped_object_state`
succeeds, `object_kind == "decision"`, `invalidated_commit_seq IS NULL`,
`subject_admission` is `Some`, and `scoped_object_state` succeeds for
`approval_object_id.or(stored_approval)` and each
`approval_chain_members` entry (`commit.rs:694-716`); any failure maps to
`invalid/not_found` or `invalid/admission_policy` with
`admission_decisions` and `commit_log` counts unchanged. Every request,
so `always`.
Fault/timing angle: an object retired by a concurrent commit between the
client's read and this commit (the writer lock serializes, so the check
sees the retirement); a chain above `MAX_AUTHORITY_CHAIN_DEPTH`.
Required faults and enabling state: subjects in each refused state; an
in-scope live observation object; a stored approval retired or moved out
of scope after the subject's last admission.
Confidence: high - [evidence](evidence/disposition-subject-is-a-live-admitted-decision-in-the-bound-project.md).
Read `disposition_request` and `scoped_object_state`.
Existing check: `kernel_routes.rs:4888`
(`a_disposition_needs_a_live_admitted_decision`), `:4982` (missing id),
`:4568`/`:4635` (`disposition_approval_chains_of_every_kind_are_project_
scoped`). Status unaudited.
Impact: a disposition landing on a foreign project's decision, or on an
observation, would be a cross-project write or an undefined admission
subject.
Open questions:
- The outcome when the subject's stored approval has been retired or moved
  out of scope after its last admission was not traced end to end; the
  evidence file records it as untraced (portfolio refinement R4).

### disposition-preview-writes-nothing-and-reports-the-commit-verdict

Type: safety
Reachability: default-production - `preview: true` is a request field
(`commit.rs:126`); the TypeScript command always previews first
(`memory-mark-command.ts:237`).
Status: active
Exercised: partial - route tests cover write-nothing, per-surface
visibility, in-request ordering, denials and refusals, and the changed-
authority refusal (`kernel_routes.rs:5120`, `:5277`, `:5355`, `:5493`);
no test runs preview and commit from the same store snapshot across all
five events and all seven prior dispositions and diffs the verdicts.
Guarantee: A preview leaves `commit_log`, `admission_decisions`, and
`operation_receipts` unchanged and, for the same store state, reports the
`outcome`, `previous_disposition`, `disposition`, and `denied` the commit
then records.
Check: `always` - after any preview request the three row counts equal
their values before it; and for a preview `P` and a commit `C` of the same
operations against the same tip with no intervening commit, each
`P.previews[i]` restricted to `DispositionResult` fields equals
`C.dispositions[i]`. Preview and commit share `disposition_request` and
`prepare_admission` (`commit.rs:827-865`; `admission.rs:1859-1870`).
Every request, so `always`.
Fault/timing angle: a commit landing between preview and commit changes
the prior; an earlier operation in the same preview changing an approval
chain is refused as malformed (`commit.rs:1174-1180`).
Required faults and enabling state: subjects in each disposition; a
preview with a recorded identity (replays, judges nothing).
Confidence: high - [evidence](evidence/disposition-preview-writes-nothing-and-reports-the-commit-verdict.md).
Existing check: the four preview tests above. Status unaudited.
Impact: a preview that wrote would charge the identity before the user
confirmed; a preview that disagreed with the commit would ask the wrong
confirmation question.
Open questions: None.

### disposition-envelope-identity-replays-without-a-second-write

Type: safety
Reachability: default-production - every `kernel.commit` carries an intent
(`project.rs:138-146`); the command derives `operationId = sessionId SEP
event SEP objectId`, followed at the PR A tip by the evidence flags
(`memoryMarkOperationId`, `memory-mark-command.ts:201-206`), and both
command callers pass it in (`command-handler.ts:415`,
`pi-plugin/src/commands/eidnara-memory-mark.ts:61`).
Status: active
Exercised: partial - `kernel_routes.rs:4982` covers replay, reused key,
and token abort; TypeScript tests cover replay through the command
(`memory-mark-command.test.ts:348`, `:359`). PR A adds a route test of
two dispute identities, replay, and reuse
(`dispute_identities_land_replay_and_refuse_reuse_like_every_envelope`,
`kernel_routes.rs:6080`) and a command test of two evidence keys in one
session (`memory-mark-command.test.ts:208`). No test replays after the
subject moved and asserts the replayed `dispositions` still equal the
original while the stored disposition differs.
Guarantee: For one `(producer, operation_key)`: an equal `request_digest`
replays the stored receipt with `replayed: true` and the original
`dispositions`, writing nothing; a different digest is refused as
`operation_key_reused`, writing nothing; at most one `commit_log` row and
one set of admission rows ever exist for the key.
Check: `always` - per identity `k`: `COUNT(commit_log WHERE producer,
operation_key = k) <= 1`; a second request with equal digest returns
`receipt.replayed == true`, `receipt.commit_seq` equal to the first, and
`dispositions` byte-equal to the first (`stored_receipt`,
`envelope.rs:1342-1372`; `commit.rs:1190-1206`); a request with a
different digest returns `invalid/operation_key_reused` (`commit.rs:1065-
1070`). Every request, so `always`.
Fault/timing angle: a lost response (`outcome_unknown`) followed by the
client's single reissue under the same identity
(`client.ts:300-301`, `:462`); a replay whose original `result` predates the
`dispositions` key.
Required faults and enabling state: a committed disposition; a repeat with
equal digest; a repeat with a different digest; a reissue after a dropped
response.
Confidence: high - [evidence](evidence/disposition-envelope-identity-replays-without-a-second-write.md).
Existing check: `kernel_routes.rs:4982`
(`disposition_operations_share_the_envelope_replay_and_token_rules`),
`kernel_routes.rs:931`, `:6080`, `memory-mark-command.test.ts:348`,
`:359`, `:208`. Status unaudited.
Impact: a second write under one identity would double-record a dispute
and break the one-row-per-report assumption P3's join by `commit_seq`
relies on.
Open questions:
- A repeated command after the object moved reports `applied` with the
  original verdict while the stored disposition differs
  (`memory-mark-command.test.ts:359`; `commit.rs:1190-1206`). Safe for the
  store; is the replayed verdict the right thing to show the user? (needs
  human input; portfolio refinement R5)

### memory-mark-command-commits-only-the-previewed-operation

Type: safety
Reachability: default-production - command registered in both plugins
(`pi-plugin/src/index.ts:550`; `command-handler.ts:413`).
Status: active
Exercised: partial - `memory-mark-command.test.ts:267-522` covers
confirmation, decline, denial, replay, cancellation, separator refusal,
refused preview, and mismatched verdicts. PR A's
`memory-mark-command.test.ts:181` asserts that both calls (preview and
commit) carry the same disposition operation with its evidence fields,
`source_kind: "user"`, and the same `cause`; it matches each body against
one expected object rather than comparing the two bodies, and it does not
assert the whole intent equal.
Guarantee: The command sends exactly one disposition operation, identical
in preview and commit, under `sourceKind: "user"` and the same intent;
asks confirmation at most once and only when `visibility_changes &&
!confirmed`; commits nothing after a denied preview, a decline, or
cancellation; and reports `outcome_unknown` when the commit's verdict list
does not answer the sent operation.
Check: `always` - for one invocation: `previewBody.operations ==
commitBody.operations == [{op:"disposition", object_id, event,
evidence_id?, evidence_ref?}]` (`memory-mark-command.ts:221-227`),
`previewBody.intent == commitBody.intent`, `source_kind == "user"`;
`confirm` called at most once; `commit` not called when the preview
verdict is `denied`, when `confirm` returns false, or when `isCancelled()`
is true; `verdictFor` returns null whenever `results.length != 1 ||
object_id != operation.object_id || event != operation.event`
(`memory-mark-command.ts:209-217`), and a null verdict maps to
`refused(preview, invalid("internal"))` on the preview branch
(`:243-245`) and to `refused(commit, unavailable("outcome_unknown"))` on
the commit branch (`:257-259`) (portfolio refinement R6). Every
invocation, so `always`. Coverage marker to add:
`command_preview_visibility_changes` (the confirmation branch is reached
only when `visibility_changes` is true, so the at-most-once clause is
vacuous without it).
Fault/timing angle: the daemon answering a verdict for another object; a
replayed preview (`receipt` present) skipping confirmation; the object
moving between preview and commit so the commit verdict differs.
Required faults and enabling state: a `FakeKernel` or stub returning
mismatched, empty, or multiple verdicts; a recorded identity; a
`visibility_changes: true` preview.
Confidence: high - [evidence](evidence/memory-mark-command-commits-only-the-previewed-operation.md).
Existing check: `memory-mark-command.test.ts:268`, `:291`, `:312`, `:331`,
`:348`, `:359`, `:372`, `:390`, `:421`, `:437`, `:451`, and PR A's `:181`,
`:208`, `:249`. Status unaudited.
Impact: committing an operation the user did not preview, or treating a
foreign verdict as the answer, would apply a disposition the confirmation
dialog never described.
Open questions: None.

### disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit

Type: safety
Reachability: default-production - the route accepts any live admitted
decision in scope as a subject, including an `adr_accepted` approval
(`kernel_routes.rs:5493` quarantines `approval-1` through the route).
Status: active
Exercised: partial - `kernel_admission.rs:1503` covers `Quarantine` on an
approval through the store API and asserts dependents demote;
`kernel_routes.rs:5493` quarantines an approval through the route and
observes a later relaxation denied. PR A's
`a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents`
(`kernel_routes.rs:6199`) files `mark_disputed` with evidence on an
`ExplicitUser` approval through the route under `source_kind:
"assistant"` and reads the dependent's row: `approval_revoked` at the
dispute's `commit_seq`, with the dispute's reason; it does not read the
dependent's `effective_maturity` after the demotion. No test files
`mark_stale` on an approval through the route. The `deferred` branch (dependents past `MAX_AUTHORITY_DEMOTIONS`,
4,096) is not yet exercised and needs that many dependents to construct
(portfolio refinement R7).
Guarantee: When the subject grants authority before the write and the
disposition leaves it non-Active, the subject grants nothing afterwards,
and every dependent whose latest decision cites it, up to
`MAX_AUTHORITY_DEMOTIONS`, receives an `approval_revoked` row in the same
`commit_seq` with `effective_maturity` at or below its automatic ceiling.
Check: `always` - for a disposition at `commit_seq = c` on subject `s`
with `subject_grants_authority(s) == true` before the write: afterwards
`validate_approval(s) == false`, and for every object `d` whose latest
prior row cited `s` as `approval_object_id`, either a row
`(subject_object_id = d, event_kind = 'approval_revoked', commit_seq = c)`
exists with `rank(effective_maturity) <=
rank(automatic_ceiling(d.source_class, d.taint_class))`, or `d` is counted
in the walk's `deferred` total once `MAX_AUTHORITY_DEMOTIONS` (4,096) rows
have been written (`with_authority_cascade`, `admission.rs:1060-1082`;
`demote_authority_dependents`, `:1452-1490`; `approval_row_fields`
requires `disposition='active'`, `:850`). Every qualifying write, so
`always`.
Fault/timing angle: the cascade runs after the subject's row is written,
inside one transaction; `MAX_APPROVAL_DEPENDENTS` (1,024) bounds grants
per approval and `MAX_AUTHORITY_DEMOTIONS` (4,096) bounds demotions per
walk, with the remainder deferred (`:665-670`).
Required faults and enabling state: an `adr_accepted` decision admitted
under `ExplicitUser`/`UserExplicit` with `Approved` maturity, in scope; at
least one dependent promoted by citing it; a `mark_disputed` from a
plugin `source_kind`.
Confidence: high - [evidence](evidence/disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit.md).
Existing check: `kernel_admission.rs:1503`
(`quarantining_an_approval_demotes_its_dependents_and_ends_its_authority`),
`kernel_routes.rs:5493`, `:6199`. Status unaudited.
Impact: a model-filed `mark_disputed` on a user's approval object
withdraws authority from everything it supported. Index D3 covers
raising authority; withdrawal by a model-class caller is implemented and
unflagged in the plan.
Open questions:
- Is a plugin-filed dispute of an `adr_accepted` approval, and its
  cascade, intended behavior for P4's "the report adds data, not
  authority"? The PR A route header documents the cascade as behavior
  (`commit.rs:60-62`); no owner decision beyond that is recorded. (needs
  human input)
- Portfolio gap G2: the preview's `visibility_changes` reports the
  subject only (`PreviewRun::disposition`, `commit.rs:827-866`; `changes_to`
  at `:537-542`) and excludes the dependents the commit demotes in the same
  `commit_seq`, so a user confirming a dispute on an approval is not shown
  the cascade. Resolved per OQ4-10 by documenting the limit: the route
  header (`commit.rs:60-63`) and the command usage
  (`memory-mark-command.ts:54`, asserted by `memory-mark-command.test.ts:163`)
  state that the preview reports the named memory's visibility only. The
  preview still does not report the cascade.
- How does a deferred dependent (past `MAX_AUTHORITY_DEMOTIONS`) get its
  visibility repaired? The code comment at `admission.rs:1445-1451` calls
  chunking "the remaining work". (partial: validity is already withdrawn
  by `validate_approval`; the served row is not)

### disposition-commit-log-row-carries-actor-and-cause-only

Type: safety
Reachability: default-production - every non-replayed `kernel.commit`
inserts one `commit_log` row (`envelope.rs:1400-1415`).
Status: active
Exercised: partial - PR A's `kernel_routes.rs:5723` reads the `commit_log`
row of two dispute commits and asserts `actor` and `cause` equal the
request's (`user:opencode` with a command-shaped cause, `agent:opencode`
with a tool-shaped cause). No test asserts the column set or a redacted
`cause`.
Guarantee: The `commit_log` row a disposition commit writes carries the
request's `actor` and `cause` after redaction and nothing identifying the
session or `source_kind`.
Check: `always` - for the receipt's `commit_seq = c`: the `commit_log` row
at `c` has `actor == redact(intent.actor).text`, `cause ==
redact(intent.cause).text`, `producer == intent.producer`, and the table's
column set is exactly `{commit_seq, transaction_id, writer_epoch,
producer, operation_key, request_digest, recorded_at, actor, cause}`
(`schema.rs:26`); the `admission_decisions` row at `c` joins to it by
`commit_seq` only. Every commit, so `always`.
Fault/timing angle: a secret-like substring in `actor` or `cause` is
replaced by the redactor before the INSERT (`RedactedIntent`), so the
stored text can differ from the sent text.
Required faults and enabling state: a disposition commit from the command
(`actor = user:opencode | user:pi`; `cause` is the command line
`commandLine(args)`, `/eidnara-memory-mark <event> <object_id>` followed
at the PR A tip by `--evidence <id>` and `--path <path>@<oid>` when
present, `memory-mark-command.ts:158-160`, `:230-236`); a `cause` carrying
a secret-like token.
Confidence: high - [evidence](evidence/disposition-commit-log-row-carries-actor-and-cause-only.md).
Read the schema and the INSERT.
Existing check: `kernel_routes.rs:5723` for `actor` and `cause`. Status
unaudited.
Impact: P4 R3 states the event row joins to a commit-log entry carrying
`session_id` and `source_kind`; at the PR A tip it carries neither, so the
reporter identity is `actor` and `cause` text plus the reporter taint in
the dispute's `v=1` reason until PR B adds typed columns. This record
stays a pin of the commit-log shape until PR B
([#1162](https://github.com/ahrav/eidnara/issues/1162)) changes it.
Open questions:
- Is the PR A reporter identity meant to be recoverable from `actor`,
  `cause`, and the reason's `reporter_taint` alone? PR A answers by
  implementation (the reason carries the derived taint; `actor` and
  `cause` are caller-asserted, see
  `commit-actor-cause-and-source-kind-are-caller-asserted`). (needs human
  input)

### admission-reason-is-redacted-bounded-and-non-blank

Type: safety
Reachability: default-production - every admission row passes
`write_admission` (`admission.rs:1675`); the route's reason is the
constant `"kernel.commit"` (`commit.rs:100`) for every event except the two
disputes, which carry the `v=1` structured reason the route composes and
caps at 4096 bytes (`commit.rs:221-227`;
`crates/daemon/src/kernel_routes/dispute.rs:14`, `:77-79`).
Status: active
Exercised: partial - `kernel_admission.rs:1676` covers a revocation reason
surviving; no test drives a reason at or above 512 KiB, a blank reason
through the route (unreachable: the constant is non-blank), or a reason
with a secret-like substring and asserts the stored text and the
redaction row.
Guarantee: The stored `reason` is `redact(reason).text`; a reason longer
than 512 KiB is refused with `InvalidInput`; a blank reason is refused with
`AdmissionPolicy`; a redaction is recorded beside the row.
Check: `always` - for every `AdmissionRequest`: `reason.trim().is_empty()
implies Err(AdmissionPolicy)` (`admission.rs:1565-1567`);
`reason.len() > MAX_REDACTABLE_BYTES implies Err(InvalidInput)`
(`redaction.rs:72-77`); otherwise the row's `reason` equals
`redact_lossy(reason).text` and a `durable_text_redactions` row exists for
each detection (`record(.., "reason", ..)`, `admission.rs:1787-1794`). Every
write, so `always`.
Fault/timing angle: none in time; the bound is per request.
Required faults and enabling state: a store-API caller with a long or
secret-bearing reason. The route cannot produce either: it refuses a
dispute reason over 4096 bytes or holding a detection with
`invalid_input` before the Kernel sees it (`dispute.rs:77-79`).
Confidence: medium - [evidence](evidence/admission-reason-is-redacted-bounded-and-non-blank.md).
The bound and the blank check are read in code; the redaction-row write
was read but its message and column mapping were not traced into
`record`.
Existing check: `kernel_admission.rs:1676`
(`a_revocation_reason_survives_without_dependents`). Status unaudited.
Impact: PR A carries the reporter taint and `evidence_ref` in `reason`
(`structured-reason-projects-typed-reporter-data` in
[`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md)); a
reason the redactor rewrote would not parse back. The route's 4096-byte
cap and detection refusal keep route-written reasons inside this record's
pass-through branch.
Open questions:
- Resolved (portfolio gap G3): `redact_lossy` calls the same
  `redact_durable_text` the route scans with
  (`crates/kernel/src/redaction.rs:41-42`). The scanner detects the test
  token `sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678`
  (`dispute.rs:342-349`), and 64 seeded 40- and 64-hex object ids beside
  an ordinary repository path carry no detection (`dispute.rs:354-367`).
  The evidence file records which rule the token matches and that the
  attribution is inferred from the rule regexes.

### commit-actor-cause-and-source-kind-are-caller-asserted

Type: safety
Reachability: default-production - every `kernel.commit` body carries an
intent (`IntentRequest`, `crates/daemon/src/kernel_routes/project.rs:138-146`)
and a `source_kind` (`CommitRequest`, `commit.rs:108-127`); `lib.rs:14529`
dispatches the route. Portfolio gap G1.
Status: active
Exercised: partial - PR A's `kernel_routes.rs:5723` sends `actor`
`user:opencode` and `agent:opencode` with free-text causes, and
`source_kind` `user` and `assistant`, on a route bound under harness
`test` (`route_identity(&project, "test", "session-a")`,
`kernel_routes.rs:70`), and asserts the `commit_log` row holds the sent
`actor` and `cause` and the reason holds the taint derived from the sent
`source_kind`. Not constructed: a `producer` that names another plugin,
and a redacted `actor` or `cause`.
Guarantee: The route stores `producer`, `actor`, and `cause` as the caller
sends them, after the Kernel's redaction, and derives the admission and
reporter classes from the caller's `source_kind` bounded by
`derive_classes`; none of the four is compared with the route binding's
harness, and the binding contributes the project scope, the receipt
namespace, and the session check only.
Check: `always` - for every non-replayed commit at `commit_seq = c` with
intent `I` on binding `B`: the `commit_log` row at `c` has `actor =
redact(I.actor)`, `cause = redact(I.cause)`, `producer = I.producer`
whatever `B.harness` is; the dispute reason's `reporter_taint` is a
function of `(source_kind, asserted_taint_class)` alone
(`commit.rs:1111-1125`); the request's `session_id` equals the channel's
bound session or the request is refused `session_mismatch` before parsing
(`management_binding`, `crates/daemon/src/lib.rs:6596-6631`). Holds on
every request, so `always`.
Fault/timing angle: none in time; the fault is a client that claims
another surface.
Required faults and enabling state: a request whose `actor` names a
harness other than the binding's; `source_kind: "user"` from a model
tool; a `producer` naming another plugin.
Confidence: high - [evidence](evidence/commit-actor-cause-and-source-kind-are-caller-asserted.md).
Read `IntentRequest::into_intent` (`project.rs:148-165`),
`kernel_route_scope` (`mod.rs:415-441`), the envelope keys stripped before
body parsing (`ENVELOPE_KEYS`, `mod.rs:461`), and every use of the bound
scope in `handle_kernel_commit` (`commit.rs:1126`, `:1141`, `:1144`).
Existing check: `kernel_routes.rs:5723` for `actor` and `cause` stored as
sent on a `test` binding. Status unaudited.
Impact: a model-driven client can file a dispute with `actor:
"user:opencode"` and `source_kind: "user"`; the stored record names a
user reporter with `reporter_taint=user_inferred`. The derivation caps the
claim below `user_explicit`, so the claim cannot raise standing, but a
reader cannot tell the command arm from a client asserting it.
Open questions:
- Should the route bind `actor` to `RouteScope.harness`
  (`mod.rs:405-412`) and `source_kind` to the calling surface? (needs
  human input)
- The bound session is checked and then discarded; no Kernel row records
  it. PR B ([#1162](https://github.com/ahrav/eidnara/issues/1162)) adds
  the session link for disputes.

## Relationship map

Shared mechanisms:

- `disposition_request` (`commit.rs:694-736`) is the single builder for
  preview and commit; records `disposition-admission-copies-the-subjects-
  prior-classes`, `disposition-subject-is-a-live-admitted-decision-in-the-
  bound-project`, and `disposition-preview-writes-nothing-and-reports-
  the-commit-verdict` all read it, and so do the dispute-evidence records
  for liveness and visibility.
- `evaluate_admission` (`admission.rs:264-388`) decides records
  `disposition-never-raises-maturity-and-withdraws-effective-support` and
  `disposition-approval-requirement-follows-the-restrictiveness-order`;
  both are pure-function checks over `EvaluationInputs`.
- `write_admission` (`admission.rs:1675-1853`) is shared by
  `admission-evidence-citation-is-existence-checked-only`,
  `admission-reason-is-redacted-bounded-and-non-blank`, and the column
  assertions in the first two records.
- `scoped_object_state` and `ScopeFilter` are shared by
  `evidence-objects-carry-no-scope-and-fail-scoped-lookup` and
  `disposition-subject-is-a-live-admitted-decision-in-the-bound-project`.
- `stored_receipt` and `commit_prepared_with_writer` are shared by
  `disposition-envelope-identity-replays-without-a-second-write`,
  `disposition-preview-writes-nothing-and-reports-the-commit-verdict`
  (replay branch), and `disposition-commit-log-row-carries-actor-and-
  cause-only`.
- Liveness semantics: `slice-write-refuses-non-live-evidence-citations`
  defines the predicate; `admission-evidence-citation-is-existence-
  checked-only` records its absence on the Kernel admission path; the PR A
  route check for disputes is in the dispute-evidence part.
- `IntentRequest` and `resolve_classes` carry the caller's claims into
  `disposition-commit-log-row-carries-actor-and-cause-only` and
  `commit-actor-cause-and-source-kind-are-caller-asserted`; the second
  records that the route compares none of them with the binding.
- `disposition-operation-carries-no-evidence-and-refuses-unknown-fields`
  is invalidated; `dispute-requires-evidence-from-plugin` and
  `evidence-ref-well-formed-and-redaction-stable` in
  [`../dispute-evidence/catalog.md`](../dispute-evidence/catalog.md)
  replace it.

Suspected dominance (not proven):

- If `disposition-preview-writes-nothing-and-reports-the-commit-verdict`
  holds, the preview branch of `memory-mark-command-commits-only-the-
  previewed-operation` cannot ask a wrong confirmation question; the
  command record still needs its own oracle for the commit branch.
- If `disposition-admission-copies-the-subjects-prior-classes` holds, the
  class-equality refusal in `evaluate_admission` is unreachable from the
  route; a `sometimes` marker on that refusal would then never fire from
  route traffic and must come from the store API.
- If `disposition-approval-requirement-follows-the-restrictiveness-order`
  holds, R4's "never relaxes" follows for every disposition without a
  valid approval; it does not follow for a caller holding one.
- `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-
  commit` depends on `disposition-never-raises-maturity-and-withdraws-
  effective-support` for the bearer's own row and adds the cascade.

## Handoff list

| Slug | Handoff | Why |
|---|---|---|
| `disposition-admission-copies-the-subjects-prior-classes` | `/testing:test-strategy` (route integration test reading the row for the three non-dispute events) | disputes covered by `kernel_routes.rs:5723`, `:6199` |
| `disposition-never-raises-maturity-and-withdraws-effective-support` | `/testing:test-strategy` (evaluator property test over all events and priors) | pure function; `proptest` over `EvaluationInputs` fits |
| `disposition-approval-requirement-follows-the-restrictiveness-order` | `/testing:test-strategy` (evaluator property test plus route cases for lateral moves) | pure table; route coverage for Disputed to Stale and Disputed to Contradicted missing (Stale to Disputed covered by `kernel_routes.rs:6146`) |
| `disposition-operation-carries-no-evidence-and-refuses-unknown-fields` | none (invalidated by PR A) | replaced by dispute-evidence records |
| `admission-evidence-citation-is-existence-checked-only` | `/testing:test-strategy` (kernel integration test) then `/low-level-systems:defensive-assertions-and-invariant-guards` | decides where the liveness guard belongs |
| `slice-write-refuses-non-live-evidence-citations` | `/testing:test-strategy` (route test with a retired evidence row) | primitive P4 reuses |
| `evidence-objects-carry-no-scope-and-fail-scoped-lookup` | `/testing:test-strategy` (route test naming an evidence object id) and human decision on R2 | structural gap |
| `disposition-subject-is-a-live-admitted-decision-in-the-bound-project` | `/testing:invariant-test-review` for `:4888`, `:4635`; `/testing:test-strategy` for the observation-subject case | mostly covered |
| `disposition-preview-writes-nothing-and-reports-the-commit-verdict` | `/testing:test-strategy` (route-level grid test over subject states, preview then commit; portfolio refinement R10) | differential oracle across states, no simulator needed |
| `disposition-envelope-identity-replays-without-a-second-write` | `/testing:invariant-test-review` for `:4982`; `/testing:stateful-workload-design` for replay-after-move | per-identity accounting |
| `memory-mark-command-commits-only-the-previewed-operation` | `/testing:invariant-test-review` for the TS suite; `/testing:test-strategy` for body-equality assertion | FakeKernel exists |
| `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit` | human decision first, then `/testing:test-strategy` (route test on an approval subject) | intended-behavior question |
| `disposition-commit-log-row-carries-actor-and-cause-only` | `/testing:test-strategy` (column-set and redacted-cause assertions) | pin until PR B |
| `commit-actor-cause-and-source-kind-are-caller-asserted` | human decision on binding `actor` and `source_kind`, then `/testing:test-strategy` | trust-boundary question (G1) |
| `admission-reason-is-redacted-bounded-and-non-blank` | `/testing:test-strategy` (kernel unit tests at the bound and with a detection) | low priority until OQ4-1 lands |
