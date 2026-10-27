# P4 contradiction report: domain model (proposed)

Status of this document: a PROPOSED semantic design for behavior the plan
`docs/plans/2026-10-24-adaptive-discovery/04-contradiction-report-plan.md`
introduces and the repository does not yet implement. Every obligation here
is a design proposal, never a discovered fact. Facts about today's code are
labelled `FACT` with a `file:line` citation verified against HEAD
`3a3b916b7`. Everything else is `PROPOSED`.

Reference note (PR A, #1160): this document describes the code at
`3a3b916b7` and is kept as the design-time record. Its `file:line`
references were remapped to the PR A branch tip (base `6159fd224`, code
commit `6c4d445e3`) through the git diff between the two revisions; where
PR A changed the cited code, the text says so inline with "PR A:". The
active records in [`dispute-evidence/catalog.md`](catalog.md)
and [`disposition-path/catalog.md`](../disposition-path/catalog.md) state the PR A
behavior.

Method: `/software-design:invariant-driven-domain-modeling`. The two-axis
rule table, enforcement sets, certainty-first outcomes, and earning verdicts
are that skill's authored procedure, not a theorem.

## 0. Scope of evidence and corrections

External-evidence scope, recorded verbatim as user-confirmed: "the plan set
(00-index, P3, P4 plans) plus repository code, tests,
docs/host-wire-protocol.md, docs/memory-reviewer-operations.md, and
docs/properties at HEAD. No tracker issues, no incident history, no git
history lens."

Corrections to [`../fact-sheet.md`](../fact-sheet.md) found while verifying:

- `load_subject_facts` is at `crates/kernel/src/admission.rs:2204-2245`,
  not `:1581-1607` (that range is `prepare_admission`). The claim itself
  holds: the admission row's `source_kind`/`source_id` come from the
  subject object's registry row.
- The "carry a scope and an evidence citation" doc comment is at
  `crates/kernel/src/envelope.rs:1923-1924`, not "about 1870".
- The transport envelope keys are at
  `crates/daemon/src/kernel_routes/mod.rs:461` and the stripping function
  `parse_request_body` at `:463-473`, not `:458-471`.
- The 512 KiB redaction bound is `secret_scanner::MAX_INPUT_BYTES`
  (`crates/secret-scanner/src/api.rs:6`) re-exported as
  `context_core::redaction::MAX_REDACTABLE_BYTES`
  (`crates/context-core/src/redaction.rs:19`); `redact` checks it at
  `crates/kernel/src/redaction.rs:73-78`.
- `derive_classes` was at `commit.rs:242-252` at `3a3b916b7` (PR A tip:
  `:370-382`); the plan's `:228-242` and the fact sheet's `:240-252` (both
  quoted as written at `3a3b916b7`) were off by a few lines.

All other shared-facts citations used below were re-read at HEAD and hold.

## 1. Altitude gate: scope and authority

Deployable: the daemon process and the Kernel SQLite store it owns. Clients:
the OpenCode plugin and the Pi plugin, which share one TypeScript code path
(`packages/opencode-plugin/src/shared/memory-mark-command.ts`, imported by
`packages/pi-plugin/src/commands/eidnara-memory-mark.ts:10-15`).

Owned semantic state: admission history of decisions
(`admission_decisions`), the commit log and receipts, and (after P3)
`use_outcome` rows. Data authority: the Kernel store, written through one
exclusive-writer transaction per `kernel.commit` envelope
(`crates/daemon/src/kernel_routes/commit.rs:1-3, 1043-1071`).

Writer categories known today (FACT):

- Ordinary: the `kernel.commit` route, dispatched at
  `crates/daemon/src/lib.rs:14660`, is the only non-test producer of
  `MarkDisputed` and `Contradict` events (grep over `crates/` for
  `EventKind::MarkDisputed|EventKind::Contradict` outside `tests/` returns
  only `admission.rs` itself).
- Exceptional: the Kernel public API `Envelope::record_admission`
  (`admission.rs:998`) accepts any `AdmissionEvent` from any in-process
  caller and performs no evidence liveness check (`write_admission`,
  `admission.rs:1726-1926`). Kernel integration tests use it
  (`crates/kernel/tests/kernel_admission.rs:885, 2269, 2894`).
- Restore and baseline rotation: a Kernel baseline digest change makes a
  database created under the old baseline refuse to open
  (`crates/storage/AGENTS.md:3`; P3 U1 test scenario). There is no
  migration path for rows written in PR A into the Batch K1 store.

Caller-visible crossings: the bearer-authenticated connection
(`docs/host-wire-protocol.md:26`: the key is the authority; `harness`,
`session`, `role` are claims). The route binds project and harness
server-side (`RouteScope`, `crates/daemon/src/kernel_routes/mod.rs:405-411`)
and strips `session_id` and `project_root` from the body before parsing
(`mod.rs:461-473`).

Routed elsewhere (not decided here): the JSON field names and shapes of the
new wire fields (`/software-design:representation-and-model-boundary-design`
and the root `AGENTS.md` one-PR rule); the SQL statements, transaction
participation, and conflict footprint
(`/software-design:persistence-boundary-and-transaction-design`); Rust and
TypeScript encodings (the design reviewers).

Accepted decisions preserved: KTD1 to KTD4, R1 to R7, the plan's non-goals,
and index D3 and D5. Where evidence shows a decision infeasible as written,
section 12 and `open-questions.md` record the conflict.

## 2. Behavior before representation

Actors:

- Human user, through `/eidnara-memory-mark`. Reporter class derived as
  `(ModelInference, UserInferred)` from `source_kind: "user"`
  (`commit.rs:378`; command sends `sourceKind: "user"` at
  `memory-mark-command.ts:256`). Actor claim `user:opencode` or `user:pi`
  (`eidnara-memory-mark.ts:18`).
- Harness model, through the `eidnara_memory` tool action `dispute` (new).
  Reporter class `(ModelInference, AssistantInference)` from the client's
  default `sourceKind` `"assistant"` (`client.ts:604`; `commit.rs:372-374`).
  Actor claim `agent:<harness>` (R5): OpenCode sends `agent:opencode`
  (`execute.ts:50`) and Pi sends `agent:pi`
  (`pi-plugin/src/tools/eidnara-memory.ts:58`).
- Daemon route: decides admission of the request (fields, evidence, scope),
  derives classes, builds the `AdmissionRequest`.
- Kernel admission: decides the disposition transition, maturity, outcome,
  and writes the row.

Externally meaningful outcomes of one dispute:

1. The subject decision's disposition becomes `Disputed` (or
   `Contradicted`) with effective maturity `Candidate`, so auto-inject stops
   serving it and explicit search labels it (P3 `oq-use-outcome` decision in
   `00-index.md`).
