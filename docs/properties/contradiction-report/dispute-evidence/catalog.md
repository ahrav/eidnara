# P4 contradiction report: proposed obligations

Every record below is a PROPOSED design obligation for behavior the plan
`04-contradiction-report-plan.md` introduces. None is implemented at HEAD
`3a3b916b7`; none is a discovered property. Records use the
`docs/properties/METHOD.md` schema with two additions the task brief
requires: a `Lifecycle:` line after `Status:` and a `Falsifier:` or
`Legal witness:` line after `Check:`. Remove both when a record is promoted
into a catalog after implementation, and write its `evidence/<slug>.md`
then. No evidence files accompany this deliverable; the evidence trail is
`domain-model.md` (sections cited per record).

`Status:` is the schema's field for the record once built and stays
`active`; the header line marks each as proposed. `Reachability:` is the
class the obligation will have when built, with the evidence for that
expectation. `Confidence:` is `low` for every record because there is no
implementation evidence.

Index:

| Slug | Type | Semantics | Rule |
|---|---|---|---|
| dispute-requires-evidence-from-plugin | safety | always | A1 |
| cited-evidence-live-at-dispute-commit | safety | always | A2, A12 |
| cited-evidence-visible-to-bound-project | safety | always | A3 |
| evidence-ref-well-formed-and-redaction-stable | safety | always | A4 |
| reporter-class-derived-never-asserted-up | safety | always | A5 |
| dispute-with-evidence-never-raises-standing | safety | always | A6, A7 |
| dispute-identity-includes-evidence-key | safety | always | A8 |
| dispute-preview-writes-nothing | safety | always-or-unreached | A9 |
| structured-reason-projects-typed-reporter-data | safety | always | A10 |
| use-outcome-link-names-own-session-prior-serve | safety | always-or-unreached | A11 |

### dispute-requires-evidence-from-plugin (PROPOSED)

