# Property catalog: dispute evidence on kernel.commit

System: the Eidnara repository at the PR A branch tip of
[#1160](https://github.com/ahrav/eidnara/issues/1160) (base `6159fd224`,
code commit `6c4d445e3`, review-round code commits `511cd6d73` and
`f68e895ca`; every `file:line` into a file either commit changed is
re-verified at `f68e895ca`).
Method: [`../../METHOD.md`](../../METHOD.md).
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

Ten records carry `Status: active`. Nine are implemented and checked by
the PR A tests; the tenth,
`use-outcome-link-names-own-session-prior-serve`, is implemented and
checked by PR B ([#1162](https://github.com/ahrav/eidnara/issues/1162)),
which also adds the typed form of
`structured-reason-projects-typed-reporter-data`. The model
tool arm of the dispute surface lands with
[#1161](https://github.com/ahrav/eidnara/issues/1161); the records that
name both surfaces cover the command arm only and say so.

## Reachability classes

Every implemented record is `default-production`. The shared evidence is one
fact: the daemon's method match dispatches `"kernel.commit"` to
`handle_kernel_commit` with no `#[cfg]` attribute
(`crates/daemon/src/lib.rs:14529`), and the route parses the evidence
fields on every disposition operation (`commit.rs:155-172`). The command
that sends them is registered in both harnesses
(`packages/opencode-plugin/src/hooks/context/command-handler.ts:413-419`,
`packages/pi-plugin/src/index.ts:550`). Each record restates its own
evidence. The proposed record's label is the class its path will have
once PR B writes it; it is not verifiable at HEAD and the record says so.

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
| `use-outcome-link-names-own-session-prior-serve` | safety | default-production | active | yes | always |

Semantics distribution: 9 `always`, 1 `always-or-unreached`, 0
`sometimes`, 0 `reachable`, 0 `unreachable`. The enabling states that
keep these from passing vacuously are listed in
[fault-map.md](fault-map.md).

## Records

### dispute-requires-evidence-from-plugin

Type: safety
Reachability: default-production - `lib.rs:14529` dispatches
`kernel.commit`; `compose_reason` runs for every operation of every
request after class resolution and before the preview or commit
transaction (`commit.rs:1162-1168`).
Status: active
Exercised: yes - command arm, tool arm, and raw route bodies: route test
`a_dispute_needs_evidence_and_other_events_take_none`
(`crates/daemon/tests/kernel_routes.rs:5971`) sends `mark_disputed` and
`contradict` with neither field and with an empty `evidence_id`, and
`mark_stale` with an `evidence_ref`, and asserts `invalid_input` from the
preview and the commit with the four row counts unchanged; it lands
`mark_stale`, `explicit_reject`, and `quarantine` without evidence with
`reason = "kernel.commit"` and `evidence_id IS NULL`. The command parser
test `a dispute without evidence, and evidence on another event, print
usage` (`memory-mark-command.test.ts:126`) covers the TypeScript side.
`a_disposition_carrying_its_own_reason_is_refused_as_an_unknown_field`
(`kernel_routes.rs:6252`) sends a `mark_disputed` body carrying a forged
`reason` field and asserts the route answers an error with the four row
counts unchanged; it asserts the error variant, not the refusal name. Tool
arm (#1161): `refuses missing evidence and unsupported events before
sending; renders denials and refusals`
(`packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1466`)
calls `dispute` with neither evidence input, with `path` and no
`gitObjectId`, and with no `objectId`, and asserts the usage error with no
daemon call; the Pi test `refuses a dispute without evidence before
sending anything` (`packages/pi-plugin/src/tools/eidnara-memory.test.ts:482`)
asserts the same for Pi. A route request from the tool with neither field
is the raw-body case the route test above already refuses
`invalid_input`, since the tool sends the same `disposition` body.
Guarantee: A `mark_disputed` or `contradict` operation that reaches the
Kernel writer through `kernel.commit` carries a non-empty `evidence_id`,
an `evidence_ref`, or both; one with neither, or with an empty id, is
refused `invalid_input` before any transaction runs; every other event
carries neither field and records the constant reason `kernel.commit`.
Check: `always` - a per-request route oracle. For every `kernel.commit`
request, it reads each disposition operation's `event`, `evidence_id`, and
`evidence_ref` (`commit.rs:155-172`) and the answer: when `event` is
`mark_disputed` or `contradict` and the request lands, `evidence_id` is a
non-empty string or `evidence_ref` is present; when `event` is one of the
other three and the request lands, both fields are absent and the written
row reads `reason = 'kernel.commit' AND evidence_id IS NULL`; every other
request answers `invalid_input` with the four row counts unchanged. The
dispute clause also has a store screen: every `admission_decisions` row
with `event_kind IN ('mark_disputed','contradict')` whose `reason` parses
as a `v=1` `DisputeReason` (`dispute.rs:108-148`) has `evidence_id IS NOT
NULL OR parse(reason).evidence_ref IS NOT NULL`. The non-dispute clause
has no store form: for those events the constant reason is the only
marker of a route-written row, so a scan on it is circular. The predicate
holds on every request, so `always`. The `v=1` prefix is the route's
marker: Kernel API callers of `record_admission` write rows outside it
(`commit.rs:55-58`).
Fault/timing angle: none; the check is a parse-time predicate that runs
before the writer is entered.
Required faults and enabling state: a body with a dispute and no evidence
field; a body with an empty `evidence_id`; a non-dispute event carrying
either field; a body carrying its own `reason`; a client that bypasses
the TypeScript parser and sends the raw body (the route tests do this).
Confidence: high - [evidence](evidence/dispute-requires-evidence-from-plugin.md).
Read `compose_reason` (`commit.rs:208-236`), `DisputeReason::validated`
(`dispute.rs:49-81`), the handler loop (`commit.rs:1162-1168`), and the
route and parser tests above.
Existing check: `kernel_routes.rs:5971`, `:6252`; `dispute.rs:372`
(`a_dispute_needs_evidence_and_other_events_take_none`, unit);
`memory-mark-command.test.ts:126`; the fake kernel mirrors the rule
(`disputeEvidenceWellFormed`,
`packages/opencode-plugin/src/shared/kernel-client-testing/fake-kernel.ts:285-307`).
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
- The model tool arm (#1161) bounds a model-filed dispute by evidence
  form and, for an `evidence_id`, liveness and visibility. A `path` with
  `gitObjectId` is checked for form only, so a model can cite a reference
  it invented. The tool also admits `contradict`, which moves the subject
  to `contradicted` (audit-only serving), past the `disputed` blast radius
  spec N3 records, and a dispute on an approval demotes its dependents.
  Nothing caps disputes per session beyond the per-request operation cap;
  the allowlist, which omits `dispute` by default, is the rollout lever.
  Whether to limit the tool to `mark_disputed` or add a per-session cap
  before enabling it is an owner decision. (needs human input)

### cited-evidence-live-at-dispute-commit

Type: safety
Reachability: default-production - `disposition_request` calls
`require_visible_evidence` for every dispute that names an `evidence_id`
(`commit.rs:732-734`), in the commit's writer closure
(`commit.rs:1083-1098`) and in the preview transaction
(`commit.rs:910-930`).
Status: active
Exercised: yes - `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing`
(`kernel_routes.rs:5801`) retires an evidence row through
`retire_evidence` and cites it under both dispute events; preview and
commit answer `not_found` and the four row counts are unchanged.
`a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter`
(`kernel_routes.rs:5723`) lands a live id and reads it back from
`admission_decisions.evidence_id`.
`a_dispute_previewed_while_its_evidence_was_visible_is_judged_again_at_commit`
(`kernel_routes.rs:6375`) previews a dispute citing project evidence
(`available`), retires the evidence's only project citer in a separate
commit, and commits the same dispute: the commit answers `not_found` with
the four row counts unchanged. The evidence row stays live, so the
refusal comes from the shared scope loop; it shows that
`require_visible_evidence` runs again inside the commit transaction
rather than trusting the preview. The Kernel read's liveness bit is
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
writer queue as the dispute. For the cited evidence row itself this state
cannot be built: a dispute's evidence is cited by a live row of the bound
project, and `retire_evidence` refuses evidence a live row cites with
`Conflict` (`crates/kernel/src/slice/write.rs:585-608`). The reachable
form is a retirement of the citer queued behind the dispute; a test needs
a second writer blocked on the lock while the dispute holds it, and no
test builds that interleaving. The ordering argument above is read from
code; the sequential form (preview, citer retired, commit) is observed by
`kernel_routes.rs:6375`.
Required faults and enabling state: an ingested evidence row retired
before the dispute; a never-ingested id; a live cited id for the positive
case; a citer retired between a preview and its commit.
Confidence: high - [evidence](evidence/cited-evidence-live-at-dispute-commit.md).
Read `require_visible_evidence` (`commit.rs:751-772`),
`evidence_citations` (`envelope.rs:532-577`), and the three route tests.
Existing check: `kernel_routes.rs:5801`, `:5723`, `:6375`;
`kernel_slice.rs:2159`. Status unaudited.
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
(`commit.rs:732-734`).
Status: active
Exercised: yes - `kernel_routes.rs:5801` cites a live id that nothing
cites, a live id cited only by an unscoped decision, and a live id cited
only by a decision of a second bound project; each answers `not_found`
from the preview and the commit with nothing written, under both dispute
events. `kernel_routes.rs:5723` lands an id cited by a decision of the
bound project. `kernel_slice.rs:2159` asserts which citers contribute a
scope: live decisions, live observations, and decision events through a
live decision; unscoped and retired citers contribute none.
`a_dispute_judges_every_citer_and_both_evidence_fields_together`
(`kernel_routes.rs:6284`) cites one evidence id cited by a decision of
the bound project and by a decision of a foreign scope whose id sorts
before the project's scope: the dispute lands, so the loop judges every
citer rather than the first; an id cited only by the foreign scope
answers `not_found` with nothing written.
`kernel_routes.rs:6375` retires the only project citer between a preview
that answered `available` and the commit, which answers `not_found` with
nothing written. `an_insert_citing_evidence_makes_it_citable_later_in_the_same_commit`
(`kernel_routes.rs:6475`) refuses a dispute citing uncited live evidence
with `not_found`, then lands one envelope holding an `insert_decision`
citing that evidence followed by the dispute, with the id on the row.
Guarantee: A dispute's `evidence_id` is accepted only when a live
decision, observation, decision event (through its live decision), or
asserted edge whose scope the bound project's filter accepts cites it;
otherwise the request is refused `not_found`, the same name a nonexistent
id gets.
Check: `always` - for every dispute row `r` with `r.evidence_id`, at
`r.commit_seq` at least one of `EvidenceCitations.citing_scopes`
(`envelope.rs:187-194`) passed `ScopeFilter::matches` for the bound
project (`commit.rs:764-770`). Every refusal case (unknown, retired,
uncited, unscoped-only, foreign-only) answers the same `not_found` with
row counts unchanged, so this check answers the same for another
project's evidence as for an unknown id (doc comment at
`commit.rs:751-754`). Holds on every request, so `always`.
Fault/timing angle: the only citing row retired between a client's read
and the commit; the read runs in the writer transaction, as in the
liveness record. The sequential form (preview, citer retired, commit) is
constructed by `kernel_routes.rs:6375`. A citer written earlier in the
same envelope counts: `apply` runs operations in order in one transaction
(`commit.rs:934-1077`), and `kernel_routes.rs:6475` observes it.
Required faults and enabling state: two bound projects in one Kernel
store; evidence cited only by the other project; evidence cited by both
projects with the foreign scope sorting first; evidence cited only by an
unscoped row; an uncited live row; a citer retired between preview and
commit; an insert citing the evidence earlier in the dispute's envelope.
Confidence: high - [evidence](evidence/cited-evidence-visible-to-bound-project.md).
Read the SQL in `evidence_citations` (`envelope.rs:553-566`), the filter
loop (`commit.rs:760-771`), and the tests above. The asserted-edge branch
of the SQL (`envelope.rs:563-564`) has no production writer and no test:
no Rust code under `crates/` inserts into `asserted_edges`, so the arm is
read, not exercised.
Existing check: `kernel_routes.rs:5801`, `:5723`, `:6284`, `:6375`,
`:6475`; `kernel_slice.rs:2159`. Status unaudited.
Impact: accepting a foreign id would attach another project's evidence to
this project's finding and reveal its existence through the answer.
Open questions:
- Evidence whose only citer carries `scope_id NULL` is never citable by a
  dispute. This follows the recommended definition in
  [open-questions.md](open-questions.md) Q1; no owner decision on the
  unscoped case is recorded. (needs human input)
- Self-citation through the same route (portfolio gap G1). The route's
  `insert_decision` and `insert_observation` accept any live evidence id
  (`commit.rs:276`, `:314`; `require_parents` checks liveness only,
  `crates/kernel/src/slice/write.rs:1095-1107`), and
  `kernel_routes.rs:6475` shows a dispute citing that evidence later in
  the same envelope lands. Under the resolved OQ4-3 definition, evidence
  is citable once this project cites it, in the same envelope included,
  so the check bounds which project a citation is attached to, not which
  evidence a client can reach. The insert path answers `available` for a
  live id and `not_found` for an unknown one; that existence answer is
  pre-existing behavior outside #1160. Whether visibility should mean
  "already cited by this project" or "citable by this project", and
  whether inserts should get the same check, is an owner decision.
  (needs human input)
- `Envelope::evidence_citations` (`envelope.rs:536-577`) reads every row
  that cites one evidence id, with no bound, inside the writer
  transaction. Review disposition: the owner defers this; each citing row
  was paid for by a write, and a bounded read is a follow-up outside
  #1160.
- The asserted-edge arm of `evidence_citations` has no production writer
  and no test (see Confidence). Recorded; no decision needed until a
  writer exists.

### evidence-ref-well-formed-and-redaction-stable

Type: safety
Reachability: default-production - `DisputeReason::validated` runs for
every dispute in `compose_reason` (`commit.rs:223-229`); the command's
`--path <path>@<oid>` reaches it (`memory-mark-command.ts:75-81`).
Status: active
Exercised: yes - `a_dispute_citing_a_repository_file_records_it_in_the_reason`
(`kernel_routes.rs:5880`) lands five paths holding `;`, `=`, `%`, `@`,
spaces, and non-ASCII bytes at 40- and 64-hex object ids and reads them
back byte for byte; it refuses an uppercase oid, a 39-byte and a 63-byte
oid, `src/../a.rs`, `.git/config`, a NUL, a path over 1024 bytes, and a
path holding a detected token with `invalid_input` from the preview and
the commit, nothing written. `kernel_routes.rs:6284` lands one dispute
carrying both `evidence_id` and `evidence_ref` and parses both back from
the reason, and refuses an uppercase-oid reference beside an id that
would answer `not_found` with `invalid_input`, so the form check runs
first. Unit tests in `dispute.rs`:
`malformed_references_are_invalid_input` (`:314`),
`a_path_the_secret_scanner_detects_is_invalid_input` (`:342`),
`git_object_ids_and_ordinary_paths_carry_no_detection` (`:354`),
`render_and_parse_are_inverse_over_reserved_and_non_ascii_bytes` (`:246`),
`reserved_and_multi_byte_values_encode_to_uppercase_escapes` (`:280`, a
fixed witness for `encode` and `decode`), and the proptest
`every_accepted_reference_parses_back_to_its_inputs` (`:292`), which
asserts every accepted path and optional id renders an ASCII reason
within the cap that parses back to the inputs.
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
is empty (`EvidenceRef::is_well_formed`, `dispute.rs:151-159`). Holds on
every stored row, so `always`.
Fault/timing angle: none; intrinsic admission.
Required faults and enabling state: each malformed shape above; a path
that holds a detected token; ordinary paths and hex object ids that must
pass.
Confidence: high - [evidence](evidence/evidence-ref-well-formed-and-redaction-stable.md).
Read `is_well_formed`, `validated`, `confined_path::check`, and the tests.
G3 is recorded in the evidence file: the test token
`sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678` is detected
(asserted at `dispute.rs:346`), and 64 seeded 40- and 64-hex ids beside
an ordinary path carry no detection (`dispute.rs:356-369`).
Existing check: `kernel_routes.rs:5880`, `:6284`; `dispute.rs:248`,
`:280`, `:292`, `:314`, `:342`, `:354`. Status unaudited.
Impact: a malformed reference cannot be followed; a path the redactor
rewrote would point at a file that does not exist.
Open questions:
- The route does not look the object up in the repository
  (`commit.rs:26-28`). A well-formed reference to a blob that never
  existed is accepted. Recorded as the documented contract; no owner
  decision to verify blobs exists.
- A path with a literal `%XX` sequence (for example a file named
  `100%41.md`) is refused by the decode-idempotence rule
  (`dispute.rs:203-209`). The command help does not mention the rule.
- A path holding a space cannot be cited from the command: the parser
  splits its arguments on `/\s+/` (`memory-mark-command.ts:85-88`), while
  the route accepts such paths (`kernel_routes.rs:5880`). At `511cd6d73`
  the usage text names the limit ("a relative repository path without
  spaces", `memory-mark-command.ts:51`). Whether the command should accept
  quoted or escaped paths is open. (needs human input)
- `evidence_ref.path` accepts C0 and bidi control characters:
  `confined_path::check` refuses NUL only among control bytes
  (`confined_path.rs:30-31`), and `encode` stores every such byte
  percent-encoded (`dispute.rs:165-175`). Review disposition: a nit, not
  fixed in #1160.

### reporter-class-derived-never-asserted-up

Type: safety
Reachability: default-production - `resolve_classes` runs on every
`kernel.commit` (`commit.rs:1154-1161`) and its taint is the reporter
taint `compose_reason` renders (`commit.rs:1165`).
Status: active
Exercised: yes - command arm, tool arm, and raw route bodies:
`a_dispute_reporter_class_is_derived_and_never_asserted_up`
(`kernel_routes.rs:6025`) refuses `asserted_taint_class: user_explicit`
under `source_kind` `assistant` and `user` with `class_over_declared`, an
unknown `source_kind` with `invalid_input`, nothing written, and records
`reporter_taint=assistant_inference` when `user` is lowered to it;
`kernel_routes.rs:5723` records `user_inferred` for `source_kind: "user"`
and `assistant_inference` for `assistant`, and asserts the row's
`(source_class, taint_class)` equal the subject's prior row. The
TypeScript test at `memory-mark-command.test.ts:174` asserts the command
sends `source_kind: "user"` on both calls. Tool arm (#1161): `sends the
evidence under the tool identity, actor, cause, and assistant source kind`
(`packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1373`)
asserts the tool's preview and commit both send `source_kind:
"assistant"` and no asserted class, and the Pi test `files a dispute under
agent:pi and the assistant source kind when allowed`
(`packages/pi-plugin/src/tools/eidnara-memory.test.ts:433`) asserts the
same for Pi; the route side stores `assistant_inference` for that source
kind (`kernel_routes.rs:5723`). The harness fixes `source_kind` in
`executeDispute`, so the tool cannot send `"user"`.
Guarantee: The reporter taint recorded in a dispute's reason equals the
taint `derive_classes(source_kind)` yields, lowered only by an asserted
class at or below it; the row's own `source_class` and `taint_class` keep
the subject's prior classes; no actor, cause, or other client field
decides either.
Check: `always` - a per-request route oracle. For every `kernel.commit`
request that lands a dispute, it reads the request's `source_kind`,
`asserted_source_class`, and `asserted_taint_class` (`commit.rs:115-119`)
and the written row: `taint_rank(parse(r.reason).reporter_taint) >=
taint_rank(derive_classes(request.source_kind).taint)`
(`commit.rs:370-382`, `:395-404`), the reporter taint equals
`resolve_classes(source_kind, asserted_source_class,
asserted_taint_class).1` (`commit.rs:413-446`), and the row's class
columns equal the subject's previous row's (`disposition_request` copies
them, `commit.rs:738-739`). The check has no store form: `commit_log`
stores no `source_kind` ([open-questions.md](open-questions.md) C2), and
the row's `source_kind` column holds the subject's facts, not the
request's. Holds on every request, so `always`.
Fault/timing angle: none.
Required faults and enabling state: an asserted class above the
derivation; an unknown `source_kind`; a lowered assertion; a subject
whose prior classes differ from the caller's derived pair.
Confidence: high - [evidence](evidence/reporter-class-derived-never-asserted-up.md).
Read `derive_classes`, `resolve_classes` (`commit.rs:413-446`), the
handler (`commit.rs:1154-1168`), and the tests.
Existing check: `kernel_routes.rs:6025`, `:5723`; `commit.rs:1264`
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
(`crates/kernel/src/admission.rs:998`).
Status: active
Exercised: yes - the evaluator grid test
`a_dispute_with_evidence_never_raises_standing_across_the_grid`
(`admission.rs:4636`) judges both dispute events with and without
evidence against every consistent `(historical, effective)` maturity pair
and every prior disposition. Route tests: `kernel_routes.rs:5723` reads
`outcome = deny`, unchanged `maturity`, and `effective_maturity =
candidate` after a dispute with evidence;
`a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale`
(`kernel_routes.rs:6146`) records `denied: true` with the prior kept for
`rejected`, `contradicted`, and `quarantined` priors and lands `stale` to
`disputed`; `kernel_routes.rs:6199` disputes an approval above Candidate
and reads its historical maturity kept and its effective maturity at
Candidate. `a_dispute_relaxes_a_contradicted_prior_only_with_a_valid_approval`
(`kernel_routes.rs:6418`) disputes a `contradicted` decision with
evidence: without an approval it answers `denied: true` and the decision
stays `contradicted`; with a valid `approval_object_id` it answers
`denied: false`, lands `disputed`, and the row reads `outcome = deny`
with the evidence id.
Guarantee: A dispute carrying evidence yields `outcome = deny`, keeps
historical maturity, never raises effective maturity, and moves the
disposition to an equal or more restrictive one unless the operation
names a valid approval; against a more restrictive prior without a valid
approval it records `denied` with the prior kept.
Check: `always` - for every dispute row `r` and its prior `p`:
`r.outcome = 'deny'`, `r.maturity = p.maturity`,
`rank(r.effective_maturity) <= rank(p.effective_maturity)`, and
`disposition_restrictiveness(r.disposition) >=
disposition_restrictiveness(p.disposition)`
(`admission.rs:4272-4279`) unless `r.approval_object_id` names an
approval the evaluator judged valid for `r`'s subject in `r`'s
transaction (`validate_approval`, `admission.rs:1638-1642`; the
relaxation rule at `admission.rs:337-350`). This matches AC8: no
relaxation without a valid approval. Validity is not a stored column, so
the approval clause is evaluated per request from the answer's `denied`
flag; over stored rows, `r.approval_object_id IS NULL` implies the
restrictiveness clause. The forbidden thing is a state with no single
detection point, so `always` rather than `unreachable`.
Fault/timing angle: none.
Required faults and enabling state: a subject above Candidate; priors in
every disposition; both dispute events with and without evidence; a
valid approval named on a dispute against a more restrictive prior. The
only evaluator gate on evidence is `Enforce` (`admission.rs:291`), so the
field has no lever on these events.
Confidence: high - [evidence](evidence/dispute-with-evidence-never-raises-standing.md).
Read `event_effect` (`admission.rs:4265-4266`), the restrictiveness table,
the relaxation rule, and the five tests.
Existing check: `admission.rs:4636`; `kernel_routes.rs:5723`, `:6146`,
`:6199`, `:6418`. Status unaudited.
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
operation (`commit.rs:1104-1106`). The route-level reply drop is not
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
of anything other than dispositions (`commit.rs:1200-1208`).
Status: active
Exercised: yes - `assert_refused_without_writing`
(`kernel_routes.rs:5701-5715`) previews and then commits one body and
asserts the same refusal name from both and unchanged `commit_log`,
`admission_decisions`, `operation_receipts`, and
`durable_text_redactions` counts; it runs for every unusable evidence id
(`:5801`), every malformed reference (`:5880`), and every missing or
misplaced field (`:5971`). `kernel_routes.rs:5880` previews each valid
reference with counts unchanged and then commits it.
`kernel_routes.rs:6375` previews a dispute that answers `available`,
retires the evidence's only project citer, and commits: the commit
answers `not_found` and writes nothing, so the commit's verdict is the
one that holds when the tip moves.
`memory-mark-command.test.ts:242` reports a refused preview by name with
no receipt and one call.
Guarantee: A `preview: true` envelope carrying the evidence fields runs
the same form, liveness, and visibility checks as the commit, writes no
row, and answers the refusal name the commit returns for the same body at
the same tip.
Check: `always-or-unreached` - the preview is optional for a raw caller;
when it runs, the four row counts before and after are equal, and for an
invalid evidence input its refusal name equals the commit's. The form
check runs before either transaction (`commit.rs:1162-1168`) and the
evidence check runs inside `disposition_request`, which both paths call
(`commit.rs:804`, `:835`).
Fault/timing angle: a commit between preview and commit can change the
answer; the commit's verdict is the one recorded
(`memory-mark-command.ts:243-244`). Constructed at the route by
`kernel_routes.rs:6375`.
Required faults and enabling state: a preview of each refused input and
of a valid one at the same tip as the following commit; a commit between
a preview and its commit that changes the verdict.
Confidence: high - [evidence](evidence/dispute-preview-writes-nothing.md).
Read the handler, `preview` (`commit.rs:910-930`), and the helper.
Existing check: `kernel_routes.rs:5703`, `:5801`, `:5880`, `:5971`,
`:6375`; `memory-mark-command.test.ts:242`. Status unaudited.
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
reason (`commit.rs:745`; cascade copy asserted at `kernel_routes.rs:6241-6245`).
Status: active
Exercised: partial - the PR A form is active and exercised:
`kernel_routes.rs:5880` parses each stored reason, compares
`render(parse(reason))` with the stored bytes, and asserts ASCII;
`kernel_routes.rs:5723` parses the event, reporter taint, and evidence id;
`kernel_routes.rs:6284` parses a reason carrying both an evidence id and
a reference; `a_disposition_carrying_its_own_reason_is_refused_as_an_unknown_field`
(`kernel_routes.rs:6252`) sends a forged `reason` and asserts an error
with the four row counts unchanged, so the client cannot supply the
reason; `dispute.rs:234`, `:246`, `:404` cover the documented rendering,
the round trip over reserved and non-ASCII bytes, and refusal of every
rendering `render` does not produce; `dispute.rs:282` pins `encode` and
`decode` on reserved and multi-byte input; the proptest at
`dispute.rs:294` asserts every accepted value renders an ASCII reason
within the cap that parses back to its inputs. Typed form (#1162):
`a_dispute_row_records_its_reporter_and_links_the_sessions_latest_serve`
(`kernel_routes.rs:6604`) asserts the reporter columns read
`(model_inference, user_inferred)` for a command-shaped dispute and
`(model_inference, assistant_inference)` for a tool-shaped one, that
`event_payload` holds the sent `evidence_ref` serialized byte for byte and
is null when none was sent, and that `DisputeReason` rendered from the
typed columns and the row's `evidence_id` equals the stored `reason`;
`reporter_columns_are_dispute_only_and_checked_by_the_schema`
(`crates/kernel/tests/kernel_use_outcome.rs:1243`) asserts other writers
store NULL and the CHECKs refuse an unknown class, a half-set pair, and
non-JSON payload.
Guarantee: The typed columns (`reporter_source_class`,
`reporter_taint_class`, `event_payload`, and the row's `evidence_id`) are
authoritative for a route-written dispute, and its `reason` equals their
`v=1` rendering. The rendering is
(event, reporter taint, optional evidence id, optional reference): one
ASCII line of at most 4096 bytes that `DisputeReason::parse` reads back
exactly and the redactor leaves unchanged. No admission decision reads it.
Check: `always` - for every row whose reason starts with `v=1;`:
`parse(reason)` is `Some`, `render(parse(reason)) == reason`, `reason` is
ASCII, `len(reason) <= MAX_REASON_BYTES` (`dispute.rs:14`), and
`redact_durable_text(reason).detections` is empty
(`dispute.rs:77-79`). The Kernel stores `redact_lossy(reason)`
(`crates/kernel/src/redaction.rs:41-42` calls the same
`redact_durable_text`), so no `durable_text_redactions` row exists for a
route-written reason. Holds on every write, so `always`.
Fault/timing angle: none.
Required faults and enabling state: references with every reserved byte
class; an evidence id holding `;`, `=`, and `%`; a reason over the cap; a
dispute on an approval so the cascade copies the reason; a body carrying
its own `reason`.
Confidence: high - [evidence](evidence/structured-reason-projects-typed-reporter-data.md).
Read `render`, `parse`, `encode`, `decode`, `decodes_to_itself`
(`dispute.rs:83-209`), and the Kernel `reason` path
(`admission.rs:1780`). The redaction-row absence for a landed dispute is
argued from the shared detector, not asserted by a test.
Existing check: `kernel_routes.rs:5880`, `:5723`, `:6199`, `:6252`,
`:6284`; `dispute.rs:234`, `:246`, `:280`, `:292`, `:390`, `:404`.
Status unaudited.
Impact: a reader could not recover the reporter or the reference, and PR
B could not be checked against PR A rows.
Open questions:
- The `event` field names the dispute event, so an `approval_revoked` row
  carries `event=mark_disputed` in its reason
  (`kernel_routes.rs:6241-6245`). A reader must take the row's own
  `event_kind` from the column. Recorded; no decision needed.
- PR A rows never coexist with the typed columns: Batch K1 rotates the
  Kernel baseline once, and a store created before it refuses to open
  (`crates/kernel/tests/kernel_open.rs:582`). Resolved by the rotation.

### use-outcome-link-names-own-session-prior-serve

Type: safety
Reachability: default-production - every `mark_disputed` and `contradict`
the route commits goes through `Envelope::record_dispute_admission`
(`crates/daemon/src/kernel_routes/commit.rs:798`;
`crates/kernel/src/admission.rs:1014`), which computes the link with
`reporting_serve` (`admission.rs:1737`) and writes it in
`write_reported_admission` (`admission.rs:1765`) with no flag.
Status: active
Exercised: yes - Kernel test
`a_dispute_links_the_reporting_sessions_latest_prior_serve_of_its_subject`
(`crates/kernel/tests/kernel_use_outcome.rs:1165`) serves `d1` to one
session at two sequences, serves another decision to the same session and
`d1` to another session, and asserts the dispute links the later serve, a
dispute of the other decision links that decision's serve, a session never
served the subject links null, and a serve recorded after the dispute
leaves the stored link unchanged. Route test
`a_dispute_row_records_its_reporter_and_links_the_sessions_latest_serve`
(`crates/daemon/tests/kernel_routes.rs:6604`) seeds serves through the P3
insert API under the route's project digest and harness, files a
command-shaped dispute that links the later of two serves, a tool-shaped
dispute of a decision the session was never served that links null, and a
`mark_stale` that links nothing. The link cannot name a serve at or above
the dispute's sequence because `use_outcomes` requires
`served_commit_seq < created_commit_seq` and the query bounds
`served_commit_seq` below the dispute's commit.
Guarantee: A dispute row's `use_outcome_id`, when set, names the
`use_outcomes` row of the route's (project digest, harness, reporting
session) whose decision rows name the dispute's subject and whose
`served_commit_seq` is the greatest strictly below the dispute's
`commit_seq`; it is null when no such row exists.
Check: `always` - for every row `r` with
`r.reporter_taint_class IS NOT NULL`: when `r.use_outcome_id` is set, the
joined record `u` has `u.served_commit_seq < r.commit_seq`, a
`use_outcome_decisions` row naming `r.subject_object_id`, and no record of
the same (project digest, harness, session) naming the subject has a
greater `served_commit_seq` below `r.commit_seq` at the time of the write;
the session is the request envelope's `session_id`, read before the
transport strips it (`commit.rs:1121`). Every route dispute takes this
path, so `always`.
Fault/timing angle: the lookup and the INSERT share one writer
transaction, so a serve committed after the dispute is never linked; the
`admission_decisions_use_outcome_immutable` trigger keeps the link as it
was written.
Required faults and enabling state: a session served the decision at two
sequences before disputing it; a session never served it; a serve of a
different decision in the same session; a serve recorded after the
dispute.
Confidence: high - [evidence](evidence/use-outcome-link-names-own-session-prior-serve.md).
Read `reporting_serve` and the INSERT; both tests above construct the
listed faults.
Existing check: `kernel_use_outcome.rs:1165`; `kernel_routes.rs:6604`.
Status unaudited.
Impact: a wrong link attributes a harm signal to a session that did not
receive the finding.
Open questions:
- Whether the bound session is the right key when the same session id
  appears under two harnesses. The key includes the harness, so two
  harnesses never share a link. Recorded; no decision needed.
- Retired serving records stay linkable: the query reads every record of
  the session, retired or not. A record a committed dispute cites is
  retained by the P3 sweep while its subject is live
  (`kernel_routes.rs:6604` asserts `cited == 1` and the record live after
  expiry). (needs human input on whether a retired record should be
  linked)

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