2. An admission row exists that a later reader can follow to the evidence
   (id, or path and oid), to the commit-log actor and cause, and to the
   reporter's derived taint class.
3. A repeat of the same report in the same session replays the receipt
   instead of recording twice.

Forbidden outcomes (named here, enforced in section 7):

- F1: a dispute recorded with no evidence of either kind from a plugin.
- F2: a dispute row whose `evidence_id` was not live at that row's
  `commit_seq`, or an `evidence_id` of another project under the chosen
  visibility rule.
- F3: a dispute that raises historical or effective maturity, or moves the
  disposition toward `Active`, or changes the subject's source or taint
  class.
- F4: a reporter recorded with a class above the one `derive_classes`
  yields for the request's `source_kind`.
- F5: a second dispute with different evidence in the same session refused
  as `operation_key_reused`, or two commits for byte-identical repeats.
- F6: a preview that writes any row, or a commit whose evidence check
  reads a different snapshot than the one it writes against.
- F7: a `use_outcome_id` on a dispute that names another session's row or
  a row served after the dispute.
- F8: a structured `reason` that cannot be parsed back to the typed data it
  encodes, or that disagrees with the typed columns once U3 exists.

## 3. Authoritative state and identities touched

### 3.1 State written by one dispute (PROPOSED set; FACT for the columns
that exist)

