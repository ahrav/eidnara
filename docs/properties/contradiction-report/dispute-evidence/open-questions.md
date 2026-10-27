# P4 contradiction report: conflicts and questions

Every item is either a conflict between the plan
(`04-contradiction-report-plan.md`, `00-index.md`, `03-use-outcome-record-
plan.md`) and repository evidence at HEAD `3a3b916b7`, or a design question
the implementer must not decide silently. Each question carries a
recommended default and the alternative; both are labelled recommendation,
never decision. "Needs human input" marks the ones that change an accepted
decision's wording or an authority boundary.

Reference note (PR A, #1160): this document describes the code at
`3a3b916b7` and is kept as the design-time record. Its `file:line`
references were remapped to the PR A branch tip (base `6159fd224`, code
commit `6c4d445e3`) through the git diff between the two revisions; where
PR A changed the cited code, the text says so inline with "PR A:". The
active records in [`dispute-evidence/catalog.md`](catalog.md)
and [`disposition-path/catalog.md`](../disposition-path/catalog.md) state the PR A
behavior.

Evidence scope (user-confirmed, verbatim): "the plan set (00-index, P3, P4
plans) plus repository code, tests, docs/host-wire-protocol.md,
docs/memory-reviewer-operations.md, and docs/properties at HEAD. No tracker
issues, no incident history, no git history lens."

## Part A. Conflicts between the plan and repository evidence

### C1. R2 "visible to the caller's project scope" has no direct meaning

Plan: R2 and U1 step 2 require the cited `evidence_id` to resolve to a live
`evidence_meta` row "visible to the caller's project scope"; the U1 test
list names a "foreign-scope `evidence_id`" refusal.

Evidence: evidence objects carry no scope. `object_state_sql` reads scope
only from `decisions` and `observations` (`crates/kernel/src/envelope.rs:
1878-1891`); the doc comment says every other kind reads `None`
(`:1924-1925`). `ScopeFilter::matches(None, ..)` returns `false`
(`crates/daemon/src/kernel_routes/project.rs:204-206`), so
`scoped_object_state` on an evidence object's `object_id` is always
`NotFound` (`commit.rs:676-690`). Tables with `scope_id`: `propositions`,
`scopes`, `asserted_edges`, `decisions`, `observations`
(`crates/kernel/src/schema.rs:118, 126, 143, 167, 175`).

Consequence: implemented literally, every `evidence_id` is refused. The
decision needs a definition. See Q1.

### C2. R3 and U1 verification name columns the commit log does not have

Plan: R3 says the event row joins to a commit-log entry carrying
"`actor`, `cause`, `session_id`, and `source_kind`"; U1 verification says
"the receipt and commit log carry actor, cause, and session for the
event".

Evidence: `commit_log` columns are `commit_seq, transaction_id,
writer_epoch, producer, operation_key, request_digest, recorded_at, actor,
cause` (`schema.rs:26`; insert `envelope.rs:1419-1435`). `IntentRequest`
carries `producer, operation_key, request_digest, actor, cause`
(`project.rs:139-146`). The transport strips `session_id` before the body
is parsed (`mod.rs:461-473`). `operation_key` is
`sha256(producer, actor, operationId)` on the client (`client.ts:284-290`),
so the session inside `operationId` is not recoverable from the stored
key.

Consequence: the session and `source_kind` are not joinable from PR A rows
by any existing column. The reporter's taint class (derived from
`source_kind`) is what the structured `reason` carries in PR A (OQ4-1), so
`source_kind` is recoverable to class granularity. The session is not
recoverable until U3 adds a column or the plan drops the claim. See Q3.

### C3. Index D5 wording versus R5 tool input `evidence_id`

Index D5: "Evidence citations are derived server-side from validated
identities; model output never names an evidence id (P2 KTD3, P4 KTD2)."

Plan R5: the `eidnara_memory` tool action `dispute` accepts `evidence_id`
or `path` plus `git_object_id`; the tool is the model's surface
(`EIDNARA_MEMORY_ACTOR = "agent:opencode"`, `execute.ts:50`).

