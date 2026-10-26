# Property catalog: dispute evidence on kernel.commit

System: the Eidnara repository at the PR A branch tip of
[#1160](https://github.com/ahrav/eidnara/issues/1160) (base `6159fd224`,
code commit `6c4d445e3`). Method: [`../../METHOD.md`](../../METHOD.md).
Companion files: [domain-model.md](domain-model.md) (the design-time
domain model), [certificate.md](certificate.md),
[open-questions.md](open-questions.md), [existing-checks.md](existing-checks.md),
[fault-map.md](fault-map.md),
[portfolio-evaluation.md](portfolio-evaluation.md), `evidence/<slug>.md`.

## Scope

The evidence a `mark_disputed` or `contradict` disposition carries on
`kernel.commit` and the checks the daemon runs on it:

- the route fields `evidence_id` and `evidence_ref`, the reason
  composition, and the evidence visibility check in
  `crates/daemon/src/kernel_routes/commit.rs`; the reference form rules and
  the `v=1` reason grammar in `crates/daemon/src/kernel_routes/dispute.rs`;
- the Kernel read `Envelope::evidence_citations`
  (`crates/kernel/src/envelope.rs:532-577`) and the exported
  `kernel::is_lower_hex_oid` (`crates/kernel/src/scope.rs:710-716`);
- the shared command runner
  `packages/opencode-plugin/src/shared/memory-mark-command.ts`, the client
  type `DispositionOperation`
  (`packages/opencode-plugin/src/shared/kernel-client/client.ts:101-111`),
  and both command callers.

The normative text for the route is the `commit.rs` module header section
"Disputes" (`commit.rs:12-63`). It is a claim under test like every other
document.