| Table | Columns the dispute sets | Status |
|---|---|---|
| `admission_decisions` | `event_kind` (`mark_disputed` or `contradict`), `subject_object_id`, `source_kind`/`source_id`/`source_class`/`taint_class` (the subject's, `load_subject_facts` `admission.rs:2204-2245`), `disposition`, `maturity`, `effective_maturity`, `outcome`, `evidence_id`, `reason`, `commit_seq`, `decided_at` | FACT at `3a3b916b7` except `evidence_id` (always `None` then) and `reason` (constant `"kernel.commit"`, `commit.rs:99`). PR A: a dispute row carries the checked `evidence_id` and the `v=1` reason (`commit.rs:738-739`) |
| `admission_decisions` (U3) | `reporter_source_class`, `reporter_taint_class`, `event_payload`, `use_outcome_id` | PROPOSED, Batch K1 |
| `commit_log` | `producer`, `operation_key`, `request_digest`, `actor`, `cause` (`crates/kernel/src/schema.rs:26`; insert `envelope.rs:1484-1499`) | FACT; no session or source_kind column |
| `operation_receipts` | `(producer, operation_key) -> request_digest, result_payload` (`schema.rs:42`; `stored_receipt` `envelope.rs:1426-1456`) | FACT |
| `durable_text_redactions` | one row per detection in `reason` (`write_admission` `record(...)` call, `admission.rs:1860-1867`) | FACT |

### 3.2 State read by one dispute

- Subject: live `decision` object in the bound project's scope
  (`scoped_object_state` `commit.rs:682-696`; kind and liveness check
  `:713-716`), and its latest admission row (`subject_admission`,
  `:717-719`).
- Evidence (PROPOSED): `evidence_meta` row by `evidence_id` with
  `invalidated_commit_seq IS NULL`, plus the project-visibility predicate
  of section 5 rule A3.
- Receipt: `(producer, operation_key)` lookup before the operation runs
  (`run` `commit.rs:1062-1090`; `preview` `:893-899`).
- Use-outcome (U3, PROPOSED): rows for (bound session, subject) with
  `served_commit_seq` below the committing `commit_seq`.

### 3.3 Identities

| Thing | Identity | Verdict |
|---|---|---|
| Subject decision | `object_id`, existing entity with lifecycle (live, retired, superseded) | Existing; not re-earned here |
| Kernel evidence | `evidence_id` (`evidence_meta` PK, `schema.rs:139`), existing entity with liveness | Existing; not re-earned here |
| Repository evidence reference | `{path, git_object_id}` | PROPOSED value: equality is exact byte equality of both fields after admission (section 5 rule A4). No continuity; a different oid is a different reference. Content-addressed outside the Kernel; the daemon does not verify the blob exists (plan scope). |
| Dispute report | after commit: `admission_decision_id` (`"{commit_seq}:{ordinal}"`, `admission.rs:1742`); before commit: the operation identity `(producer, operation_key, request_digest)` | PROPOSED: NOT EARNED as an entity. A dispute is an immutable appended event; nothing distinguishes "the same dispute" through change. Its idempotent identity is the operation identity (section 5 rule A8). |
| Reporter | `(derived source class, derived taint class, actor claim, cause, bound session, bound harness)` | PROPOSED value stamped at commit. The session and harness are known only at the route (`RouteScope`, `mod.rs:405-411`) and are not persisted by any column today. |

The reporter is a value, not an entity: there is no per-actor principal
(`docs/host-wire-protocol.md:26`; plan non-goal `ExplicitUser`).

## 4. Legal and illegal states

Legal (PROPOSED) for one dispute row `r` with `event_kind` in
`{mark_disputed, contradict}` written through the route:

- L1: `r.evidence_id IS NOT NULL` or `parse(r.reason).evidence_ref` is
  present (U3: `r.event_payload.evidence_ref`). Both may be present.
- L2: if `r.evidence_id IS NOT NULL` then the `evidence_meta` row `e` has
  `e.created_commit_seq < r.commit_seq` and
  (`e.invalidated_commit_seq IS NULL` or
  `e.invalidated_commit_seq > r.commit_seq`). Liveness is as-of the
  dispute's commit, never "now".
- L3: `r.source_class`, `r.taint_class` equal the subject's prior row's
  (FACT: refused otherwise, `evaluate_admission` `admission.rs:284-290`).
- L4: `r.disposition` restrictiveness is at least the prior's
  (`disposition_restrictiveness`, `admission.rs:4241-4248`), or the row is
  recorded `denied` with the prior disposition unchanged
  (`admission.rs:346-350`).
- L5: `r.effective_maturity == Candidate` when `r.disposition` is not
  `Active` (`admission.rs:361-367`); `r.maturity` (historical) equals the
  prior's.
- L6: `r.outcome == deny` (`event_effect`, `admission.rs:4234-4235`).
- L7: reporter taint recorded (PR A: in `reason`; U3: typed) is exactly
  `derive_classes(request.source_kind)` lowered by an asserted class at or
  below it (`resolve_classes`, `commit.rs:413-442`).
- L8 (U3): `r.use_outcome_id` is null or names a `use_outcome` row with
  the same session as the route binding, the same decision as
  `r.subject_object_id`, and `served_commit_seq < r.commit_seq`.
- L9: exactly one `commit_log` row and one receipt exist per accepted
  operation identity; a byte-identical repeat produces no second row.

Illegal states are the negations, one per forbidden outcome F1 to F8.

A row with `evidence_id` whose evidence is later invalidated is LEGAL. The
Kernel's citation predicate that blocks `retire_evidence` excludes
`admission_decisions` (`EVIDENCE_CITED_SQL`,
`crates/kernel/src/slice/write.rs:42-54`), and capture expiry invalidates
evidence rows in place (`crates/kernel/src/cas/deletion.rs:389-392`;
`local_file.rs:3`). The dispute's citation therefore does not pin evidence
retention. This is a derived-state freshness fact for readers (section 9),
and a design question (conflicts file Q7).

## 5. Rules on two axes

Temporal kinds: IA initial admissibility, SP state predicate, TS
transition/history safety, LP liveness. Observation dependencies: INT
intrinsic, CUR current authoritative state, CTX mutable context, DER
derived observation. All rows PROPOSED unless marked FACT.

| Id | Statement | Kind | Deps | Authority | Validity point | Witness | Falsifier |
|---|---|---|---|---|---|---|---|
| A1 | A `mark_disputed` or `contradict` operation on `kernel.commit` carries `evidence_id`, `evidence_ref`, or both; otherwise the envelope is refused by name and nothing is written. Other events keep today's shape (R1). | IA | INT | Daemon route | Body parse / plan build, before any store access | Request with `evidence_ref` only lands | A row with `event_kind=mark_disputed`, `evidence_id NULL`, and no `evidence_ref` in `reason`, written after PR A |
| A2 | A cited `evidence_id` names an `evidence_meta` row with `invalidated_commit_seq IS NULL` as observed inside the committing transaction (KTD2, R2). | SP at commit | CUR | Daemon route, inside the Kernel writer transaction | Commit of the envelope | Dispute citing a fresh capture lands; `admission_decisions.evidence_id` equals it | Row whose evidence `invalidated_commit_seq <= row.commit_seq`, or whose evidence does not exist |
| A3 | A cited `evidence_id` is visible to the bound project. Definition chosen here (recommendation; see conflicts Q1): the evidence is live AND at least one live `decisions`, `observations`, `decision_events` (through its decision), or `asserted_edges` row cites it and carries a `scope_id` that `ScopeFilter::matches` accepts for the bound project. Refusal is `not_found`, the same name as for a nonexistent id. | SP at commit | CUR | Daemon route | Commit | Evidence cited by this project's own decision is accepted | A dispute in project P lands citing evidence cited only by project Q's objects |
| A4 | `evidence_ref.git_object_id` is 40 or 64 lowercase hex bytes (`is_lower_hex_oid`, `crates/kernel/src/scope.rs:710-716`; `pub(crate)` at `3a3b916b7`, PR A: `pub` and re-exported as `kernel::is_lower_hex_oid`, `crates/kernel/src/lib.rs:150`); `evidence_ref.path` is at most 1024 bytes (`MAX_PATH_BYTES = kernel::MAX_TEXT_FIELD_BYTES`, `project_text.rs:37`; `cas/mod.rs:35`), passes `confined_path::check` (`crates/daemon/src/confined_path.rs:29-41`: relative, no `.`/`..`/empty component, at most 16 components, protected components refused), and is unchanged by the redaction scanner. Otherwise refused `invalid_input`. | IA | INT | Daemon route | Plan build | `src/lib.rs@<40 hex>` lands | A row whose `reason` carries a path containing the redaction placeholder, or an uppercase oid |
| A5 | The reporter's recorded taint class equals `derive_classes(source_kind)` lowered only by an asserted class at or below it; the subject row's `source_class`/`taint_class` are the subject's prior (R3, D3). | TS | INT + CUR | Daemon derives (FACT: `resolve_classes` `commit.rs:413-442`); Kernel refuses a subject class change (FACT: `admission.rs:284-290`) | Commit | Command dispute records `user_inferred`; tool dispute records `assistant_inference`; subject classes unchanged | A row recording reporter `explicit_user`, or a subject row whose classes differ from its prior |
| A6 | A dispute never raises historical or effective maturity and never relaxes the disposition (R4, KTD4). FACT today for the event table; PROPOSED that adding `evidence_id` does not change it (`has_evidence` gates only `Enforce`, `admission.rs:291-293`). | TS | CUR | Kernel `evaluate_admission` | Commit | After dispute: `outcome=deny`, `effective_maturity=candidate`, historical unchanged | `outcome=promote` or `effective_maturity` above the prior's after a `mark_disputed` carrying evidence |
| A7 | Filing against a prior whose disposition is more restrictive than the requested one (`Rejected`, `Contradicted`, `Quarantined` for `mark_disputed`; `Quarantined` for `contradict`) is recorded with `denied=true` and the prior disposition kept (FACT for the event table: `admission.rs:337-350`). PROPOSED: the evidence is still written on that row. | TS | CUR | Kernel | Commit | `mark_disputed` on a `Rejected` decision returns `denied: true`, disposition `rejected` | The disposition moves to `disputed` without a valid approval |
| A8 | A command dispute's logical identity is (reporting session, event, subject, canonical evidence key); a tool dispute's is the executor's (session, tool-call id), so a new tool-call id writes the same evidence again and different evidence under one tool-call id is refused (section 7.3). For the command: byte-identical repeats replay the receipt; a different evidence key in the same session is a distinct dispute and lands; the same identity with different bytes is refused `operation_key_reused` (FACT for the mechanism: `stored_receipt` `envelope.rs:1426-1456`; `run` `commit.rs:1083-1091`). | TS | CUR (receipts) | Daemon receipt namespace + Kernel receipt lookup | Receipt lookup at the start of the writer transaction | Two disputes with different `--evidence` in one session both land; the second `--evidence X` repeat replays | Second dispute with different evidence answers `operation_key_reused`; or two `commit_log` rows for one byte-identical repeat |
| A9 | Preview with the new fields performs A1 to A5 identically and writes nothing (FACT for today's preview: no commit-log row, receipt, or token check, `commit.rs:122-126`, `:893-916`). | SP | CUR | Daemon route | Return of the preview | Preview of an invalid evidence id answers the same refusal name as commit would; row counts unchanged | A preview that leaves an `admission_decisions` or `commit_log` row, or that accepts an id the commit refuses |
| A10 | PR A: `reason` is a deterministic rendering of (reporter taint, optional `evidence_ref`) that parses back exactly and contains no redaction placeholder. U3: typed columns are authoritative; `reason` equals their rendering or is retired to a constant. | SP | DER | Daemon route composes; typed columns (U3) win disagreement | Commit (write); read for consumers | `parse(reason) == {taint, ref}` | `parse(reason)` fails, or disagrees with `reporter_taint_class` / `event_payload` |
| A11 | U3: `use_outcome_id` names the `use_outcome` row for (bound session, subject) with the greatest `served_commit_seq` strictly below the dispute's `commit_seq`; null when none exists or P3 is absent. Chosen inside the committing transaction (R6). | SP at commit | CUR | Daemon route reading Kernel rows in the same transaction | Commit | Dispute from a session served the decision at seq n lands at seq n+k with the link set | Link names another session's row, or a row with `served_commit_seq >= commit_seq` |
| A12 | The evidence check and the admission write observe one snapshot: both run inside the `commit_before` closure (FACT: `commit.rs:1062-1090`). | TS | CUR | Kernel exclusive writer | Commit | Concurrent `retire_evidence` serializes before or after the whole envelope | A row citing evidence invalidated by a commit with a lower `commit_seq` |
| A13 | The actor string is a claim and never decides class or admission (FACT: `docs/host-wire-protocol.md:26`; `commit.rs:5-10`). PROPOSED: the route may check `agent:<harness>` against `RouteScope.harness` and refuse a mismatch as `invalid_input`, but the reporter class still comes from `source_kind`. | IA | INT + CTX (binding) | Daemon route | Plan build | Pi tool sending `agent:pi` under a Pi binding lands | A row whose reporter class was derived from the actor prefix |
| A14 | Progress: an envelope that passes admission commits or returns a definite refusal within the route deadline (`DEFAULT_DEADLINE` 5 s, `MAX_DEADLINE` 30 s, `commit.rs:88-89`); the deadline bounds writer acquisition only (`:90-92`). | LP | CTX | Kernel writer lease | Return | Commit returns with a receipt | Writer held past the deadline by a prior envelope; caller receives `unavailable` with nothing written |

"Always" is replaced by the validity point in each row. L2's as-of form is
what makes A2 a commit-time predicate rather than a lifetime fact; evidence
can be invalidated later without violating any rule here.

## 6. Commands and queries

### 6.1 Command: FileDispute (`kernel.commit`, `preview: false`)

- Actor / authority: plugin on a bearer-authenticated bound route. Daemon
  decides admission of the request; Kernel decides the transition.
- Intent / admission: `Operation::Disposition { object_id, event,
  approval_object_id?, evidence_id?, evidence_ref? }` with `event` in the
  five-event table; A1 and A4 checked before store access;
  `deny_unknown_fields` keeps misspelled fields out (`commit.rs:106-107,
  :137`).
- Observations: receipt lookup for `(producer, operation_key)`; subject
  object state and scope; subject's latest admission; evidence liveness
  and visibility (A2, A3); (U3) `use_outcome` rows. All read inside the
  writer transaction at the committing snapshot.
- Guard: A1 to A5, A8 (no digest mismatch), subject live and in scope,
  approval chain in scope when cited (FACT: `commit.rs:720-725`).
- Authoritative change: one `admission_decisions` row; one `commit_log`
  row; one receipt; redaction rows for `reason` detections; no change to
  the subject object row or its token (FACT: test
  `a_disposition_leaves_the_object_token_valid`,
  `crates/daemon/tests/kernel_routes.rs:4947`).
- Postcondition: L1 to L9.
- Effects outside the transaction: none. Serving surfaces read the new row
  on their next query; no push.
- Outcomes (certainty / cause / caller action):
  - Definitely applied / none / stop. Response carries
    `receipt.commit_seq`, `replayed: false`, and the verdict
    (`outcome`, `previous_disposition`, `disposition`, `denied`).
  - Definitely applied (earlier) / replay / stop. `replayed: true`. A
    replayed preview returns `previews: []` with the receipt
    (`commit.rs:1199-1206`); a replayed commit returns the stored
    `dispositions` (`commit.rs:1224-1240`, asserted equal to the first
    response at `kernel_routes.rs:6114-6121`), so the TS command reports
    the stored verdict with `replayed: true`. Its `outcome_unknown`
    branch (`memory-mark-command.ts:299-300`) covers a commit response
    that omits the verdict (Q8).
  - Definitely not applied / domain rejection (`invalid`:
    `invalid_input` for A1, A4; `not_found` for subject or evidence
    liveness or visibility; `admission_policy` for a subject without an
    admitted prior or a Kernel class refusal) / stop, correct the input.
  - Definitely not applied / conflict (`invalid` `operation_key_reused`)
    / stop; the client's identity derivation is wrong (see A8).
  - Definitely not applied / unavailable (writer deadline, store closed)
    / retry with the same identity; replay is safe because the receipt
    lookup precedes the operation.
  - Unknown / transport loss after the writer committed / safe replay
    with the same identity, then read `replayed`.
  - Recorded but `denied: true` / policy (A7) / stop; the evidence is on
    the row, the disposition did not move.

### 6.2 Command-like query: PreviewDispute (`preview: true`)

Same admission and guards as FileDispute, no write (A9). It is a query by
effect but it reads the receipt table and the admission state, so its
answer expires as soon as another envelope commits (FACT: the TS command
accepts that the commit verdict may differ from the preview,
`memory-mark-command.ts:301`). Consumers: the slash command's confirmation
step only. Prohibited decision: a preview result never authorizes skipping
any check at commit.

### 6.3 Internal queries inside the transaction

| Query | Source / authority | Freshness | Absence meaning | Prohibited decisions |
|---|---|---|---|---|
| Evidence liveness (A2) | `evidence_meta` in the writer snapshot | Exact at `commit_seq` | `not_found` | Must not be answered from a client cache or an earlier preview |
| Evidence project visibility (A3) | citing scoped rows + `ScopeFilter::matches` (`project.rs:199-206`) | Exact at `commit_seq` | `not_found` (same name as nonexistence, closing the existence oracle) | Same |
| Use-outcome row (A11) | `use_outcome` (P3) | Exact at `commit_seq` | null link | Must not substitute a row from another session or a later serve |
| Stored receipt (A8) | `operation_receipts` | Exact | proceed | A digest mismatch is a refusal, never a second commit |

### 6.4 Reader queries (consumers of the dispute)

| Query | Authority | Provenance | Freshness | Consumers | Prohibited decisions |
|---|---|---|---|---|---|
| "Was decision D disputed after it was served to session S?" (P3 R3) | `admission_decisions` joined by `subject_object_id` and `commit_seq` | `commit_seq` of the row | As of the reader's `known_as_of` | P3 read API, operator observability | Must not infer reporter identity from `source_kind`/`source_id` (those are the subject's) |
| "What evidence did dispute r cite, and is it still live?" | `admission_decisions.evidence_id`, `reason`/`event_payload`, `evidence_meta.invalidated_commit_seq` | row `commit_seq`; evidence `invalidated_commit_seq` | Two answers: live at `r.commit_seq` (always true by L2) and live now (may be false) | Operator, later reviewer | Must not present "live now" as "live when filed" or the reverse |
| "Who filed r?" | `commit_log.actor`, `commit_log.cause` via `commit_seq`; reporter taint from `reason` (PR A) or typed columns (U3) | row `commit_seq` | Exact | Operator | Must not treat `actor` as authenticated identity; the session is not recoverable from `operation_key` (a hash, `client.ts:284-290`) and no column holds it (conflict C2) |

## 7. Mutation authority and enforcement sets

Who may file: any holder of the bearer key through either plugin (FACT:
`docs/host-wire-protocol.md:26`). The daemon has no per-actor principal;
"user" versus "agent" is the `source_kind` claim mapped to a taint class
(`commit.rs:370-380`). A model may only lower, never raise, the derived
class (`resolve_classes`, `:413-442`).

What the daemon decides (PROPOSED): A1, A3, A4, A8 namespace, A9, A10
rendering, A11 link selection, A13. A2 is a daemon check inside the Kernel
transaction (KTD2).

What the Kernel decides (FACT): subject class invariance, approval need,
disposition and maturity result, outcome, redaction of `reason`,
`evidence_id` identity-safety (`identity`, `admission.rs:1750-1755`),
non-blank `reason` (`admission.rs:1616-1618`), receipt replay versus digest
conflict.

### 7.1 Enforcement set for A2 and A3 (evidence live and visible)

- Winning authority: `evidence_meta` and the citing scoped rows in the
  writer snapshot.
- Observed decision state: evidence row liveness, citing rows' scope ids,
  the project binding's scope terms.
- Advisory checks: none on the client (the client sends ids as typed by
  the user or model).
- Authoritative mechanism: the check runs inside the `commit_before`
  closure that writes the row (`commit.rs:1062-1090`), so the Kernel's
  exclusive writer serializes it against `retire_evidence` and capture
  expiry.
- Ordered set: receipt lookup; token checks; per operation: subject state
  and scope; subject admission; evidence check; `record_admission`.
- Ordinary writers: the route (`record_disposition`, `commit.rs:789-814`).
- Exceptional writers: `Envelope::record_admission` from Kernel tests and
  any future daemon code; these bypass A2 and A3 entirely because the
  Kernel does not check evidence liveness on this path (FACT:
  `write_admission` only runs `identity` on the id, `admission.rs:1750`).
  Closure test 1 fails for the Kernel API. Conflicts file Q5 asks whether
  the liveness check belongs in the Kernel instead.
- Commit point: the envelope's commit.
- Abort / conflict: whole envelope not applied; the client may retry
  after correcting the id.
- Unknown result: transport loss after commit; safe replay by identity.
- Progress: bounded by the writer lease deadline.
- Witness and falsifier: rule table rows A2, A3.
- Evidence status: proposed from design.

### 7.2 Enforcement set for A6 (tightening only)

Existing Kernel mechanism (`evaluate_admission`, `admission.rs:280-404`),
covered by `disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`
(`kernel_routes.rs:4398`) and
`a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one`
(`:4458`). The dispute adds `evidence_id` to the event; the only evidence
gate in the evaluator is for `Enforce` (`:275-277`), so the new field has
no lever on maturity. Writer closure: every disposition path passes
through `evaluate_admission` (`record_admission_inner` `:1041`,
`preview_admission` `:1859`). Status: FACT for the mechanism; PROPOSED
that a test asserts the maturity row after a dispute that carries
evidence.

### 7.3 Enforcement set for A8 (one dispute, one identity)

- Authority: `operation_receipts` under the project-prefixed key
  (`ProjectBinding::operation_key`, `project.rs:78-83`).
- Mechanism: `stored_receipt` before the operation (`envelope.rs:1363-
  1372`); `run` maps a conflict raised before entering the closure to
  `OperationKeyReused` (`commit.rs:1083-1091`).
- Ordinary writers: both plugins through `deriveOperationKey(producer,
  actor, operationId)` (`client.ts:284-290`) with `operationId =
  [sessionId, event, objectId, ...evidence flags]` for the command
  (`memoryMarkOperationId`, `memory-mark-command.ts:210-215`) and
  `sessionId SEP toolCallId` for the tool (`execute.ts:430-432`).
- Gap at `3a3b916b7`, closed by PR A: the command's `operationId` omitted
  the evidence, so a second dispute in one session with different evidence
  changed the `request_digest` (`deriveRequestDigest` covers operations,
  `client.ts:272-281`) but not the key, and the daemon refused
  `operation_key_reused`. PR A appends the evidence flags to the identity;
  `memory-mark-command.test.ts:212` lands two disputes with different
  evidence in one session as two commits and replays a repeat of the first.
  The tool's key is per tool call, so it never had the collision; its
  replay unit is the tool call.
- Identity (applied in PR A): the command's `operationId` is
  `[sessionId, event, objectId, evidence flags]`, the flags being
  `--evidence <id>` and `--path <path>@<oid>` in a fixed order after A4
  admission (`evidenceFlags`, `memory-mark-command.ts:152-162`). Each field
  must be free of U+001F (`joinKeyFields`, `client.ts:247-255`); A4's path
  bounds exclude NUL but not U+001F, so the parser refuses it in the object
  id, the evidence id, and the path (`memory-mark-command.ts:124-129`,
  `:80`).
- Consequence: the replay unit is "one dispute per (session, event,
  object, evidence)", where the doc comment at `3a3b916b7` described "one
  dispute per (session, event, object)". The logical intent of "one
  dispute" is "this session asserts this evidence against this finding";
  two pieces of evidence are two assertions and two rows, each a tightening
  that the Kernel accepts (A7: a repeat on an already `Disputed` decision
  records `outcome=deny`, disposition unchanged, `admission.rs:337-383`).
  The alternative (keep the key, refuse the second evidence) is recorded in
  conflicts Q2.