Evidence: both statements are in the plan set; the code has neither. The
CI gate "Invented provenance" exists (index, common verification). Whether
a served surface ever shows the model an evidence id was not verified in
this scope.

Consequence: a model-named `evidence_id`, even if validated live and
visible, is model output naming an evidence id. KTD2's validation is the
plan's reconciliation; the index sentence as written does not allow it.
Not resolvable from code. See Q11 (needs human input).

### C4. R5 `actor: agent:<harness>` versus today's constant

Plan R5: the tool sends `actor: agent:<harness>`.

Evidence: before #1161 both harnesses sent the constant `agent:opencode`;
#1161 keeps it for OpenCode
(`packages/opencode-plugin/src/tools/eidnara-memory/execute.ts:50`) and
gives the Pi tool `agent:pi`
(`packages/pi-plugin/src/tools/eidnara-memory.ts:58`). The daemon knows
the bound harness (`RouteScope.harness`, `mod.rs:410`) and does not check
the actor against it; the actor is a claim (`docs/host-wire-protocol.md:
26`).

Consequence: the Pi plugin needs its own constant (`agent:pi`), which
changes the Pi tool's `operation_key` for every future write (the key
hashes the actor), so a Pi tool call replayed across the change would key
differently. Not a correctness break (receipts are per identity), but a
behavior change outside the plan's stated files
(`packages/pi-plugin/src/tools/eidnara-memory.ts` is not in U2's file
list). See Q4.

### C5. Slash-command operation identity collides on a second evidence

Plan: none of R1 to R7 mentions the operation identity; the plan assumes a
dispute with evidence rides today's envelope.

Evidence at `3a3b916b7`: `memoryMarkOperationId` was `[sessionId, event,
objectId]`; `deriveRequestDigest` covers the operations
(`client.ts:272-281`); a stored receipt with a different digest is
`KernelError::Conflict` (`envelope.rs:1385-1387`), which `run` maps to
`OperationKeyReused` when raised before entering the closure
(`commit.rs:1062-1070`).

Consequence at `3a3b916b7`: a user who disputed the same finding twice in
one session with different evidence got `operation_key_reused`. PR A
closed it: `memoryMarkOperationId` appends the evidence flags
(`memory-mark-command.ts:210-215`), so two pieces of evidence are two
identities; see `domain-model.md` section 7.3 and
`memory-mark-command.test.ts:212`. The tool's identity is per tool call
(`execute.ts:430-432`) and never collided. See Q2.

### C6. U3 typed columns cannot be backfilled from PR A rows

Plan: OQ4-1 and OQ4-2 encode reporter class and `evidence_ref` in
`reason` for PR A and move them to typed columns in Batch K1.

Evidence: single-baseline storage, no migrations (`crates/storage/AGENTS.md:
3`); a Kernel baseline digest change makes a database created under the
previous baseline refuse to open (P3 U1 test scenario; the Memory Store
precedent at `docs/memory-reviewer-operations.md:86-88`).

Consequence: there is no supported path by which a PR A dispute row exists
in the Batch K1 store, so U3 has nothing to parse and no cutover to run.
The structured `reason` still matters in PR A for readers, and its grammar
still needs a version tag so a mixed deployment (PR A daemon, later reader)
is recognizable. Not a conflict in behavior; a conflict in what "move to
typed columns" can mean. See Q6.

### C7. Plan line references that are off at HEAD

The plan cites `derive_classes` at `commit.rs:226-240` (HEAD: `:368-378`),
`disposition_request` evidence at `:590-596` (HEAD `3a3b916b7`: `:595-596`;
PR A: `:731-732`), and the
reporter join at `envelope.rs:1342-1351` (HEAD: insert at `:1419-1435`).
Shared-facts corrections are in `domain-model.md` section 0. No semantic
claim changes.

### C8. "Required ... from a plugin" has no non-plugin peer on the route