Type: safety
Reachability: default-production - `kernel.commit` is dispatched for every
bound plugin connection (`crates/daemon/src/lib.rs:14367`); the new
fields ride the same route with no flag in the plan.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - the fields do not exist; `disposition_request`
hard-codes `evidence_id: None` (`crates/daemon/src/kernel_routes/commit.rs:595`).
Guarantee: A `mark_disputed` or `contradict` operation that reaches the
Kernel writer through `kernel.commit` carries `evidence_id`, `evidence_ref`,
or both; one lacking both is refused by name before any store access, and
`mark_stale`, `explicit_reject`, and `quarantine` keep today's shape.
Check: `always` - for every `admission_decisions` row with `event_kind IN
('mark_disputed','contradict')` and `reason` beginning with the PR A
structured prefix (or, U3, `reporter_taint_class IS NOT NULL`),
`evidence_id IS NOT NULL OR parse(reason).evidence_ref IS NOT NULL`
(U3: `event_payload.evidence_ref`); the predicate is evaluated on stored
rows, so it must hold whenever evaluated. Rows written through the Kernel
API by tests are excluded by the prefix or column test, which is why the
check keys on the route's own marker rather than on `event_kind` alone.
Falsifier: one stored row with `event_kind='mark_disputed'`, the route's
reporter marker, `evidence_id IS NULL`, and no `evidence_ref`.
Legal witness: `/eidnara-memory-mark mark-disputed <id> --path
src/lib.rs@<40 hex>` lands with `evidence_id NULL` and the path and oid in
the structured reason.
Fault/timing angle: none; the check is a parse-time predicate.
Required faults and enabling state: a plugin client that omits both
fields (today's client does exactly this, `client.ts:95-101`), or a client
bypassing the TypeScript parser and sending the raw body.
Confidence: low - proposed; no implementation evidence. Verified at HEAD
that `Operation::Disposition` has no evidence field (`commit.rs:97-104`)
and that `AdmissionEvent.evidence_id` exists on the Kernel side
(`crates/kernel/src/admission.rs:146-152`), so the carrier exists.
Existing check: none for evidence. `a_disposition_needs_a_live_admitted_decision`
(`crates/daemon/tests/kernel_routes.rs:4882`) covers the subject guard
only.
Impact: a dispute with nothing to check lowers a finding's standing on
assertion alone; the plan's objective ("recorded with the dispute so a
later reader can check it") fails silently.
Open questions:
- Whether "from a plugin" needs a marker at all: every `kernel.commit`
  caller is a plugin today; the Kernel API path is the only non-plugin
  writer. See conflicts Q5. (needs human input)

### cited-evidence-live-at-dispute-commit (PROPOSED)

Type: safety
Reachability: default-production - same route as above; the check runs
inside `commit_before` for every commit (`commit.rs:906-934`).
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - no evidence liveness check exists on the disposition
path; `write_admission` runs only `identity` on the id (`admission.rs:1685-1690`).
Guarantee: Every dispute row's `evidence_id`, when present, names an
`evidence_meta` row that was live at the dispute's `commit_seq`: created
before it and not invalidated at or before it; the check and the row write
observe one writer snapshot.
Check: `always` - for every row `r` with `r.evidence_id IS NOT NULL` and
`event_kind IN ('mark_disputed','contradict')`, the joined `evidence_meta`
row `e` satisfies `e.created_commit_seq < r.commit_seq AND
(e.invalidated_commit_seq IS NULL OR e.invalidated_commit_seq >
r.commit_seq)`. Stated as-of the row's commit, because evidence may be
invalidated later without fault (`EVIDENCE_CITED_SQL` excludes
`admission_decisions`, `crates/kernel/src/slice/write.rs:42-54`).
Falsifier: a row `r` citing `e` with `e.invalidated_commit_seq <=
r.commit_seq`, or citing an `evidence_id` with no `evidence_meta` row (the
FK at `schema.rs:162` already blocks the second form).
Legal witness: dispute citing a MemoryReviewer capture's evidence lands;
`admission_decisions.evidence_id` equals the cited id; a later capture
expiry sets `e.invalidated_commit_seq` above `r.commit_seq`.
Fault/timing angle: a preflight read outside the writer followed by a
concurrent `retire_evidence` (`write.rs:585`) or capture expiry
(`cas/deletion.rs:389-392`) before the writer is entered. Running the read
inside the `commit_before` closure removes the window.
Required faults and enabling state: a live decision in the bound project;
an evidence row that is retired between a client-side or preflight
observation and the commit.
Confidence: low - proposed; no implementation evidence. Verified the
exclusive-writer closure (`commit.rs:906-934`) and that invalidation is an
in-place update, never a delete (`cas/deletion.rs:389-392`; no `DELETE FROM
evidence_meta` in `crates/kernel/src/`).
Existing check: `require_live_evidence` (`write.rs:1151-1178`) and
`require_optional_live(.., "evidence_meta", ..)` (`write.rs:1107`) enforce
the same predicate on insert and decision-event paths; none on admission.
Impact: a dispute resting on retired evidence is unverifiable at filing
time, which is the condition the plan's stop rule names ("if the evidence
cannot be validated as live").
Open questions:
- Whether the check belongs in `write_admission` so the Kernel API path
  cannot bypass it. (needs human input)

### cited-evidence-visible-to-bound-project (PROPOSED)

Type: safety
Reachability: default-production - same route.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - and the predicate itself is undefined: evidence
objects carry no scope (`crates/kernel/src/envelope.rs:1844-1845`), and
`ScopeFilter::matches(None, ..)` is false (`project.rs:204-206`), so
`scoped_object_state` on an evidence object is always `NotFound`.
Guarantee: Under the recommended definition, a cited `evidence_id` is
accepted only when at least one live `decisions`, `observations`,
`decision_events` (through its decision), or `asserted_edges` row cites it
and that row's `scope_id` names the bound project; otherwise the envelope
is refused `not_found`, the same name used for a nonexistent id.
Check: `always` - for every dispute row `r` with `evidence_id`, at
`r.commit_seq` there existed a live citing row with a scope accepted by the
bound project's filter. Evaluated as an as-of predicate over the commit
history (the citing row may be retired later).
Falsifier: project P's dispute row cites evidence whose only citing rows
at that commit belong to project Q's scope, or to no scope.
Legal witness: a dispute cites the `evidence_id` on one of the bound
project's own live decisions and lands.
Fault/timing angle: same window as the liveness check; the citing row can
be retired between observation and commit.
Required faults and enabling state: two projects in one Kernel store; a
client in P naming an evidence id it learned from Q.
Confidence: low - proposed; no implementation evidence. Verified that
scope lives on `decisions`, `observations`, `asserted_edges`,
`propositions`, `scope_term`, `scopes` (`schema.rs:117, 125, 142, 166,
174`) and not on `evidence_meta` (`schema.rs:138`).
Existing check: none.
Impact: under "live only", accepting a foreign id both leaks its existence
(acceptance versus `not_found`) and attaches another project's evidence to
this project's finding; under the recommended definition neither occurs.
Open questions:
- Which definition: live-only; live and cited by a scoped object of the
  bound project (recommended); or another. See conflicts Q1. (needs human
  input)
- Whether evidence whose only citing object carries `scope_id NULL` is
  ever citable. (needs human input)

### evidence-ref-well-formed-and-redaction-stable (PROPOSED)

Type: safety
Reachability: default-production - same route; applies to both plugin
surfaces (`--path p@oid`, tool `path` plus `git_object_id`).
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet.
Guarantee: A persisted `evidence_ref` has a `git_object_id` of exactly 40
or 64 lowercase hex bytes and a `path` of at most 1024 bytes that passes
`confined_path::check` and that the redaction scanner leaves unchanged; any
other reference is refused `invalid_input` before store access.
Check: `always` - for every stored `evidence_ref` (PR A: parsed from
`reason`; U3: `event_payload`), `is_lower_hex_oid(oid)` holds
(`crates/kernel/src/scope.rs:710-715`), `len(path) <= 1024`
(`project_text.rs:37`; `kernel::MAX_TEXT_FIELD_BYTES`, `cas/mod.rs:35`),
`confined_path::check(path)` is `Ok` (`confined_path.rs:29-41`), and
`path` contains no redaction placeholder.
Falsifier: a stored reference with an uppercase oid, a 39-byte oid, a path
containing `..` or a NUL, more than 16 components, or a path containing
the redaction placeholder because `redact` rewrote it.
Legal witness: `src/lib.rs@<40 lowercase hex>` is stored byte-identical.
Fault/timing angle: none; intrinsic admission.
Required faults and enabling state: a client sending any malformed
reference; a path that looks like a secret to the scanner.
Confidence: low - proposed; no implementation evidence. Verified the
three validators exist and that `is_lower_hex_oid` is `pub(crate)` to the
Kernel, so the daemon cannot call it today without an export.
Existing check: `validate_relative` (`project_text.rs:627-640`) applies the
path rule to captures; `local_file.rs:143` refuses a capture path the
scanner would rewrite; `is_lower_hex_oid` has Kernel-side callers only.
Impact: a malformed reference cannot be followed by a later reader; a
redaction-rewritten path silently points at a file that does not exist.
Open questions:
- Where the oid validator is exposed (Kernel export or daemon copy).
- Whether the daemon verifies that the blob exists at the oid in the
  repository. The plan does not ask for it; recorded as out of scope.

### reporter-class-derived-never-asserted-up (PROPOSED)

Type: safety
Reachability: default-production - same route.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - the reporter class is not recorded anywhere today;
the admission row carries the subject's classes (`load_subject_facts`,
`admission.rs:2126-2167`; `disposition_request` passes `prior.source_class`
and `prior.taint_class`, `commit.rs:589-590`).
Guarantee: The reporter class recorded with a dispute equals
`derive_classes(source_kind)` lowered by an asserted class at or below it
(`user_inferred` for the command, `assistant_inference` for the tool), and
the row's own `source_class`/`taint_class` remain the subject's prior; no
actor string, cause, or client field decides either.
Check: `always` - for every dispute row `r`, recorded reporter taint
`<= derive_classes(commit.source_kind).taint` in the daemon's rank order
(`taint_rank`, `commit.rs:301-312`), and `(r.source_class, r.taint_class)`
equal the subject's previous row's. The second half is the Kernel's
existing refusal (`admission.rs:268-274`), restated here because the
reporter data must not be written into those columns.
Falsifier: a row whose reporter taint is `explicit_user` or whose
`taint_class` differs from the prior row's.
Legal witness: command dispute records reporter `user_inferred`; tool
dispute from the same session on the same decision records
`assistant_inference`; both rows show the subject's unchanged classes.
Fault/timing angle: none.
Required faults and enabling state: a client asserting
`asserted_taint_class` above the derived class (refused
`class_over_declared` today); a client sending `source_kind: "user"` from
the tool (accepted today; the reporter class follows the claim, which is
why the plan keeps `ExplicitUser` out of scope).
Confidence: low - proposed; no implementation evidence. Verified
`derive_classes` (`commit.rs:240-250`) and `resolve_classes`
(`:283-312`).
Existing check: `resolve_classes` refuses over-declaration; Kernel refuses
subject class change; neither records the reporter.
Impact: a reporter class above the derived one would give model output
the standing of a user statement (D3 violated).
Open questions:
- Whether the route verifies `actor` against `RouteScope.harness`
  (`mod.rs:409`) for `agent:<harness>`. See conflicts Q4. (needs human
  input)