### 7.4 Enforcement set for A10 and A11 (derived reason, use-outcome link)

See section 9. Both are route-composed values written inside the same
transaction; no other writer composes them. The structured `reason` passes
through `redact`, which rewrites secret-like substrings (lossy,
`redaction.rs:41, 73-78`); A4's redaction-stability requirement on `path`
and the Kernel's `identity` on `evidence_id` keep the rendering stable.
`local_file.rs:143` already applies this rule to captured paths ("a stored
path must equal the path the run named").

## 8. Transitions

State of the subject decision as the dispute sees it: `(disposition,
historical maturity, effective maturity, source_class, taint_class)`.
Only the disposition and effective maturity move.

| Current disposition | Command / actor | Guard | Next / postcondition | Illegal example |
|---|---|---|---|---|
| `Active` | `mark_disputed` with evidence, any reporter | A1 to A5, subject live and in scope | `Disputed`, effective `Candidate`, `outcome=deny`, `denied=false` | `effective_maturity` stays above `Candidate` |
| `Active`, `Stale`, `Disputed`, `Superseded`, `Rejected` | `contradict` with evidence | same | `Contradicted`, effective `Candidate` | Moves to `Contradicted` with no evidence on the row |
| `Stale`, `Superseded` | `mark_disputed` | same | `Disputed` (same restrictiveness 1, not a relaxation, `admission.rs:4241-4248`) | refused as relaxation |
| `Disputed` | `mark_disputed` (second evidence) | same | `Disputed` unchanged; new row with the new evidence; `outcome=deny` | Second row refused `operation_key_reused` (A8) |
| `Rejected`, `Contradicted`, `Quarantined` | `mark_disputed` without approval | same | unchanged; row recorded with `denied=true`, `outcome=deny`; evidence on the row (A7) | disposition relaxes to `Disputed` |
| `Quarantined` | `contradict` without approval | same | unchanged; `denied=true` | relaxes to `Contradicted` |
| any | `mark_stale`, `explicit_reject`, `quarantine` | today's guards (R1) | today's results | refused for lacking evidence |
| retired or foreign subject | any | subject check fails | refused `not_found`; nothing written | row written against a retired decision |
| subject with no admitted prior | any | `subject_admission` is `None` | refused `admission_policy` (FACT `commit.rs:717-719`) | row written |

A state-transition model is earned for the subject disposition (history
changes legality: relaxation needs approval). It already exists in the
Kernel; this plan adds no new state and no new transition. The dispute
record itself has no lifecycle: NOT EARNED.

## 9. Derived-state contracts

### 9.1 Preview versus commit

| Field | Content |
|---|---|
| Derivative | The preview verdict and per-surface visibility delta |
| Base authority | The Kernel snapshot at preview time |
| Freshness | Expires at the next commit touching the subject, its approvals, or the evidence |
| Allowed consumers | The command's confirmation prompt |
| Prohibited decisions | Skipping A1 to A5 at commit; reporting `applied` from a preview |
| Mismatch | The commit's verdict wins (FACT: `memory-mark-command.ts:301-302`) |

### 9.2 Structured `reason` (PR A) as a projection of typed data

| Field | Content |
|---|---|
| Derivative and purpose | `admission_decisions.reason` carries reporter taint and optional `evidence_ref` until Batch K1 |
| Base authority | PR A: the route's typed in-memory values at commit; there is no other copy, so the rendering is the only persisted form. U3: `reporter_source_class`, `reporter_taint_class`, `event_payload` |
| Derivation | A fixed grammar chosen by the implementer (handoff), restricted to the identity-safe alphabet so `redact` is the identity on it; version-tagged so U3 can recognize PR A rows |
| Visible freshness | Exact (written in the same row) |
| Consumers | P3's join, operator reads, the U3 implementer's parser |
| Prohibited decisions | None of the Kernel's admission logic reads `reason` (FACT: only `redact` and `record` touch it, `admission.rs:1749, 1787-1794`); it must stay that way. A reader must not derive authority (class, approval) from `reason` |
| Mismatch detection | U3: `render(typed) == reason` check in tests; a mismatch is a defect, typed columns win |
| Rebuild / cutover | None needed: Batch K1 rotates the Kernel baseline and the old database refuses to open (`crates/storage/AGENTS.md:3`), so PR A rows are not carried into the U3 store by any supported path (conflicts Q6) |
| Retirement | After U3, `reason` may return to a constant or keep the rendering; either way it is derived |

### 9.3 Use-outcome link freshness (U3)

| Field | Content |
|---|---|
| Derivative | `admission_decisions.use_outcome_id` |
| Base authority | `use_outcome` rows (P3) at the dispute's `commit_seq` |
| Derivation | Row for (bound session, subject) with greatest `served_commit_seq < commit_seq` |
| Freshness | Exact at commit; later serves to the same session do not update it (append-only admission rows) |
| Consumers | P3 R3 read ("disputed after serve"), P7 counterfactuals |
| Prohibited decisions | The link never changes disposition, maturity, or class; absence means "not served to this session before the dispute, or P3 absent", not "no dispute" |
| Mismatch | The P3 sweep keeps rows a live dispute cites (P3 U1 approach step 2); a dangling `use_outcome_id` after a sweep is a defect in the sweep, not in the dispute |

### 9.4 Evidence citation liveness (readers)

| Field | Content |
|---|---|
| Derivative | The reader's view "dispute r rests on evidence e" |
| Base authority | `evidence_meta.invalidated_commit_seq` |
| Freshness | L2 guarantees liveness at `r.commit_seq` only. `EVIDENCE_CITED_SQL` (`write.rs:42-54`) excludes `admission_decisions`, so `retire_evidence` and capture expiry can invalidate `e` later |
| Consumers | Operator reads; P3 join |
| Prohibited decisions | A reader must not treat a dispute whose evidence is now retired as never filed, and must not treat it as currently substantiated without checking `invalidated_commit_seq` |
| Open | Whether a dispute's citation should pin evidence (conflicts Q7) |

### 9.5 Serving projections

Served visibility and sensitivity are computed from the admission chain
(`served_rows_for`, `admission.rs:1037`; served sensitivity folding reads
`decision_events` with `memory_reviewer_support`, `admission.rs:3507-3523`,
not `admission_decisions.evidence_id`). PROPOSED: the dispute's
`evidence_id` has no effect on served sensitivity or visibility beyond the
disposition change the event already causes. Falsifier: a served row's
sensitivity changes after a dispute whose only new input is `evidence_id`.

## 10. Persistence and concurrency obligations (handoff)

Required of the topology, not chosen here:

- A2, A3, A11, and the row write share one outcome and one snapshot.
  Today's `commit_before` closure provides this; the obligation is that
  the evidence and use-outcome reads happen inside it, not in a preflight.
- Partial results forbidden: no `commit_log` row without its
  `admission_decisions` row and receipt, and the reverse.
- Falsifying interleaving (model-relative): T1 reads `e` live in a
  preflight outside the writer; T2 retires `e` and commits; T1 enters the
  writer and writes `r` citing `e` with `r.commit_seq > e.invalidated`.
  Running the check inside the closure removes it.
- Writer participation: the Kernel API path (`record_admission`) does not
  participate in A2/A3; either close it in the Kernel or document it as a
  trusted in-process bypass (conflicts Q5).
- Retry: definite refusals are terminal for the same input; `unavailable`
  is replay-safe by identity; unknown outcomes replay by identity and read
  `replayed`.

Route to `/software-design:persistence-boundary-and-transaction-design`
for the SQL, the read set, and the conflict footprint.

## 11. Structure and variation inputs

- Hidden decision: the project-visibility predicate for evidence (A3). It
  is the one difficult decision; clients need only the refusal name.
  Alternatives recorded in conflicts Q1. Topology (where the predicate
  lives, route or Kernel) routes to
  `/software-design:dependency-direction-and-effect-boundaries`.
- Hidden decision: the `reason` grammar (A10), protected clients: the U3
  parser and operator reads. Removal condition: Batch K1 lands.
- Variation point: harness in the tool actor (`agent:<harness>`). Present
  selection (two harnesses), selected at plugin build time, owner: each
  plugin. The daemon already knows the harness from the binding
  (`mod.rs:410`); A13 proposes the check. NOT EARNED as a runtime
  abstraction; a constant per plugin suffices.
- Variation point for evidence kind (`evidence_id` versus
  `evidence_ref`): a closed two-alternative value, both present now.
  Mechanism choice routes to
  `/software-design:abstraction-and-variation-point-design`.
- Labelled edges the model requires: plugin -> route (calls, dataflow:
  operation and evidence); route -> Kernel (calls inside one transaction;
  mutates `admission_decisions` through `record_admission` only); route ->
  `evidence_meta`, citing rows, `use_outcome` (observes, at commit
  snapshot); readers -> `admission_decisions` + `commit_log` (observes).
  No mutation-authority edge from any projection into admission.

## 12. Conflicts with accepted decisions (summary; details in
open-questions.md)