Plan R1 distinguishes plugin callers. Evidence: every `kernel.commit`
request arrives through a bound plugin connection (dispatch
`crates/daemon/src/lib.rs:14529`; the daemon has no internal caller of the
route). The only non-plugin writer of disposition events is the Kernel API
`Envelope::record_admission` (`admission.rs:982`), used by Kernel tests.
Consequence: at the route the qualifier is vacuous; the real question is
whether the Kernel API path must also require and validate evidence. See
Q5.

## Part B. Design questions the implementer must not decide silently

### Q1. What "visible to the caller's project scope" means (needs human input)

Options:

- (a) Live-only: any `evidence_meta` row with `invalidated_commit_seq IS
  NULL` in the store. Authority consequence: a project can attach another
  project's evidence to its own finding, and acceptance versus `not_found`
  reveals whether a foreign id exists.
- (b) Live and cited by a live scoped object of the bound project: at
  least one live `decisions`, `observations`, `decision_events` (via its
  decision), or `asserted_edges` row cites the evidence and carries a
  `scope_id` that `ScopeFilter::matches` accepts. Authority consequence:
  only evidence already attached to this project's objects is citable;
  refusal is `not_found` for nonexistent, retired, and foreign alike, so no
  existence oracle; a fresh capture whose observation carries no scope is
  not citable by id (it is citable by `evidence_ref`).
- (c) Live and the evidence's registry `domain_id` equals the project's
  domain. Not a project boundary: `MEMORY_DOMAIN_ID` is shared across
  projects (`execute.ts:46`).

Recommendation: (b), implemented inside the writer transaction, with the
citing-row scan bounded. Alternative: (a), with the plan's R2 wording
changed to "live" and the oracle accepted. Whether evidence cited only by
objects with `scope_id NULL` is ever citable is a sub-question; the
recommendation is no.

### Q2. What one dispute is, for idempotency (needs human input)

Options:

- (a) Identity = (session, event, subject, canonical evidence key). Two
  pieces of evidence are two disputes; each lands as its own admission row
  (the Kernel accepts a repeat `mark_disputed` on a `Disputed` decision as
  `outcome=deny`, disposition unchanged, `admission.rs:321-367`). Repeats
  of the same evidence replay.
- (b) Identity = (session, event, subject) as today. One dispute per
  session per finding; a second evidence is refused `operation_key_reused`
  and the client must tell the user to amend nothing (the first stands).
- (c) Identity = (session, event, subject, tool-call or invocation id).
  Every invocation is a new dispute; byte-identical repeats from a new
  invocation duplicate rows.

Recommendation: (a). It matches the plan's objective (evidence recorded so
a later reader can check it: two references, two rows) and keeps the
replay guarantee the command's doc comment promises (PR A implements (a):
`memoryMarkOperationId`, `memory-mark-command.ts:210-215`). Alternative: (b), if the product wants one
dispute per session and a clear "already disputed" message. (c) loses the
replay property and is not recommended. Under (a), the evidence key must be
U+001F-free (`client.ts:247-255`); A4 excludes NUL only, so the parser must
also refuse U+001F in `path` as it does for `object_id`
(`memory-mark-command.ts:124-126`).

### Q3. How the reporting session is joinable (needs human input)

Options:

- (a) PR A: not joinable; the structured `reason` carries the reporter
  taint class only; the commit log carries `actor` and `cause`. U3 adds a
  typed `reporter_session` (or relies on `use_outcome_id`, which exists
  only when the decision was served to that session).
- (b) PR A: encode the session id in the structured `reason`. It then
  passes `redact` (session ids are identity-safe) and is readable, but it
  widens the data a `kernel.*` row stores about a session.
- (c) Amend R3 and U1 verification to drop "session" for PR A.

Recommendation: (c) for PR A, with (a)'s typed column deferred to U3 only
if P3's link is insufficient. Alternative: (b), if the operator read
"which session filed this" is required before Batch K1. Either way, the
claim that the commit log "carries session" is false at HEAD and the plan
text should change.