### dispute-with-evidence-never-raises-standing (PROPOSED)

Type: safety
Reachability: default-production - same route.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: partial - the event table is covered without evidence
(`disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`,
`kernel_routes.rs:4392`); the plan asks for the existing test to be
extended with evidence.
Guarantee: A dispute carrying `evidence_id` or `evidence_ref` yields
`outcome=deny`, keeps historical maturity, sets effective maturity to
`candidate`, and moves the disposition only toward greater restrictiveness
or leaves it unchanged with `denied=true`; the evidence on the event has no
lever on maturity.
Check: `always` - for every dispute row `r` and its prior `p`:
`r.outcome='deny'`, `r.maturity=p.maturity`,
`r.effective_maturity='candidate'`, and
`restrictiveness(r.disposition) >= restrictiveness(p.disposition)`
(`disposition_restrictiveness`, `admission.rs:4163-4170`). Chosen over
`unreachable` because there is no single code point; the forbidden thing
is a state.
Falsifier: a row with `outcome='promote'`, or `effective_maturity` above
`candidate`, after a `mark_disputed` that carried `evidence_id`.
Legal witness: `Active` with effective `Corroborated` becomes `Disputed`
with effective `Candidate`, historical `Corroborated`.
Fault/timing angle: none.
Required faults and enabling state: a subject above `Candidate`; a dispute
with evidence; the only evaluator gate on `has_evidence` is `Enforce`
(`admission.rs:275-277`), so the field is inert for these events.
Confidence: low - proposed; no implementation evidence for the evidence
case. Verified `event_effect` for `MarkDisputed` and `Contradict`
(`admission.rs:4156-4157`) and the support withdrawal
(`admission.rs:345-351`).
Existing check: the two route tests above and
`crates/kernel/tests/kernel_admission.rs:885, 2269, 2894` for the kernel
event table without evidence.
Impact: KTD4 and D3 fail: a report would become a promotion path.
Open questions:
- Whether a dispute against a `Rejected`, `Contradicted`, or
  `Quarantined` prior should be refused outright rather than recorded
  `denied=true` with the evidence on the row (today's table records it).
  See conflicts Q9. (needs human input)

### dispute-identity-includes-evidence-key (PROPOSED)

Type: safety
Reachability: default-production - the slash command on both harnesses;
the tool keys per tool call and is unaffected.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - today's command identity is `[sessionId, event,
objectId]` (`memory-mark-command.ts:98-100`), and `request_digest` covers
the operations (`client.ts:262-271`), so a second dispute with different
evidence in one session hits `operation_key_reused`.
Guarantee: One dispute's identity is (reporting session, event, subject,
canonical evidence key). A byte-identical repeat replays the stored receipt
with no second commit; a different evidence key in the same session is a
second dispute that lands; a different body under the same identity is
refused `operation_key_reused`.
Check: `always` - (a) for every pair of `commit_log` rows with equal
`(producer, operation_key)` there is at most one row (the unique index
`idx_commit_operation`, `schema.rs:25`, already enforces this); (b) two
commits from one session for one subject with different evidence keys
produce two `admission_decisions` rows and two receipts; (c) the evidence
key is U+001F-free (`joinKeyFields`, `client.ts:237-245`).
Falsifier: the second `--evidence` in a session answers
`operation_key_reused`; or two `commit_log` rows exist for one
byte-identical repeat.
Legal witness: `--evidence A` then `--evidence B` in one session: two rows,
`replayed:false` both; `--evidence A` again: `replayed:true`, row count
unchanged.
Fault/timing angle: a transport loss after commit followed by a client
retry with the same identity must replay, not duplicate (the receipt
lookup precedes the operation, `commit.rs:927-935`).
Required faults and enabling state: the same session filing twice with
different evidence; a client retry after an ambiguous response.
Confidence: low - proposed; no implementation evidence. Verified the
receipt mechanism (`envelope.rs:1283-1313`) and the project-prefixed key
(`project.rs:78-83`).
Existing check: `disposition_operations_share_the_envelope_replay_and_token_rules`
(`kernel_routes.rs:4976`) covers replay of an identical dispute; nothing
covers a second evidence.
Impact: the user's second piece of evidence is refused with a message
about a reused key, and the record keeps only the first.
Open questions:
- Whether the alternative (keep today's key; one dispute per session per
  object) is intended. See conflicts Q2. (needs human input)

### dispute-preview-writes-nothing (PROPOSED)

Type: safety
Reachability: default-production - `runMemoryMarkCommand` previews before
every commit (`memory-mark-command.ts:126`).
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: partial - preview without evidence is covered
(`a_disposition_preview_reports_visibility_per_surface_and_writes_nothing`
`kernel_routes.rs:5114`,
`a_disposition_preview_reports_denials_and_refusals_like_the_commit`
`:5349`); with the new fields, not yet.
Guarantee: A `preview: true` envelope carrying the new fields runs the same
admission (evidence required, live, visible, well-formed) and leaves
`admission_decisions`, `commit_log`, `operation_receipts`, and
`durable_text_redactions` row counts unchanged; its refusal names equal
those the commit would return for the same body.
Check: `always-or-unreached` - the preview path is optional (the tool may
skip it); when it runs, row counts before and after are equal and, for an
invalid evidence input, the preview's refusal name equals the commit's.
Falsifier: a preview that increments any of the four row counts; or a
preview that accepts an `evidence_id` the commit then refuses `not_found`
with no intervening commit.
Legal witness: preview of `--evidence <retired id>` answers `not_found`;
counts unchanged; commit of the same body answers `not_found`.
Fault/timing angle: an intervening commit can change the answer; that is
permitted and the commit's verdict wins (`memory-mark-command.ts:147`).
Required faults and enabling state: a disposition with evidence under
`preview: true`.
Confidence: low - proposed; no implementation evidence. Verified today's
preview contract (`commit.rs:68-72`, `:729-756`, `:1001-1040`).
Existing check: preview tests cited above cover the no-evidence case.
Impact: a writing preview would record disputes the user declined.
Open questions: None.

### structured-reason-projects-typed-reporter-data (PROPOSED)

Type: safety
Reachability: default-production - every dispute row written in PR A; in
U3 every row additionally has typed columns.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - `reason` is the constant `"kernel.commit"` today
(`commit.rs:45, 596`).
Guarantee: In PR A, `reason` on a dispute row is a deterministic,
version-tagged rendering of (reporter taint, optional `evidence_ref`) over
an identity-safe alphabet, so `redact` leaves it unchanged and a parser
recovers the values exactly; in U3, `reporter_source_class`,
`reporter_taint_class`, and `event_payload` are authoritative and `reason`
is derived from them or constant. No admission decision ever reads
`reason`.
Check: `always` - PR A: `parse(reason)` succeeds and `render(parse(reason))
== reason` and `reason` contains no redaction placeholder and no
`durable_text_redactions` row exists for `(admission_decision, id,
"reason")`. U3: `render(typed columns) == reason` or `reason` equals the
constant. The Kernel reads `reason` only in `redact` and `record`
(`admission.rs:1684, 1782-1789`); that remains true.
Falsifier: a dispute row with a `durable_text_redactions` row for its
`reason`; or `parse(reason)` disagreeing with `reporter_taint_class`.
Legal witness: a dispute with `--path src/a.rs@<oid>` from the command
stores a `reason` that parses to `{taint: user_inferred, ref: {path:
"src/a.rs", oid}}`.
Fault/timing angle: none.
Required faults and enabling state: a path or id that the scanner would
rewrite (refused by the well-formedness record, so this record's violation
requires that one to fail first).
Confidence: low - proposed; no implementation evidence. Verified that
`reason` has no length CHECK (`schema.rs:162`), that `redact` is lossy
(`redaction.rs:41, 73-78`), and that `prepare_with_facts` refuses a blank
reason (`admission.rs:1560-1562`).
Existing check: none.
Impact: a reader cannot recover the reporter or the reference; U3 cannot
be verified against PR A behavior.
Open questions:
- The grammar and version tag (implementation choice; record it in the
  route module header per R7).
- PR A rows do not carry into the Batch K1 store under single-baseline
  storage; confirm that no cutover of `reason` into typed columns is
  expected. See conflicts Q6. (needs human input)

### use-outcome-link-names-own-session-prior-serve (PROPOSED)

Type: safety
Reachability: default-production - once Batch K1 lands (P3 U1 table and
P4 U3 columns); before that the column does not exist.
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - neither the `use_outcome` table nor the column
exists at HEAD.
Guarantee: A dispute row's `use_outcome_id`, when set, names a
`use_outcome` row whose session is the route's bound session, whose
decision is the dispute's subject, and whose `served_commit_seq` is the
greatest one strictly below the dispute's `commit_seq`; it is null when no
such row exists.
Check: `always-or-unreached` - the link path runs only when P3 is present;
when it runs, for every row `r` with `r.use_outcome_id IS NOT NULL`, the
joined `use_outcome` row `u` has `u.session = bound session`,
`u.decision = r.subject_object_id`, `u.served_commit_seq < r.commit_seq`,
and no other row for the same (session, decision) has a greater
`served_commit_seq` below `r.commit_seq`.
Falsifier: `u.session` differs from the reporting session; or
`u.served_commit_seq >= r.commit_seq`.
Legal witness: session S served D at seq 100 and 140; S disputes D at seq
150; the link names the seq-140 row.
Fault/timing angle: the lookup and the write share one writer transaction;
a serve committed after the dispute's snapshot is not linked (correct). A
P3 sweep must keep rows a live dispute cites (P3 U1 approach step 2).
Required faults and enabling state: P3 U1 landed; a session served the
decision before disputing it.
Confidence: low - proposed; no implementation evidence. Verified that the
daemon knows the bound session at the route (envelope keys stripped at
`mod.rs:460-472`; `RouteScope` at `:404-410`) and that no column carries
it today.
Existing check: none.
Impact: a wrong link attributes a harm signal to a session that did not
receive the finding; P7 counterfactuals read the wrong rows.
Open questions:
- Whether the bound session is the right key when the same session id
  appears under two harnesses (the binding carries both). (needs human
  input)
- Whether a dangling `use_outcome_id` after a P3 sweep is a sweep defect
  (recommended) or the column should carry no FK. See conflicts Q10.