Nine records are active and checked by the PR A tests. One record,
`use-outcome-link-names-own-session-prior-serve`, stays a proposed
obligation for PR B ([#1162](https://github.com/ahrav/eidnara/issues/1162));
it keeps the design-time `Lifecycle:`, `Falsifier:`, and `Legal witness:`
lines and has no evidence file until PR B makes it checkable. The model
tool arm of the dispute surface lands with
[#1161](https://github.com/ahrav/eidnara/issues/1161); the records that
name both surfaces cover the command arm only and say so.

## Reachability classes

Every active record is `default-production`. The shared evidence is one
fact: the daemon's method match dispatches `"kernel.commit"` to
`handle_kernel_commit` with no `#[cfg]` attribute
(`crates/daemon/src/lib.rs:14529`), and the route parses the evidence
fields on every disposition operation (`commit.rs:155-172`). The command
that sends them is registered in both harnesses
(`packages/opencode-plugin/src/hooks/context/command-handler.ts:413-419`,
`packages/pi-plugin/src/index.ts:550`). Each record restates its own
evidence.

## Index

| Slug | Type | Reachability | Status | Exercised | Semantics |
|---|---|---|---|---|---|
| `dispute-requires-evidence-from-plugin` | safety | default-production | active | partial | always |
| `cited-evidence-live-at-dispute-commit` | safety | default-production | active | yes | always |
| `cited-evidence-visible-to-bound-project` | safety | default-production | active | yes | always |
| `evidence-ref-well-formed-and-redaction-stable` | safety | default-production | active | yes | always |
| `reporter-class-derived-never-asserted-up` | safety | default-production | active | partial | always |
| `dispute-with-evidence-never-raises-standing` | safety | default-production | active | yes | always |
| `dispute-identity-includes-evidence-key` | safety | default-production | active | yes | always |
| `dispute-preview-writes-nothing` | safety | default-production | active | yes | always-or-unreached |
| `structured-reason-projects-typed-reporter-data` | safety | default-production | active | partial | always |
| `use-outcome-link-names-own-session-prior-serve` | safety | default-production (PR B) | active (proposed) | not yet | always-or-unreached |

Semantics distribution: 8 `always`, 2 `always-or-unreached`, 0
`sometimes`, 0 `reachable`, 0 `unreachable`. The enabling states that
keep these from passing vacuously are listed in
[fault-map.md](fault-map.md).

## Records

### dispute-requires-evidence-from-plugin

Type: safety
Reachability: default-production - `lib.rs:14529` dispatches
`kernel.commit`; `compose_reason` runs for every operation of every
request after class resolution and before the preview or commit
transaction (`commit.rs:1119-1125`).
Status: active
Exercised: partial - command arm: route test
`a_dispute_needs_evidence_and_other_events_take_none`
(`crates/daemon/tests/kernel_routes.rs:5971`) sends `mark_disputed` and
`contradict` with neither field and with an empty `evidence_id`, and
`mark_stale` with an `evidence_ref`, and asserts `invalid_input` from the
preview and the commit with the four row counts unchanged; it lands
`mark_stale`, `explicit_reject`, and `quarantine` without evidence with
`reason = "kernel.commit"` and `evidence_id IS NULL`. The command parser
test `a dispute without evidence, and evidence on another event, print
usage` (`memory-mark-command.test.ts:126`) covers the TypeScript side. The
model tool arm sends no disposition at this revision and lands with #1161.
Guarantee: A `mark_disputed` or `contradict` operation that reaches the
Kernel writer through `kernel.commit` carries a non-empty `evidence_id`,
an `evidence_ref`, or both; one with neither, or with an empty id, is
refused `invalid_input` before any transaction runs; every other event
carries neither field and records the constant reason `kernel.commit`.
Check: `always` - for every `admission_decisions` row with `event_kind IN
('mark_disputed','contradict')` whose `reason` parses as a `v=1`
`DisputeReason` (`dispute.rs:106-146`), `evidence_id IS NOT NULL OR
parse(reason).evidence_ref IS NOT NULL`; and for every route-written row
of the other three disposition events, `reason = 'kernel.commit' AND
evidence_id IS NULL`. The predicate holds on every stored row, so
`always`. The `v=1` prefix is the route's marker: Kernel API callers of
`record_admission` write rows outside it (`commit.rs:55-58`).
Fault/timing angle: none; the check is a parse-time predicate that runs
before the writer is entered.
Required faults and enabling state: a body with a dispute and no evidence
field; a body with an empty `evidence_id`; a non-dispute event carrying
either field; a client that bypasses the TypeScript parser and sends the
raw body (the route tests do this).
Confidence: high - [evidence](evidence/dispute-requires-evidence-from-plugin.md).
Read `compose_reason` (`commit.rs:206-234`), `DisputeReason::validated`
(`dispute.rs:49-79`), the handler loop (`commit.rs:1119-1125`), and the
route and parser tests above.
Existing check: `kernel_routes.rs:5971`; `dispute.rs:339`
(`a_dispute_needs_evidence_and_other_events_take_none`, unit);
`memory-mark-command.test.ts:126`; the fake kernel mirrors the rule
(`disputeEvidenceWellFormed`,
`packages/opencode-plugin/src/shared/kernel-client-testing/fake-kernel.ts:269-287`).
Status unaudited.
Impact: a dispute with nothing to check lowers a finding's standing on
assertion alone, and a later reader has no evidence to follow.
Open questions:
- The check cannot see Kernel API writers: `record_admission` accepts a
  `mark_disputed` without evidence from any in-process caller
  (`commit.rs:55-58`). No production caller other than this route writes
  the two dispute events at this revision (searched
  `crates/daemon/src`, `crates/kernel/src`, and `packages/*/src`). Whether
  the Kernel should enforce the requirement itself stays open. (needs
  human input)

### cited-evidence-live-at-dispute-commit

Type: safety
Reachability: default-production - `disposition_request` calls
`require_visible_evidence` for every dispute that names an `evidence_id`
(`commit.rs:719-721`), in the commit's writer closure
(`commit.rs:1047-1062`) and in the preview transaction
(`commit.rs:877-897`).
Status: active
Exercised: yes - `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing`
(`kernel_routes.rs:5801`) retires an evidence row through
`retire_evidence` and cites it under both dispute events; preview and
commit answer `not_found` and the four row counts are unchanged.
`a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter`
(`kernel_routes.rs:5723`) lands a live id and reads it back from
`admission_decisions.evidence_id`. The Kernel read's liveness bit is
covered by `evidence_citations_name_the_scopes_of_live_citers_and_the_rows_liveness`
(`crates/kernel/tests/kernel_slice.rs:2159`).
Guarantee: A dispute row's `evidence_id`, when present, names an
`evidence_meta` row whose `invalidated_commit_seq` is NULL in the same
writer transaction that writes the row; a retired or nonexistent id is
refused `not_found` with nothing written.
Check: `always` - for every row `r` with `r.evidence_id IS NOT NULL`,
`event_kind IN ('mark_disputed','contradict')`, and a `v=1` reason, the
joined `evidence_meta` row `e` satisfies `e.invalidated_commit_seq IS NULL
OR e.invalidated_commit_seq > r.commit_seq`. The check is stated as of the
row's commit because a citation holds no retention: evidence may be
retired later without fault (`commit.rs:59-60`). Holds on every write, so
`always`.
Fault/timing angle: a retirement ordered between a client's read and the
commit. The read (`Envelope::evidence_citations`, `envelope.rs:536-577`)
runs inside the commit's exclusive writer transaction, so a retirement
either commits before it (refused) or after the dispute commits (the
as-of predicate holds). Not constructed: a retirement queued in the same
writer queue as the dispute. The store admits one writer at a time, so a
test needs a second writer blocked on the lock at the moment the dispute
holds it; no test builds that interleaving, and the ordering argument
above is read from code, not observed.
Required faults and enabling state: an ingested evidence row retired
before the dispute; a never-ingested id; a live cited id for the positive
case.
Confidence: high - [evidence](evidence/cited-evidence-live-at-dispute-commit.md).
Read `require_visible_evidence` (`commit.rs:738-759`),
`evidence_citations` (`envelope.rs:532-577`), and the two route tests.
Existing check: `kernel_routes.rs:5801`, `:5723`; `kernel_slice.rs:2159`.
Status unaudited.
Impact: a dispute resting on retired evidence cannot be followed by a
later reader at the moment it was filed.
Open questions:
- The Kernel admission write still checks existence only
  (`admission-evidence-citation-is-existence-checked-only` in
  [`../disposition-path/catalog.md`](../disposition-path/catalog.md)); a
  Kernel API writer can cite a retired row. Whether the check belongs in
  `write_admission` stays open. (needs human input)

### cited-evidence-visible-to-bound-project

Type: safety
Reachability: default-production - same call path as the liveness record
(`commit.rs:719-721`).
Status: active
Exercised: yes - `kernel_routes.rs:5801` cites a live id that nothing
cites, a live id cited only by an unscoped decision, and a live id cited
only by a decision of a second bound project; each answers `not_found`
from the preview and the commit with nothing written, under both dispute
events. `kernel_routes.rs:5723` lands an id cited by a decision of the
bound project. `kernel_slice.rs:2159` asserts which citers contribute a
scope: live decisions, live observations, and decision events through a
live decision; unscoped and retired citers contribute none.
Guarantee: A dispute's `evidence_id` is accepted only when a live
decision, observation, decision event (through its live decision), or
asserted edge whose scope the bound project's filter accepts cites it;
otherwise the request is refused `not_found`, the same name a nonexistent
id gets.
Check: `always` - for every dispute row `r` with `r.evidence_id`, at
`r.commit_seq` at least one of `EvidenceCitations.citing_scopes`
(`envelope.rs:187-194`) passed `ScopeFilter::matches` for the bound
project (`commit.rs:751-757`). Every refusal case (unknown, retired,
uncited, unscoped-only, foreign-only) answers the same `not_found` with
row counts unchanged, so a refusal reveals nothing about another
project's evidence. Holds on every request, so `always`.
Fault/timing angle: the only citing row retired between a client's read
and the commit; the read runs in the writer transaction, as in the
liveness record.
Required faults and enabling state: two bound projects in one Kernel
store; evidence cited only by the other project; evidence cited only by
an unscoped row; an uncited live row.
Confidence: high - [evidence](evidence/cited-evidence-visible-to-bound-project.md).
Read the SQL in `evidence_citations` (`envelope.rs:553-566`), the filter
loop (`commit.rs:747-758`), and the tests above. The asserted-edge branch
of the SQL has no test that constructs an edge citer; it is read, not
exercised.
Existing check: `kernel_routes.rs:5801`, `:5723`; `kernel_slice.rs:2159`.
Status unaudited.
Impact: accepting a foreign id would attach another project's evidence to
this project's finding and reveal its existence through the answer.
Open questions:
- Evidence whose only citer carries `scope_id NULL` is never citable by a
  dispute. This follows the recommended definition in
  [open-questions.md](open-questions.md) Q1; no owner decision on the
  unscoped case is recorded. (needs human input)

### evidence-ref-well-formed-and-redaction-stable

Type: safety
Reachability: default-production - `DisputeReason::validated` runs for
every dispute in `compose_reason` (`commit.rs:221-227`); the command's
`--path <path>@<oid>` reaches it (`memory-mark-command.ts:75-81`).
Status: active
Exercised: yes - `a_dispute_citing_a_repository_file_records_it_in_the_reason`
(`kernel_routes.rs:5880`) lands five paths holding `;`, `=`, `%`, `@`,
spaces, and non-ASCII bytes at 40- and 64-hex object ids and reads them
back byte for byte; it refuses an uppercase oid, a 39-byte and a 63-byte
oid, `src/../a.rs`, `.git/config`, a NUL, a path over 1024 bytes, and a
path holding a detected token with `invalid_input` from the preview and
the commit, nothing written. Unit tests in `dispute.rs`:
`malformed_references_are_invalid_input` (`:300`),
`a_path_the_secret_scanner_detects_is_invalid_input` (`:328`),
`git_object_ids_and_ordinary_paths_carry_no_detection` (`:340`),
`render_and_parse_are_inverse_over_reserved_and_non_ascii_bytes` (`:266`).
Guarantee: A stored `evidence_ref` has a `git_object_id` of exactly 40 or
64 lowercase hex bytes and a `path` of at most 1024 bytes that passes the
daemon's confined-path check, decodes to itself, and holds no
secret-scanner detection; any other reference is refused `invalid_input`
before any transaction runs, and the stored bytes equal the sent bytes.
Check: `always` - for every `v=1` reason `r` with an `evidence_ref`,
`kernel::is_lower_hex_oid(oid)` (`scope.rs:710-716`), `len(path) <=
MAX_EVIDENCE_PATH_BYTES` (`dispute.rs:12`), `confined_path::check(path)`
is `Ok` (`crates/daemon/src/confined_path.rs:29-45`), the path holds no
`%` followed by two hex digits, and `redact_durable_text(path).detections`
is empty (`EvidenceRef::is_well_formed`, `dispute.rs:149-157`). Holds on
every stored row, so `always`.
Fault/timing angle: none; intrinsic admission.
Required faults and enabling state: each malformed shape above; a path
that holds a detected token; ordinary paths and hex object ids that must
pass.
Confidence: high - [evidence](evidence/evidence-ref-well-formed-and-redaction-stable.md).
Read `is_well_formed`, `validated`, `confined_path::check`, and the tests.
G3 is recorded in the evidence file: the test token
`sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678` is detected
(asserted at `dispute.rs:313`), and 64 seeded 40- and 64-hex ids beside
an ordinary path carry no detection (`dispute.rs:323-336`).
Existing check: `kernel_routes.rs:5880`; `dispute.rs:249`, `:283`,
`:311`, `:323`. Status unaudited.
Impact: a malformed reference cannot be followed; a path the redactor
rewrote would point at a file that does not exist.
Open questions:
- The route does not look the object up in the repository
  (`commit.rs:26-28`). A well-formed reference to a blob that never
  existed is accepted. Recorded as the documented contract; no owner
  decision to verify blobs exists.
- A path with a literal `%XX` sequence (for example a file named
  `100%41.md`) is refused by the decode-idempotence rule
  (`dispute.rs:204-210`). The command help does not mention the rule.

### reporter-class-derived-never-asserted-up

Type: safety
Reachability: default-production - `resolve_classes` runs on every
`kernel.commit` (`commit.rs:1111-1118`) and its taint is the reporter
taint `compose_reason` renders (`commit.rs:1122`).
Status: active
Exercised: partial - command arm and raw route bodies:
`a_dispute_reporter_class_is_derived_and_never_asserted_up`
(`kernel_routes.rs:6025`) refuses `asserted_taint_class: user_explicit`
under `source_kind` `assistant` and `user` with `class_over_declared`, an
unknown `source_kind` with `invalid_input`, nothing written, and records
`reporter_taint=assistant_inference` when `user` is lowered to it;
`kernel_routes.rs:5723` records `user_inferred` for `source_kind: "user"`
and `assistant_inference` for `assistant`, and asserts the row's
`(source_class, taint_class)` equal the subject's prior row. The
TypeScript test at `memory-mark-command.test.ts:174` asserts the command
sends `source_kind: "user"` on both calls. The model tool arm lands with
#1161.
Guarantee: The reporter taint recorded in a dispute's reason equals the
taint `derive_classes(source_kind)` yields, lowered only by an asserted
class at or below it; the row's own `source_class` and `taint_class` keep
the subject's prior classes; no actor, cause, or other client field
decides either.
Check: `always` - for every `v=1` reason `r`, `taint_rank(r.reporter_taint)
>= taint_rank(derive_classes(commit.source_kind).taint)`
(`commit.rs:368-380`, `:395-404`), and the row's class columns equal the
subject's previous row's (`disposition_request` copies them,
`commit.rs:725-726`). Holds on every write, so `always`.
Fault/timing angle: none.
Required faults and enabling state: an asserted class above the
derivation; an unknown `source_kind`; a lowered assertion; a subject
whose prior classes differ from the caller's derived pair.
Confidence: high - [evidence](evidence/reporter-class-derived-never-asserted-up.md).
Read `derive_classes`, `resolve_classes` (`commit.rs:411-444`), the
handler (`commit.rs:1111-1125`), and the tests.
Existing check: `kernel_routes.rs:6025`, `:5723`; `commit.rs:1215`
(`a_plugin_write_is_capped_at_inference_classes`), `:1261`; Status
unaudited.
Impact: a reporter class above the derived one would give model output
the standing of a user statement.
Open questions:
- `source_kind` itself is caller-asserted: a client may send `user` from
  any surface. See `commit-actor-cause-and-source-kind-are-caller-asserted`
  in [`../disposition-path/catalog.md`](../disposition-path/catalog.md)
  (portfolio gap G1). (needs human input)

### dispute-with-evidence-never-raises-standing

Type: safety
Reachability: default-production - every dispute reaches
`evaluate_admission` through `record_admission`
(`crates/kernel/src/admission.rs:982`).
Status: active
Exercised: yes - the evaluator grid test
`a_dispute_with_evidence_never_raises_standing_across_the_grid`
(`admission.rs:4532`) judges both dispute events with and without
evidence against every consistent `(historical, effective)` maturity pair
and every prior disposition. Route tests: `kernel_routes.rs:5723` reads
`outcome = deny`, unchanged `maturity`, and `effective_maturity =
candidate` after a dispute with evidence;
`a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale`
(`kernel_routes.rs:6146`) records `denied: true` with the prior kept for
`rejected`, `contradicted`, and `quarantined` priors and lands `stale` to
`disputed`; `kernel_routes.rs:6199` disputes an approval above Candidate
and reads its historical maturity kept and its effective maturity at
Candidate.
Guarantee: A dispute carrying evidence yields `outcome = deny`, keeps
historical maturity, never raises effective maturity, and moves the
disposition only to an equal or more restrictive one; against a more
restrictive prior without an approval it records `denied` with the prior
kept.
Check: `always` - for every dispute row `r` and its prior `p`:
`r.outcome = 'deny'`, `r.maturity = p.maturity`,
`rank(r.effective_maturity) <= rank(p.effective_maturity)`, and
`disposition_restrictiveness(r.disposition) >=
disposition_restrictiveness(p.disposition)`
(`admission.rs:4168-4175`). The forbidden thing is a state with no single
detection point, so `always` rather than `unreachable`.
Fault/timing angle: none.
Required faults and enabling state: a subject above Candidate; priors in
every disposition; both dispute events with and without evidence. The
only evaluator gate on evidence is `Enforce` (`admission.rs:275`), so the
field has no lever on these events.
Confidence: high - [evidence](evidence/dispute-with-evidence-never-raises-standing.md).
Read `event_effect` (`admission.rs:4161-4162`), the restrictiveness table,
and the four tests.
Existing check: `admission.rs:4532`; `kernel_routes.rs:5723`, `:6146`,
`:6199`. Status unaudited.
Impact: a report would become a promotion path.
Open questions:
- A dispute against a `rejected`, `contradicted`, or `quarantined` prior
  is recorded `denied: true` with the evidence on the row, not refused
  outright. The test pins the recorded form; see
  [open-questions.md](open-questions.md) Q9. (needs human input)

### dispute-identity-includes-evidence-key

Type: safety
Reachability: default-production - both harness commands derive the
identity with `memoryMarkOperationId(sessionId, args)`
(`command-handler.ts:415`,
`packages/pi-plugin/src/commands/eidnara-memory-mark.ts:61`).
Status: active
Exercised: yes - `memory-mark-command.test.ts:201` files a dispute by id
and one by file in one session: two identities, two commits, two
receipts; a repeat of the first replays with `replayed: true` and no new
receipt. `dispute_identities_land_replay_and_refuse_reuse_like_every_envelope`
(`kernel_routes.rs:6080`) lands two disputes of one decision under two
keys (two commit-log rows, two admission rows), replays a byte-identical
repeat with the first `commit_seq` and verdict and no write, and refuses
the first key with other bytes `operation_key_reused`.
Guarantee: One command dispute's identity is (session, event, subject,
evidence flags in a fixed order); a byte-identical repeat replays the
stored receipt with no second write; other evidence in the same session
is a second dispute that lands; the same identity with other bytes is
refused `operation_key_reused`.
Check: `always` - (a) at most one `commit_log` row per `(producer,
operation_key)` (the unique index on `commit_log`,
`crates/kernel/src/schema.rs:26`); (b)
`memoryMarkOperationId(s, a) == memoryMarkOperationId(s, b)` exactly when
`a` and `b` name the same event, object, and evidence
(`memory-mark-command.ts:189-194`, `:144-154`); (c) every identity field
is free of U+001F (the parser refuses it in the object id, evidence id,
and path, `memory-mark-command.ts:79`, `:116-121`). Holds on every
request, so `always`.
Fault/timing angle: a lost response followed by the client's reissue
under the same identity must replay. The receipt lookup precedes the
operation (`commit.rs:1068-1070`). The route-level reply drop is not
constructed (fault F12 in the disposition-path fault map).
Required faults and enabling state: two disputes with different evidence
in one session; an identical repeat; the same key with other bytes.
Confidence: high - [evidence](evidence/dispute-identity-includes-evidence-key.md).
Read `memoryMarkOperationId`, `evidenceFlags`, `parseMemoryMarkArgs`, and
the two tests.
Existing check: `memory-mark-command.test.ts:201`; `kernel_routes.rs:6080`.
Status unaudited.
Impact: a second piece of evidence would be refused with a message about
a reused key and the record would keep only the first.
Open questions:
- `--evidence A --path p@o` and `--path p@o --evidence A` produce one
  identity because `evidenceFlags` orders the flags
  (`memory-mark-command.ts:144-154`); the parser test asserts both orders
  parse (`memory-mark-command.test.ts:99`) but no test asserts the two
  identities are equal. Read, not exercised.

### dispute-preview-writes-nothing

Type: safety
Reachability: default-production - `runMemoryMarkCommand` previews before
every commit (`memory-mark-command.ts:222`); the route refuses a preview
of anything other than dispositions (`commit.rs:1151-1159`).
Status: active
Exercised: yes - `assert_refused_without_writing`
(`kernel_routes.rs:5701-5715`) previews and then commits one body and
asserts the same refusal name from both and unchanged `commit_log`,
`admission_decisions`, `operation_receipts`, and
`durable_text_redactions` counts; it runs for every unusable evidence id
(`:5801`), every malformed reference (`:5880`), and every missing or
misplaced field (`:5971`). `kernel_routes.rs:5880` previews each valid
reference with counts unchanged and then commits it.
`memory-mark-command.test.ts:223` reports a refused preview by name with
no receipt and one call.
Guarantee: A `preview: true` envelope carrying the evidence fields runs
the same form, liveness, and visibility checks as the commit, writes no
row, and answers the refusal name the commit returns for the same body at
the same tip.
Check: `always-or-unreached` - the preview is optional for a raw caller;
when it runs, the four row counts before and after are equal, and for an
invalid evidence input its refusal name equals the commit's. The form
check runs before either transaction (`commit.rs:1119-1125`) and the
evidence check runs inside `disposition_request`, which both paths call
(`commit.rs:790`, `:835`).
Fault/timing angle: a commit between preview and commit can change the
answer; the commit's verdict is the one recorded
(`memory-mark-command.ts:243-244`).
Required faults and enabling state: a preview of each refused input and
of a valid one at the same tip as the following commit.
Confidence: high - [evidence](evidence/dispute-preview-writes-nothing.md).
Read the handler, `preview` (`commit.rs:877-897`), and the helper.
Existing check: `kernel_routes.rs:5703`, `:5801`, `:5880`, `:5971`;
`memory-mark-command.test.ts:223`. Status unaudited.
Impact: a writing preview would record disputes the user declined; a
disagreeing one would ask the wrong question.
Open questions:
- The preview reports the subject's visibility only; a dispute on an
  approval also demotes its dependents in the commit. The limit is
  documented (`commit.rs:60-63`, `memory-mark-command.ts:53`) and tracked
  as G2 on `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit`.

### structured-reason-projects-typed-reporter-data

Type: safety
Reachability: default-production - every route-written dispute row, and
every `approval_revoked` row its cascade writes, carries the composed
reason (`commit.rs:732`; cascade copy asserted at `kernel_routes.rs:6241-6245`).
Status: active
Exercised: partial - the PR A form is active and exercised:
`kernel_routes.rs:5880` parses each stored reason, compares
`render(parse(reason))` with the stored bytes, and asserts ASCII;
`kernel_routes.rs:5723` parses the event, reporter taint, and evidence id;
`dispute.rs:235`, `:249`, `:373` cover the documented rendering, the
round trip over reserved and non-ASCII bytes, and refusal of every
rendering `render` does not produce. The typed form (reporter columns and
`event_payload`) arrives in PR B (#1162) and is not yet exercised.
Guarantee: In PR A, a dispute row's `reason` is the `v=1` rendering of
(event, reporter taint, optional evidence id, optional reference): one
ASCII line of at most 4096 bytes that `DisputeReason::parse` reads back
exactly and the redactor leaves unchanged. No admission decision reads it.
Check: `always` - for every row whose reason starts with `v=1;`:
`parse(reason)` is `Some`, `render(parse(reason)) == reason`, `reason` is
ASCII, `len(reason) <= MAX_REASON_BYTES` (`dispute.rs:14`), and
`redact_durable_text(reason).detections` is empty
(`dispute.rs:75-77`). The Kernel stores `redact_lossy(reason)`
(`crates/kernel/src/redaction.rs:41-42` calls the same
`redact_durable_text`), so no `durable_text_redactions` row exists for a
route-written reason. Holds on every write, so `always`.
Fault/timing angle: none.
Required faults and enabling state: references with every reserved byte
class; an evidence id holding `;`, `=`, and `%`; a reason over the cap; a
dispute on an approval so the cascade copies the reason.
Confidence: high - [evidence](evidence/structured-reason-projects-typed-reporter-data.md).
Read `render`, `parse`, `encode`, `decode`, `decodes_to_itself`
(`dispute.rs:81-210`), and the Kernel `reason` path
(`admission.rs:1689`). The redaction-row absence for a landed dispute is
argued from the shared detector, not asserted by a test.
Existing check: `kernel_routes.rs:5880`, `:5723`, `:6199`;
`dispute.rs:235`, `:249`, `:359`, `:390`. Status unaudited.
Impact: a reader could not recover the reporter or the reference, and PR
B could not be checked against PR A rows.
Open questions:
- The `event` field names the dispute event, so an `approval_revoked` row
  carries `event=mark_disputed` in its reason
  (`kernel_routes.rs:6241-6245`). A reader must take the row's own
  `event_kind` from the column. Recorded; no decision needed.
- Whether PR A rows carry into the PR B typed columns stays open
  ([open-questions.md](open-questions.md) Q6). (needs human input)

### use-outcome-link-names-own-session-prior-serve (PROPOSED)

Type: safety
Reachability: default-production - once PR B
([#1162](https://github.com/ahrav/eidnara/issues/1162)) writes the link.
At the PR A tip the `use_outcomes` table and the
`admission_decisions.use_outcome_id` column exist
(`crates/kernel/src/schema.rs:188`, `:163`), and the dispute route writes
no value into the column (`AdmissionRequest` carries no such field,
`commit.rs:722-734`).
Status: active
Lifecycle: proposed obligation, not yet implemented
Exercised: not yet - PR B lands the link.
Guarantee: A dispute row's `use_outcome_id`, when set, names a
`use_outcome` row whose session is the route's bound session, whose
decision is the dispute's subject, and whose `served_commit_seq` is the
greatest one strictly below the dispute's `commit_seq`; it is null when no
such row exists.
Check: `always-or-unreached` - the link path runs only when a serve
exists; when it runs, for every row `r` with `r.use_outcome_id IS NOT
NULL`, the joined `use_outcome` row `u` has `u.session = bound session`,
`u.decision = r.subject_object_id`, `u.served_commit_seq < r.commit_seq`,
and no other row for the same (session, decision) has a greater
`served_commit_seq` below `r.commit_seq`.
Falsifier: `u.session` differs from the reporting session; or
`u.served_commit_seq >= r.commit_seq`.
Legal witness: session S served D at seq 100 and 140; S disputes D at seq
150; the link names the seq-140 row.
Fault/timing angle: the lookup and the write share one writer
transaction; a serve committed after the dispute's snapshot is not
linked. A use-outcome sweep must keep rows a live dispute cites.
Required faults and enabling state: a session served the decision before
disputing it.
Confidence: low - proposed; no implementation evidence. The route binds
the session (envelope keys stripped at
`crates/daemon/src/kernel_routes/mod.rs:461-473`; `RouteScope` at
`:405-412`), and no route code writes `use_outcome_id` at this revision.
Existing check: none.
Impact: a wrong link attributes a harm signal to a session that did not
receive the finding.
Open questions:
- Whether the bound session is the right key when the same session id
  appears under two harnesses. (needs human input)
- Whether a dangling `use_outcome_id` after a sweep is a sweep defect or
  the column should carry no FK. See [open-questions.md](open-questions.md)
  Q10. The column carries an FK with `ON DELETE RESTRICT` at this revision
  (`schema.rs:163`).

## Relationship map

Shared mechanisms:

- `compose_reason` and `DisputeReason::validated` are the single form gate
  for `dispute-requires-evidence-from-plugin`,
  `evidence-ref-well-formed-and-redaction-stable`, and
  `structured-reason-projects-typed-reporter-data`; they run before any
  transaction, so `dispute-preview-writes-nothing` inherits their refusal
  names for free.
- `require_visible_evidence` over `Envelope::evidence_citations` decides
  both `cited-evidence-live-at-dispute-commit` (the `live` bit) and
  `cited-evidence-visible-to-bound-project` (the scope loop), and answers
  one name for both, so the two records share one refusal oracle.
- `resolve_classes` feeds `reporter-class-derived-never-asserted-up`
  through `compose_reason(classes.1)`; `disposition_request` copies the
  subject's classes for the same record's second clause.
- `evaluate_admission` decides `dispute-with-evidence-never-raises-standing`;
  evidence enters it only as `has_evidence`.
- The envelope receipt mechanism decides the route half of
  `dispute-identity-includes-evidence-key`; `memoryMarkOperationId` decides
  the command half.

Records replaced in the implemented part: `disposition-path`'s
`disposition-operation-carries-no-evidence-and-refuses-unknown-fields` is
invalidated by `dispute-requires-evidence-from-plugin` and
`evidence-ref-well-formed-and-redaction-stable`;
`evidence-objects-carry-no-scope-and-fail-scoped-lookup` and
`admission-evidence-citation-is-existence-checked-only` are re-labelled
and point here for the route's visibility and liveness checks.