- C1 (R2 as written): "visible to the caller's project scope" has no
  direct meaning for evidence objects (`ScopeFilter::matches(None)` is
  false, `project.rs:204-206`; `envelope.rs:1923-1924`). A3 defines it by
  citing scoped rows. Needs confirmation.
- C2 (R3, U1 verification): the commit log carries `actor` and `cause`
  only (`schema.rs:26`); `session_id` and `source_kind` are not persisted,
  and the session is not recoverable from `operation_key`. "The receipt and
  commit log carry actor, cause, and session" is infeasible without a new
  column (U3) or a derivation the plan does not name.
- C3 (D5 versus R5): D5 says model output never names an evidence id; the
  tool action `dispute` accepts `evidence_id` from the model. Validation
  (KTD2) is the plan's answer; the wording conflict needs a human.
- C4 (R5 `agent:<harness>`): before #1161 both harnesses sent
  `agent:opencode`; #1161 keeps it for OpenCode (`execute.ts:50`) and
  gives Pi `agent:pi` (`pi-plugin/src/tools/eidnara-memory.ts:58`). The
  daemon can verify against the binding. Decide whether to verify.
- C5 (A8): at `3a3b916b7` the command's operation identity collided on a
  second evidence. PR A applied the section 7.3 identity
  (`memoryMarkOperationId`, `memory-mark-command.ts:210-215`); closed.