### Q4. Whether the daemon verifies `actor: agent:<harness>` against the binding

Options:

- (a) Verify: refuse `invalid_input` when `actor` starts with `agent:` and
  the suffix differs from `RouteScope.harness` (`mod.rs:410`). The class
  still comes from `source_kind`.
- (b) Do not verify; the actor is a claim (bearer-only authority,
  `docs/host-wire-protocol.md:26`), consistent with every other route.

Recommendation: (b), with the Pi plugin adopting `agent:pi` and the route
doc stating that `actor` is a claim. Alternative: (a), if a wrong harness
label in the commit log is considered a data-quality defect worth a
refusal. Note C4: changing the Pi constant changes future Pi tool operation
keys.

### Q5. Where the evidence liveness check lives (needs human input)

Options:

- (a) Daemon route only (plan KTD2 as written). The Kernel API path
  (`record_admission`) remains able to write any `evidence_id`; Kernel
  tests and future daemon code bypass the check.
- (b) Kernel `write_admission` (or `prepare_with_facts`) requires a live
  evidence row for any `evidence_id` on any event, using
  `require_optional_live(tx, "evidence_meta", ..)` (`write.rs:1107`)
  which already exists for inserts. Closes the writer inventory; widens the
  change into `crates/kernel` and may affect existing Kernel tests that
  write admission events with arbitrary evidence ids.

Recommendation: (a) for PR A to keep the change inside the route, with the
open bypass recorded in the route module header and the catalog record;
(b) considered for Batch K1 when the Kernel baseline changes anyway. This
is a scope decision above the implementer.

### Q6. What "move to typed columns" means given single-baseline storage

Options:

- (a) U3 adds the columns; PR A rows are not expected in the K1 store; the
  `reason` grammar keeps a version tag so any reader can tell PR A rows
  from U3 rows if a store is ever carried by an unsupported path. No
  cutover code.
- (b) U3 also ships a one-time parser from `reason` into typed columns, to
  be run by an operator tool. This is migration vocabulary the gates job
  rejects ("schema-migration identifiers", index common verification).

Recommendation: (a). Alternative (b) conflicts with the repository's
storage rule and is not recommended.

### Q7. Whether a dispute's citation pins evidence retention (needs human input)

Fact: `EVIDENCE_CITED_SQL` (`write.rs:42-54`) does not include
`admission_decisions`, so `retire_evidence` and capture expiry
(`local_file.rs:3`) can invalidate evidence a dispute cites. The FK
`ON DELETE RESTRICT` (`schema.rs:163`) is inert because evidence rows are
invalidated, never deleted.

Options:

- (a) Do not pin. The dispute records "live at filing"; readers check
  `invalidated_commit_seq` (derived-state contract 9.4). Capture retention
  windows stay finite as designed.
- (b) Pin: extend `EVIDENCE_CITED_SQL` with a live `admission_decisions`
  citation (live meaning the subject decision is live). Disputes then hold
  MemoryReviewer captures past `retain_until`, which changes the capture
  retention contract and the reviewer hold accounting
  (`memory_reviewer_hold.rs:408`).

Recommendation: (a) for PR A; the plan's KTD2 speaks only of validation at
admission. Alternative (b) is a retention-policy change and needs an owner.

### Q8. Replay response shape for a committed dispute

Fact: a replayed preview returns `previews: []` with the receipt
(`commit.rs:1163-1170`); the TS command treats a commit receipt with no
matching verdict as `outcome_unknown` (`memory-mark-command.ts:299-300`).
Whether a replayed commit returns the stored verdict list was not traced
end to end here.

Recommendation: the commit replay returns the stored `result` so the
client reports "already recorded (replayed)" rather than
`outcome_unknown`. Alternative: keep today's shape and have the client
treat `replayed: true` as applied. Verify the actual replay body before
choosing.

### Q9. A dispute against a more restrictive prior (needs human input)

