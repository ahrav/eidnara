# System model: kernel.commit disposition path at HEAD

System: `/local/home/ahrav/scratch/eidnara`, HEAD `3a3b916b7`
(2026-10-25). Date: 2026-10-25. Method: `docs/properties/METHOD.md` and
`/testing:property-discovery-and-catalog`.

Reference note (PR A, #1160): this document describes the code at
`3a3b916b7` and is kept as the design-time record. Its `file:line`
references were remapped to the PR A branch tip (base `6159fd224`, code
commit `6c4d445e3`) through the git diff between the two revisions; where
PR A changed the cited code, the text says so inline with "PR A:". The
active records in [`dispute-evidence/catalog.md`](../dispute-evidence/catalog.md)
and [`disposition-path/catalog.md`](catalog.md) state the PR A
behavior.

Scope: the implemented `kernel.commit` `disposition` operation, end to end,
as it exists at HEAD. Proposed P4 behavior is cataloged by a different
owner; it appears here only as claims under test and as contract-versus-code
leads.

External-evidence scope, user-confirmed and recorded verbatim: "the plan set
(00-index, P3, P4 plans) plus repository code, tests,
docs/host-wire-protocol.md, docs/memory-reviewer-operations.md, and
docs/properties at HEAD. No tracker issues, no incident history, no git
history lens."

Line references were read at HEAD. Where [`../fact-sheet.md`](../fact-sheet.md)
or the plan cites a line that differs at HEAD, the HEAD line is given and
the difference noted.

## Lens: architecture and data flow

- Entry: the daemon's method match dispatches `"kernel.commit"` to
  `HandlerCore::handle_kernel_commit` at `crates/daemon/src/lib.rs:14529`.
  The plan cites `:14344` at its baseline; the arm moved to `:14529` at
  HEAD. The arm carries no `#[cfg]` attribute, so the route is reachable in
  a default build.
- `kernel_request` binds the route scope from the channel's management
  binding and the body's `project_root` (`kernel_routes/mod.rs:415-441`),
  then strips `method, kind, session_id, project_root` before strict
  parsing (`mod.rs:458-471`). The session id reaches the daemon only
  through the channel binding; no body field or commit-log column carries
  it.
- `CommitRequest` (`commit.rs:107-127`) is `deny_unknown_fields`;
  `Operation::Disposition { object_id, event, approval_object_id }`
  (`commit.rs:155-159`) is also `deny_unknown_fields`
  (`commit.rs:136-137` on the enum). `DispositionEvent` has five variants
  (`commit.rs:239-247`).
- `resolve_classes` derives the caller's `(SourceClass, TaintClass)` from
  `source_kind` (`derive_classes`, `commit.rs:368-380`; shared-facts cites
  `:366-378`, the function signature is at `:368`). The derived pair is
  stored in `CommitPlan.classes` and used only by `admit` for inserts; the
  disposition path never reads it.
- Commit path: `run` (`commit.rs:1041-1072`) calls
  `KernelStore::commit_before`, checks tokens, then `apply`
  (`commit.rs:899-1038`). Each disposition calls `record_disposition`
  (`commit.rs:783-796`) which calls `disposition_request`
  (`commit.rs:694-736`) and `Envelope::record_admission`
  (`admission.rs:982-987`).
- Preview path: `preview` (`commit.rs:875-897`) runs inside
  `KernelStore::preview`; `PreviewRun::disposition` (`commit.rs:825-863`)
  calls the same `disposition_request` and then
  `Preview::preview_admission` (`admission.rs:1859-1870`).
- Response: `CommitResult.dispositions` (`commit.rs:500-502`) is
  serialized into the receipt's `result` and read back by replays
  (`commit.rs:1188-1204`).

## Lens: state and persistence

- `commit_log` columns: `commit_seq, transaction_id, writer_epoch,
  producer, operation_key, request_digest, recorded_at, actor, cause`
  (`crates/kernel/src/schema.rs:26`); INSERT at
  `crates/kernel/src/envelope.rs:1484-1499`. Append-only triggers. No
  `session_id`, no `source_kind` column.
- `operation_receipts` keyed `UNIQUE(producer, operation_key)` with
  `request_digest` and `result_payload` (`schema.rs:42`);
  `stored_receipt` (`envelope.rs:1362-1392`) replays on equal digest and
  returns `Conflict` on a different digest. The lookup runs before the
  operation inside the writer transaction
  (`envelope.rs:1408-1416`).
- `admission_decisions` (`schema.rs:163`): `source_kind`/`source_id` are
  the subject's (`load_subject_facts`, `admission.rs:2131-2170`),
  `source_class`/`taint_class` are what the request carried,
  `reason TEXT NOT NULL`, `evidence_id TEXT REFERENCES
  evidence_meta(evidence_id) ON DELETE RESTRICT`. No length CHECK on
  `reason`. `foreign_keys=ON` is set at `schema.rs:244` and `open.rs:1035`.
- `evidence_meta` (`schema.rs:139`): live means `invalidated_commit_seq IS
  NULL`. The table has no `scope_id` column.
- `object_state_sql` (`envelope.rs:1898-1912`) reads `scope_id` as
  `COALESCE(dec.scope_id, obs.scope_id)`, so any object that is not a
  decision or observation has `scope_id = None`. `load_object_state`
  (`envelope.rs:1925-1939`) carries the doc comment at
  `envelope.rs:1923-1924` (shared-facts cites "~1870").
- `ScopeFilter::matches` returns `Ok(false)` for `scope_id = None`
  (`kernel_routes/project.rs:199-206`), so `scoped_object_state`
  (`commit.rs:676-691`) answers `NotFound` for every evidence object id.
- Liveness primitives on the slice write path: `require_live`
  (`crates/kernel/src/slice/write.rs:1138-1149`), `require_optional_live`
  (`:1110-1120`), `require_live_evidence` (`:1151-1178`), all filtering on
  `invalidated_commit_seq IS NULL` and returning `NotFound`.
- The admission write path has no liveness check on `evidence_id`:
  `write_admission` passes it through `identity()` only
  (`admission.rs:1690-1695`) and relies on the FK. A FK violation maps to
  `KernelError::Conflict` (`crates/kernel/src/lib.rs:212-224`), which the
  route reports as `invalid/already_exists` when raised inside `apply`
  (`commit.rs:1063-1071`).

## Lens: concurrency model

- One exclusive writer transaction per commit (`TransactionBehavior::
  Immediate`, `envelope.rs:1403-1406`); `deadline_ms` bounds writer
  acquisition only, operation count bounds the hold (`commit.rs:91-93`).
- Within an envelope, `admission_latest` caches each subject's newest
  decision so a later operation on the same object sees the earlier one as
  its prior (`write_admission`, `admission.rs:1835-1843`); the preview
  mirrors this (`admission.rs:1866-1869`) and refuses when an earlier
  previewed decision would have changed an approval chain
  (`refuse_changed_authority`, `admission.rs:1873-1885`; route maps it to
  `invalid_params`, `commit.rs:1172-1178`).
- Preview and commit are separate requests; the object can move between
  them. The TypeScript command treats the commit's verdict as the recorded
  one (`memory-mark-command.ts:301-302`).

## Lens: claimed safety guarantees (claims under test)

- commit.rs module header (`commit.rs:1-10`): "a caller may lower the
  derived class but never raise it"; every written row "admitted under
  the derived classes". The second sentence describes inserts; the
  disposition path copies the subject's prior classes instead
  (`commit.rs:725-726`).
- `Operation::Disposition` doc (`commit.rs:152-154`): "the kernel's fixed
  event table decides the resulting disposition, and a transition toward a
  less restrictive disposition needs a valid approval".
- `DispositionEvent` doc (`commit.rs:237-238`): "each is a disposition
  transition, never a maturity promotion".
- Plan P4 R4: "the report never raises maturity or relaxes a disposition;
  Active to Disputed needs no approval and lands regardless of source
  class". Code: `event_effect` returns `(current, Some(Disputed), Deny)`
  for MarkDisputed (`admission.rs:4161`); `historical = current.max(target)`
  with `target == current` (`admission.rs:336-337`); approval is required
  only when `relaxes_disposition` (`:321-330`); the class check compares
  the request to the prior (`:268-273`) and the route sends the prior's
  classes. Each piece is a claim under test, not a verified fact.
- Plan: "MarkDisputed forces maturity to Candidate". Code: the forcing is
  on `supported`, hence on `effective_maturity`
  (`admission.rs:345-349`); `historical_maturity` is preserved. The claim
  is precise only when read as effective maturity.
- Plan: "the event is recorded under the subject's prior source class".
  Code: `source_class: Some(prior.source_class)` (`commit.rs:725`). Claim
  under test.
- Plan: "evidence is dropped at the disposition path". Code:
  `evidence_id: None` at `3a3b916b7`; the operation shape had no
  evidence field and `deny_unknown_fields` refused one. Claim under test.
  PR A: the request carries the dispute's `evidence_id` (`commit.rs:731`).
- Index D3: "Authority never derives from model confidence". Code: no
  disposition event is in `event_can_raise_support` (`admission.rs:4189-4199`).
  The complement is implemented: a disposition can withdraw
  authority from an approval bearer and cascade demotion
  (`with_authority_cascade`, `admission.rs:1060-1082`). Claim under test.

## Lens: claimed liveness guarantees

None found on this surface. The route has no retry, no background work,
and no convergence obligation. `deadline_ms` is capped at 30 s
(`commit.rs:88-89`) and bounds writer acquisition only.

## Lens: existing test strategy

See `existing-checks.md`. Route tests drive `source_kind: "assistant"`
through a test `Daemon`; kernel tests drive `record_admission` directly;
TypeScript tests drive `runMemoryMarkCommand` against `FakeKernel`. No
test asserts the `source_class`, `taint_class`, `maturity`,
`effective_maturity`, `evidence_id`, or `reason` columns of a route-written
disposition row.

## Lens: failure and degradation

- Kernel refusals map through `kernel_routes/state.rs:252-287`; the
  disposition path can raise `NotFound` (subject, approval, or chain
  member out of scope, retired, or missing), `AdmissionPolicy` (no prior
  admission, class mismatch, blank reason), `Conflict` (reused key, FK),
  `InvalidInput` (identity with a secret-like substring, reason above
  512 KiB).
- A relaxation without approval is not a refusal: it writes a `Deny` row
  and the response carries `denied: true` (`commit.rs:760-780`;
  `kernel_routes.rs:4479-4503`).
- TypeScript: a commit whose verdict list does not match the sent
  operation is `unavailable(outcome_unknown)` because a receipt exists
  (`memory-mark-command.ts:298-300`).

## Lens: unproven assumptions

- The route assumes `subject_admission` returns the object's own latest
  decision (`AdmissionKey::Object`), not the lineage's; `visible_as_of`
  folds the lineage separately (`admission.rs:2172-2181`). A disposition on
  an object whose lineage holds a newer source rejection is judged against
  the object row only.
- `prepare_with_facts` back-fills the stored approval when the request
  names none (`admission.rs:1574-1585`); `disposition_request` scopes
  `approval_object_id.or(stored_approval)` (`commit.rs:710-716`), so the
  two agree on which approval is read, but nothing asserts this
  mechanically.
- `has_evidence: request.event.evidence_id.is_some()` (`admission.rs:1650`)
  is an existence test on the request, not on the store.

## Lens: wildcard

- `disposition_restrictiveness` ranks Stale, Disputed, and Superseded
  equal (`admission.rs:4168-4175`): Stale to Disputed and Disputed to
  Stale are neither tightening nor relaxing, so both land without
  approval. R4's "tightening only" wording does not describe this lateral
  move.
- A `MarkDisputed` on an `adr_accepted` approval object is a plugin-
  reachable write (the route requires only a live admitted decision in
  scope) that ends the approval's authority
  (`approval_row_fields` requires `disposition='active'`,
  `admission.rs:850`) and demotes every dependent in the same commit.
  Route test `kernel_routes.rs:5493` quarantines an approval and observes
  the dependent relaxation denied afterwards; no route test files
  `mark_disputed` against an approval.
- `Preview` with an already recorded identity replays the receipt and
  judges nothing (`commit.rs:878-880`); the command then commits, which
  also replays (`memory-mark-command.ts:270-296`).

## Contract-versus-code disagreements preserved as leads

1. Plan P4 R3 says the event row "can be joined to the commit log entry
   carrying caller-asserted `actor`, `cause`, `session_id`, and
   `source_kind`" citing `envelope.rs:1401-1410`. The `commit_log` schema
   (`schema.rs:26`) and the INSERT (`envelope.rs:1484-1499`) carry
   `actor` and `cause` only. `session_id` is stripped at
   `kernel_routes/mod.rs:461` and `source_kind` is consumed by
   `resolve_classes`. Joinability today is by `actor` and `cause` text.
   Lead for record `disposition-commit-log-row-carries-actor-and-cause-only`.
2. Plan P4 R2 says a cited `evidence_id` "must resolve to a live
   `evidence_meta` row visible to the caller's project scope". Evidence
   objects carry no scope (`envelope.rs:1923-1924`, `:1901`), and
   `scoped_object_state` answers `NotFound` for every evidence object id.
   The route's project-scope mechanism cannot answer R2 as written. Lead
   for record `evidence-objects-carry-no-scope-and-fail-scoped-lookup`.
3. Plan P4 Problem Frame says MarkDisputed "forces maturity to
   `Candidate` (`:347-351`)". At HEAD the forcing is at
   `admission.rs:345-349` and applies to `effective_maturity`;
   `historical_maturity` is unchanged. Lead for record
   `disposition-never-raises-maturity-and-withdraws-effective-support`.
4. Plan P4 KTD2 says the daemon will validate evidence as live before
   admission; the Kernel's own admission write enforces existence (FK) and
   not liveness, while the slice write path enforces liveness. Lead for
   records `admission-evidence-citation-is-existence-checked-only` and
   `slice-write-refuses-non-live-evidence-citations`.
5. Plan P4 cites `derive_classes` at `commit.rs:226-240` and
   `disposition_request` at `:590-596`; at HEAD they are at `:368-380` and
   `:694-736` (the request's `evidence_id` at `:731`; `None` at
   `3a3b916b7`, the cited id of a dispute after PR A). Line drift only at
   authoring; the cited behavior matched then.
6. commit.rs module header (`:7-10`) says every written row is "admitted
   under the derived classes". Disposition rows are admitted under the
   subject's prior classes. Documentation lead; recorded in
   `disposition-admission-copies-the-subjects-prior-classes`.