- C6 (U3 rebuild): PR A rows do not survive the Batch K1 rotation under
  single-baseline storage; no migration exists or is allowed.

## 13. Falsifiers (one per load-bearing rule)

Listed in the rule table (section 5) and repeated as records in
`proposed-obligations.md`. Analysis bounds: every falsifier is a single
row or a two-step interleaving over the tables named; no model checking was
run; writer closure for the Kernel API path is unverified by construction
(section 7.1).

## 14. Handoffs

- Wire shape and JSON names for `evidence_id`, `evidence_ref`, the tool
  `dispute` action, and the command flags: representation skill plus the
  root `AGENTS.md` one-PR rule for Rust and TypeScript peers.
- SQL and transaction placement of A2, A3, A11: persistence-boundary
  skill.
- Where the visibility predicate and `is_lower_hex_oid` (currently
  `pub(crate)` in the Kernel) are exposed: dependency-direction skill and
  `/design-review:rust-design-review`.
- TypeScript parsing of `--evidence` and `--path p@oid`, identity change,
  replay handling: `/design-review:typescript-design-review`.
- Test oracles for the records: `/testing:invariant-test-review` after
  implementation; the records in `proposed-obligations.md` are the
  candidate catalog entries.

## 15. Output certificate

```text
SCOPE AND AUTHORITY: daemon + Kernel store; plugins are bearer-authenticated
  claimants; daemon admits requests and derives classes; Kernel decides
  transitions; wire/SQL/language routed.
BEHAVIOR: FileDispute, PreviewDispute; witnesses and forbidden outcomes F1-F8.
RULES: A1-A14 with temporal kind, dependencies, validity point (section 5).
IDENTITY AND VALUES: subject and evidence entities existing; evidence_ref a
  value; dispute report NOT EARNED as entity (operation identity + row id);
  reporter a stamped value.
TRANSITIONS: existing Kernel disposition table; dispute adds data, no state.
PRESERVATION: enforcement sets 7.1-7.4; Kernel API path is an open bypass
  for A2/A3.
COMMAND OUTCOMES: certainty/cause/action in 6.1.
QUERY CONTRACTS: 6.3 internal, 6.4 readers.
PERSISTENCE AND CONCURRENCY: one snapshot per envelope; checks inside the
  writer closure; replay by identity.
DERIVED STATE: preview, structured reason, use-outcome link, citation
  liveness, serving projections (section 9).
STRUCTURE INPUTS: visibility predicate and reason grammar as hidden
  decisions; harness actor NOT EARNED as abstraction.
VARIATION / ASSEMBLY: closed two-kind evidence value; no assembly change.
FALSIFIERS: section 5 table; bounds in section 13.
HANDOFF: section 14; open conflicts C1-C6 and questions in
  open-questions.md.
```