Fact: `mark_disputed` on a `Rejected`, `Contradicted`, or `Quarantined`
decision is a relaxation (`disposition_restrictiveness`, `admission.rs:
4168-4175`); without approval it is recorded with `denied=true`, outcome
`deny`, disposition unchanged (`admission.rs:330-334`), and the row is
written with its evidence.

Options:

- (a) Keep the table: the row lands `denied`, the evidence is on it, the
  client reports "denied" (today's `denied` outcome,
  `memory-mark-command.ts:302`).
- (b) Refuse at the route before writing when the requested disposition
  is less restrictive than the prior, so no evidence is recorded against a
  finding that is already more than disputed.

Recommendation: (a); it adds data without authority and matches R4. The
alternative removes a possibly useful record and adds a route rule the
Kernel already expresses.

### Q10. FK and sweep behavior for `use_outcome_id` (U3)

Options:

- (a) FK `REFERENCES use_outcome(...) ON DELETE RESTRICT`, and the P3 sweep
  excludes rows a live dispute cites (P3 U1 step 2 says it does).
- (b) No FK; the link may dangle after a sweep; readers treat a missing
  row as "served record expired".

Recommendation: (a), consistent with every other citation FK in the
schema. Alternative (b) only if P3's retention class must expire rows
regardless of citations.

### Q11. Whether the tool may name an `evidence_id` at all (needs human input)

See C3. Options:

- (a) Keep R5: the tool accepts `evidence_id`, the daemon validates it
  (Q1 definition), and the index's D5 sentence is amended to "model output
  never names an evidence id that the daemon has not validated as live and
  visible".
- (b) Restrict the tool to `path` plus `git_object_id`; `evidence_id` stays
  a user-only input on the slash command. D5 holds as written.

Recommendation: (a), because the plan's accepted decision is R5 and KTD2
is cited by D5 as its mechanism; the wording change is editorial. The
alternative (b) is safer against invented provenance and costs nothing if
no served surface shows the model evidence ids, which this scope could not
confirm. This is the one question where the two accepted texts cannot both
stand unchanged.

### Q12. Reporter marker for the "from a plugin" test (implementation)

The route must distinguish its own dispute rows from rows written through
the Kernel API when a catalog check asserts "every route-written dispute
has evidence". Recommendation: the structured `reason` prefix in PR A and
`reporter_taint_class IS NOT NULL` in U3 serve as the marker; the Kernel
API path writes neither. Alternative: a dedicated `writer` column in U3.
No human input needed; record the choice in the route header.

## Part C. Items verified as consistent (no conflict)

- KTD1: `kernel.commit` is the only route that writes an admission event
  from a plugin (`commit.rs:1-10`; dispatch `lib.rs:14529`); section 7.8 of
  `docs/host-wire-protocol.md` lists no `kernel.*` method (`:767`), so the
  route doc is the module header plus the `docs/properties` catalogs.
- KTD4 and R4: `MarkDisputed` and `Contradict` request `Deny` with a more
  restrictive disposition (`admission.rs:4161-4162`); support is withdrawn
  to `Candidate` (`:345-351`); `has_evidence` gates only `Enforce`
  (`:275-277`).
- Non-goal `ExplicitUser`: `derive_classes` maps every `source_kind` to
  `ModelInference` (`commit.rs:368-378`).
- Non-goal free-text rationale: `reason` is redacted lossy
  (`redaction.rs:41, 73-78`) and bounded at 512 KiB
  (`secret-scanner/src/api.rs:6`); the structured reason stays within an
  identity-safe alphabet, so the non-goal holds.
- R2 bounds: `is_lower_hex_oid` (`scope.rs:710-716`; PR A made it `pub`)
  and the path rule
  (`project_text.rs:37, 627-640`; `confined_path.rs:29-41`) exist; the oid
  validator is `pub(crate)` and needs an export or a copy.
- P3 join: "later MarkDisputed/Contradict events on a served decision are
  joined by query" works on `admission_decisions.subject_object_id` and
  `commit_seq` with no P4 change.
