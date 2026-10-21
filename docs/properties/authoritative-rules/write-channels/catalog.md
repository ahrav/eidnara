# Proposed write channels: directive channel (N1) and repository policy (N2)

## Scope

Proposed obligations for the two authority channels the settled plan
(`docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md`,
baseline `08de9051a735f136ff63a9dca49eaa11493b3006`) adds in front of the
unchanged Kernel admission policy: the `directive.*` route family with the
explicit-user subject guard in `kernel.commit` (N1), and the
`repository_policy` module that ingests `AGENTS.md` blobs from the git HEAD
tree and reconciles rule decisions (N2). Nothing in this part is implemented
at HEAD; every record is a design obligation whose check becomes executable
when U3 and U5 land. Existing Kernel mechanisms the obligations reuse are
cited with the test that already pins them.

Reachability is stated per record for the proposed path once built. The
directive route is reachable in default production whenever a project is
bound and a user invokes the command; the repository policy path is reachable
in default production whenever the bound project root is a git repository
with a readable HEAD. Neither is gated by a config key in the plan.

## Index

| slug | type | reachability | status | existing check |
| --- | --- | --- | --- | --- |
| directive-record-self-approves-at-hardcoded-class-pair | safety | default-production | active | `crates/kernel/tests/kernel_admission.rs:1740`, `crates/kernel/src/admission.rs:5066` |
| directive-explicit-user-unreachable-from-model-paths | safety | default-production | active | `crates/daemon/tests/kernel_routes.rs:1145` |
| directive-operation-key-replay-is-idempotent | safety | default-production | active | `crates/daemon/tests/kernel_routes.rs:931, 4096` |
| directive-revise-supersedes-same-lineage-and-reapproves | safety | default-production | active | `crates/kernel/tests/kernel_admission.rs:788, 991` |
| directive-revoke-cascades-and-stops-serving | safety | default-production | active | `crates/kernel/tests/kernel_admission.rs:469, 2081` |
| directive-confirmed-from-promotes-by-fresh-record | safety | default-production | active | none |
| directive-explicit-user-subject-guard-refuses-model-commit | safety | default-production | active | none |
| repo-policy-rules-derive-from-head-tree-or-nothing | safety | default-production | active | none |
| repo-policy-bounds-fail-closed-per-file | safety | default-production | active | none |
| repo-policy-segmentation-is-deterministic | safety | default-production | active | none |
| repo-policy-unit-reconciliation-by-digest | safety | default-production | active | `crates/kernel/tests/kernel_admission.rs:788` |
| repo-policy-rule-verified-only-with-provenance-witness | safety | default-production | active | `crates/kernel/tests/kernel_admission.rs:1292, 1323, 1339, 4150, 4211` |
| repo-policy-per-file-commit-atomic-and-pass-converges | liveness | default-production | active | none |
| repo-policy-cached-head-oid-is-advisory-and-reconverges | safety | default-production | active | none |

## Records

### directive-record-self-approves-at-hardcoded-class-pair

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the `directive.record` route (plan U3) does not exist at
HEAD
Guarantee: A successful `directive.record` leaves exactly one new live
`adr_accepted` decision with `source_kind user_directive`, lineage
`("user_directive","directive:<uuid>",1)`, sensitivity Normal, whose latest
admission row is `AcceptedAdr` at `(explicit_user, user_explicit)` with
effective maturity Approved, disposition Active, visibility `automatic`, and
that row is Visible on AutoInject.
Check: `always` - after a committed `directive.record`, the decision row exists
and is live, its admission row has `source_class='explicit_user'`,
`taint_class='user_explicit'`, `effective_maturity='approved'`,
`visibility='automatic'`, `sensitivity_class='normal'`, and
`surface_visibility(Automatic, AutoInject, Normal) == Visible`; the admission
row's `commit_seq` equals the decision's `created_commit_seq` (same envelope)
and the decision insert precedes the admission inside that envelope. `always`
because every committed record must satisfy it.
Fault/timing angle: the order of insert and `record_admission` inside the
envelope; `subject_is_accepted_decision` reads the live decision at admission
time.
Required faults and enabling state: a bound project; a route body with non-empty
text; for the negative arm, a body carrying `asserted_source_class` or
`asserted_taint_class`, which must be `invalid_params` with no write.
Confidence: high -
[evidence](evidence/directive-record-self-approves-at-hardcoded-class-pair.md).
Verified the self-approval mechanism (`admission.rs:4221-4234, 2436-2458`), the
Normal requirement (`admission.rs:843-856`), and the test seed that already uses
this sequence (`crates/daemon/tests/kernel_routes.rs:4355-4386`).
Existing check: `crates/kernel/tests/kernel_admission.rs:1740`
`only_an_accepted_decision_object_self_approves` and
`crates/kernel/src/admission.rs:5066` pin the Kernel half; nothing pins a daemon
route.
Impact: a directive that fails to self-approve lands Candidate and never
injects; a route that accepts asserted classes makes `explicit_user` reachable
from a crafted body.
Open questions:
- Whether the route refuses `asserted_*` fields through `deny_unknown_fields` on
  the body or through an explicit check; the plan says refuse (resolvable at
  implementation).

### directive-explicit-user-unreachable-from-model-paths

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the `directive.*` methods do not exist; the model tool
client at HEAD cannot be shown to lack a method that is not yet defined
Guarantee: No admission row with `source_class = explicit_user` is produced by a
commit whose producer is a plugin route (`kernel.commit`, `kernel.ingest`) or
the MemoryReviewer acceptance; the only production producer of such rows is the
`directive.*` route, and `derive_classes` keeps every plugin `source_kind` at
`ModelInference`.
Check: `always` - for every row in `admission_decisions` with
`source_class='explicit_user'`, the joined `commit_log.producer` is the
directive route's producer (or a test seed under `cfg(test)`); and
`derive_classes(k)` returns `ModelInference` for every `k` it accepts. `always`
because this is a forbidden state (F1), so `always(!X)` rather than
`unreachable`.
Fault/timing angle: none; a static table plus a ledger invariant.
Required faults and enabling state: a model tool call through `kernel.commit`
with any `source_kind` and with `asserted_source_class: "explicit_user"`; the
expected outcome is `ClassOverDeclared`.
Confidence: high -
[evidence](evidence/directive-explicit-user-unreachable-from-model-paths.md).
Verified `derive_classes` (`commit.rs:236-248`) and `resolve_classes` refusing
an over-declared class (`commit.rs:271-312`).
Existing check: `crates/daemon/tests/kernel_routes.rs:1145`
`a_plugin_route_cannot_declare_a_class_above_the_derived_one` covers the
over-declaration arm; nothing covers the ledger-wide invariant.
Impact: a model-authored row reaching `explicit_user` becomes a self-approving
authority and an approval root for other rows.
Open questions:
- Which daemon-internal modules other than the three routes call
  `record_admission` with caller-chosen classes; an inventory is needed before the
  writer set is closed (resolvable at implementation).

### directive-operation-key-replay-is-idempotent

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no directive route; the Kernel receipt mechanism it reuses
is exercised by existing commit-route tests
Guarantee: A second `directive.record` (or `revise`, `revoke`) with the same
`operation_key` and the same request digest returns the original receipt with
`replayed: true` and writes no row; the same key with a different digest is
refused as a reused key and writes nothing; the key is namespaced by project
digest and a directive route family so it cannot answer a `kernel.commit`
receipt.
Check: `always` - after two identical calls, `COUNT(*)` of live `user_directive`
decisions equals one and the second response carries `replayed: true`; after a
same-key different-digest call, the response is the reuse refusal and the row
count is unchanged; the stored `operation_receipts.operation_key` starts with
`<project digest>:<family>:`. `always` because every replay must behave this
way.
Fault/timing angle: a lost response (daemon crash after `COMMIT`, client retry)
is the motivating window; the retry must find the receipt.
Required faults and enabling state: a committed record; a repeated request with
equal bytes; a repeated request with the same key and changed text.
Confidence: high -
[evidence](evidence/directive-operation-key-replay-is-idempotent.md). Verified
`stored_receipt` replay and digest conflict (`envelope.rs:1279-1309`), the
receipt lookup before the operation (`envelope.rs:1321-1330`), the route's
`OperationKeyReused` mapping (`commit.rs:928-935`), and the family-namespaced
key (`project.rs:73-78`).
Existing check: `crates/daemon/tests/kernel_routes.rs:931`
`replayed_intents_return_one_receipt_and_projects_never_collide` and `:4096`
`a_receipt_is_keyed_by_route_family_and_a_blank_key_is_refused` pin the
mechanism for `kernel.commit`.
Impact: a replay that writes twice yields two live directives from one user
action; a replay that answers a foreign family's receipt reports a write that
did not happen.
Open questions:
- What the replayed response body carries for `record`: the plan says "replays
  without a second row"; the receipt result payload must include the object id so
  a retrying client can learn it (resolvable at implementation).

### directive-revise-supersedes-same-lineage-and-reapproves

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no `directive.revise` route
Guarantee: `directive.revise` on a live directive at revision N writes, in one
envelope, a successor on the same `("user_directive", source_id)` lineage at
revision N+1 with sensitivity Normal, invalidates the predecessor with
`superseded_by` set to the successor, records `AcceptedAdr` at `(explicit_user,
user_explicit)` on the successor so it is Approved and Visible on AutoInject,
and demotes every row whose support rested on the predecessor to its automatic
ceiling.
Check: `always` - after a committed revise: predecessor `invalidated_commit_seq`
is set and `superseded_by` names the successor; successor `source_revision ==
predecessor + 1`, same `source_kind` and `source_id`; successor latest admission
is Approved/automatic/explicit_user; any row whose latest admission before the
revise had `approval_object_id = predecessor` now has `effective_maturity <=
automatic_ceiling(its classes)`. `always` because every committed revise must
satisfy it.
Fault/timing angle: the authority cascade runs inside the same transaction as
the invalidation; a dependent demoted past `MAX_AUTHORITY_DEMOTIONS` is
deferred, not failed.
Required faults and enabling state: a live directive D1; a model row M admitted
with `approval_object_id = D1`; a revise of D1.
Confidence: high -
[evidence](evidence/directive-revise-supersedes-same-lineage-and-reapproves.md).
Verified `supersede_decision` guards (`admission.rs:1312-1349`),
`correct_decision_inner` succession and cascade (`write.rs:354-452`), and
`validate_successor` requiring a strictly greater revision on the same lineage
(`write.rs:1271-1284`).
Existing check: `crates/kernel/tests/kernel_admission.rs:788`
`correction_preserves_predecessor_history_and_emits_both_changes` and `:991`
`superseding_a_quarantined_predecessor_is_rejected` pin the Kernel half.
Impact: a revise that keeps the predecessor live serves two directives; a revise
that omits `AcceptedAdr` leaves the successor Candidate and the user's
correction silently disappears from injection.
Open questions:
- Whether the successor should inherit the predecessor's object scope (project
  or project plus `dir:`) or accept a new scope on revise; the plan body
  `{object_id, text, operation_key}` carries no scope (needs human input).

### directive-revoke-cascades-and-stops-serving

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no `directive.revoke` route
Guarantee: `directive.revoke` on a live directive invalidates the decision
object, records an `approval_revoked` change with the reason, demotes every
dependent whose support descended from it to its automatic ceiling, and after
the commit the Kernel's serving view no longer reports the directive Visible on
any surface.
Check: `always` - after a committed revoke: the object's
`invalidated_commit_seq` is set; a change of kind `APPROVAL_REVOKE_KIND` names
it; every dependent's latest admission is `ApprovalRevoked` with
`effective_maturity <= ceiling`; a `kernel.read` at the new tip returns no row
for the object. `always` because a revoked directive served is forbidden outcome
F3.
Fault/timing angle: a revoke concurrent with a transform pass that already read
the old snapshot serves the old row for that pass (snapshot semantics); the
guarantee binds the next read.
Required faults and enabling state: a live directive D1; a dependent M citing
D1; a revoke with a non-empty reason; a read after the commit.
Confidence: high -
[evidence](evidence/directive-revoke-cascades-and-stops-serving.md). Verified
`revoke_approval_inner` (`admission.rs:1351-1445`) and
`demote_authority_dependents` (`admission.rs:1447-1486`).
Existing check: `crates/kernel/tests/kernel_admission.rs:469`
`approval_revocation_fans_out_support_demotion_without_rewriting_history` and
`:2081` `an_unvisited_descendant_of_a_revoked_root_grants_nothing` pin the
Kernel half.
Impact: a revoked rule still injected is the "stale or revoked rule acquires
authority" failure the plan's verification names.
Open questions:
- OQ8 of the plan: `revoke` as irreversible `revoke_approval` versus
  `ExplicitReject`; the plan assumes revoke and offers `reject` separately (needs
  human input).

### directive-confirmed-from-promotes-by-fresh-record

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no `directive.record` route
Guarantee: `directive.record` with `confirmed_from = <candidate object id>`
writes a new directive whose rationale names the candidate and records
`ExplicitReject` on the candidate in the same envelope; the candidate's classes
stay `model_inference`, its disposition becomes Rejected and its visibility
`review_only`; no admission row on the candidate ever carries `explicit_user`.
Check: `always` - after the commit: a new live `user_directive` decision exists
whose `payload.rationale` carries `confirmed_from`; the candidate's latest
admission has `disposition='rejected'`, `visibility='review_only'`, and its
`source_class` equals its previous `source_class`; the candidate object is still
registered (not invalidated). `always` because in-place promotion is forbidden
outcome F7.
Fault/timing angle: the two writes share one envelope; a candidate already
Rejected or Quarantined still accepts a new directive but the second
`ExplicitReject` is a denied-at-same-state outcome the handler must tolerate.
Required faults and enabling state: a live `model_inference` candidate C that is
Visible only Labeled on ExplicitSearch; a record with `confirmed_from = C`.
Confidence: medium -
[evidence](evidence/directive-confirmed-from-promotes-by-fresh-record.md).
Verified `event_effect(ExplicitReject)` and `visibility_row(Rejected) ==
ReviewOnly` (`admission.rs:4144, 4251`) and that `disposition_request` reuses
the subject's stored classes (`commit.rs:566-600`); the handler itself is
proposed.
Existing check: none.
Impact: promoting in place would let model text acquire `explicit_user`
authority and become an approval root.
Open questions:
- Whether `confirmed_from` naming an object that is not a live `model_inference`
  decision is `invalid_params` or `not_found` (resolvable at implementation).

### directive-explicit-user-subject-guard-refuses-model-commit

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the guard does not exist; at HEAD the model tool can retire
a Visible directive
Guarantee: A `kernel.commit` operation `supersede_decision`, `retire_decision`,
or `Disposition` whose subject's latest admission row has `source_class =
explicit_user` is refused with the invalid-input outcome and writes nothing when
the caller's derived class is `model_inference`; the same operations on a
`model_inference` subject still succeed; the `directive.*` route is not subject
to the guard.
Check: `always` - for every committed `kernel.commit` whose resolved source
class is `ModelInference`, no touched object had a latest admission with
`source_class='explicit_user'` before the commit (equivalently, no
`admission_decisions` row with `source_class='explicit_user'` has a successor
row or an invalidation written by a commit whose producer is a plugin route).
`always` because a directive silenced by a model is forbidden outcome F2.
Fault/timing angle: the guard must read the subject's latest admission inside
the envelope (same-transaction cache) so a directive recorded moments earlier is
seen.
Required faults and enabling state: a Visible root-scoped directive; a model
tool `retire`, `revise`, or `explicit_reject` naming it; a control
`model_inference` row receiving the same operations.
Confidence: high -
[evidence](evidence/directive-explicit-user-subject-guard-refuses-model-commit.md).
Verified the gap at HEAD: `RetireDecision` checks scope only
(`commit.rs:849-853`), `SupersedeDecision` admits only the successor
(`commit.rs:787-848`), `Disposition` uses the subject's stored classes so the
Kernel's class-continuity check does not fire (`commit.rs:566-600`,
`admission.rs:268-274`); `Envelope::subject_admission` exists to read the latest
row (`admission.rs:1001-1008`).
Existing check: none; `crates/daemon/tests/kernel_routes.rs:4882`
`a_disposition_needs_a_live_admitted_decision` is adjacent but asserts liveness,
not class.
Impact: without the guard a model can retire or supersede a user directive with
nothing to notice; `eidnara_memory retire` at HEAD reads Visible rows and sends
`retire_decision`
(`packages/opencode-plugin/src/tools/eidnara-memory/execute.ts:665-690`).
Open questions:
- A compound-scoped directive (project plus `dir:`) is already `NotFound` to the
  Project-only `ScopeFilter` at HEAD (`eligibility.rs:86-93`,
  `scope.rs:1418-1420`); the guard is load-bearing for root-scoped directives and
  defensive for the rest (resolvable at implementation).
- Whether the guard should also cover the repository_policy class pair
  `(trusted_local_code, current_config)`; the plan names `explicit_user` only
  (needs human input).

### repo-policy-rules-derive-from-head-tree-or-nothing

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the `repository_policy` module (plan U5) does not exist
Guarantee: Every live `repository_policy` decision is backed by an artifact
whose `RepositoryProvenance.revision` is a commit oid in whose tree the cited
`AGENTS.md` blob exists with exactly the ingested bytes; when the project root
has no repository, HEAD is unborn, or gix fails, `reconcile` returns without any
Kernel write.
Check: `always` - for every live `repository_policy` decision, resolving
`provenance.revision` as a commit and reading the blob at the lineage's relative
path yields bytes whose SHA-256 equals the artifact digest; and after a
`reconcile` against a non-repository or unborn-HEAD root, `COUNT(*)` of objects
with `source_kind='repository_policy'` and of observations with
`observation_kind='config_present'` on such lineages is unchanged. `always`
because a rule from working-tree bytes is forbidden outcome F4.
Fault/timing angle: HEAD moving between oid resolution and tree read; the read
must be keyed by the resolved oid, not by HEAD at read time.
Required faults and enabling state: a project root that is not a repository; an
initialized repository with no commit; a repository whose `AGENTS.md` differs
between index/worktree and HEAD; a gix error injected by a corrupt object.
Confidence: medium -
[evidence](evidence/repo-policy-rules-derive-from-head-tree-or-nothing.md).
Verified gix is a daemon dependency with a HEAD-stability pattern
(`crates/kernel/src/applicability/checkout.rs:699-715`) and object hash
verification (`crates/daemon/src/git_reconcile.rs:462-472`); the module is
proposed.
Existing check: none.
Impact: a rule from uncommitted bytes has no revision identity and no provenance
witness; it would be served as Verified policy the repository never committed.
Open questions:
- Detached HEAD: the brief lists it among fail-closed inputs; the plan's U5 step
  1 lists only no repo, unborn HEAD, and gix error, and KTD4 treats detached HEAD
  as "Branch absent" while a commit oid still exists. Which applies to
  `reconcile`? (needs human input)

### repo-policy-bounds-fail-closed-per-file

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no module
Guarantee: A file over 64 KiB, a file that is not valid UTF-8, a file yielding
more than 64 units, or any file beyond the 32-file cap produces no units, no
artifact, and no `config_present` observation for that file lineage at that oid,
and a unit over 512 bytes after whitespace normalization is dropped; other files
in the same pass are unaffected.
Check: `always` - after a pass whose tree contains one over-bound `AGENTS.md`
and one in-bound `AGENTS.md`, the in-bound lineage has a live observation and
decisions at the new oid and the over-bound lineage has no observation whose
evidence names that oid. `always` because an admitted over-bound input is the
smallest falsifier of an initial-admissibility rule.
Fault/timing angle: none; intrinsic input bounds.
Required faults and enabling state: fixtures at 64 KiB exactly (admitted), 64
KiB plus one byte (refused), invalid UTF-8 bytes, 65 list items, 33 `AGENTS.md`
files, a 513-byte unit.
Confidence: medium -
[evidence](evidence/repo-policy-bounds-fail-closed-per-file.md). Bounds are plan
constants; the artifact cap and UTF-8 handling at ingest are verified
(`ingest.rs:73-75, 141-180`).
Existing check: none.
Impact: an unbounded file turns the transform pass into an unbounded Kernel
write; a non-UTF-8 payload would be classified Sensitive at ingest and poison
the trigger sensitivity of every rule it witnesses.
Open questions:
- When a file that previously produced rules becomes over-bound at a new HEAD,
  the plan says "no units and no observation"; whether its rev N rules are
  retired, left live, or superseded is unspecified. Leaving them live serves rules
  from an older HEAD (F6) (needs human input).
- Which 32 files are chosen when more exist: the plan does not fix an order; a
  deterministic order (path sort) is required for parity (resolvable at
  implementation).

### repo-policy-segmentation-is-deterministic

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no segmentation function
Guarantee: Segmentation is a pure function of the file bytes and relative path:
identical inputs yield the identical ordered sequence of (label, normalized
text, content digest, object id); fenced code blocks and headings yield no
units; each unit's label is the nearest preceding heading as `[Heading]` and,
for nested files, `[<dir>]`; object ids are
`rule_<sha256(source_id:rev:unit_digest)>`.
Check: `always` - golden fixtures: running segmentation twice on the same bytes
gives equal output; the fixture with a fenced block containing list syntax
yields no unit from inside the fence; a heading line yields no unit; the
recorded id for each unit equals the recomputed hash. `always` because the
function must hold for every input.
Fault/timing angle: none.
Required faults and enabling state: fixtures covering list items, paragraphs,
nested lists, fences with list markers inside, headings only, CRLF line endings,
trailing whitespace.
Confidence: medium -
[evidence](evidence/repo-policy-segmentation-is-deterministic.md). The grammar
is a plan statement; determinism is checkable once written.
Existing check: none.
Impact: nondeterministic ids break parity (R13), make every pass look like a
blob change, and produce spurious supersessions.
Open questions:
- Two units with identical normalized text in one file hash to one object id and
  the second `insert_decision` fails or folds; the plan does not say whether
  duplicates are dropped or disambiguated (resolvable at implementation, but
  affects ids and parity).

### repo-policy-unit-reconciliation-by-digest

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no module
Guarantee: When a file's blob changes at a new HEAD, one envelope per file
writes a `config_present` observation at lineage revision N+1 and, per unit
matched by content digest against the live rev N decisions: an unchanged unit
gets a successor with identical text at N+1 via `supersede_decision`, a changed
unit gets a successor with the new text, a removed unit is retired, and a new
unit is inserted; every predecessor is Superseded with `superseded_by` set or
invalidated by retirement; every live decision on the lineage is at revision
N+1; an unchanged blob at a new HEAD writes nothing.
Check: `always` - after the commit, every live decision on the lineage has
`source_revision == N+1`; every rev N decision has `invalidated_commit_seq` set;
for each unit digest present in both revisions, the rev N object's
`superseded_by` names a rev N+1 object with the same digest; removed digests
have no rev N+1 object; an identical blob at a different oid leaves the
lineage's max revision unchanged. `always` because a stale rev N decision live
beside rev N+1 is the falsifier.
Fault/timing angle: the plan must be computed from live rows inside the
envelope; a plan computed from a read before the writer is acquired can name a
predecessor another pass already invalidated.
Required faults and enabling state: three-unit file at HEAD X; a commit
rewriting unit two and deleting unit three (AE5); a commit changing only
whitespace outside units; a commit touching another file only.
Confidence: medium -
[evidence](evidence/repo-policy-unit-reconciliation-by-digest.md). Verified
`supersede_decision` lineage and revision rules (`write.rs:354-452, 1271-1284`),
`retire_decision` reachability (`write.rs:543-546`), and `insert_decision`
(`write.rs:139-162`); the matching logic is proposed.
Existing check: `crates/kernel/tests/kernel_admission.rs:788`
`correction_preserves_predecessor_history_and_emits_both_changes` pins
succession.
Impact: a missed retirement serves a deleted rule as current policy; a missed
supersession serves two revisions of one rule.
Open questions:
- "An unchanged blob writes nothing" conflicts with the per-file operation key
  `<path>:<oid>`: a new oid with an unchanged blob derives a new key, so "writes
  nothing" must be a blob-digest comparison before the commit, not a receipt
  replay (resolvable at implementation).
- How the matching treats a unit whose text is unchanged but whose heading label
  changed: same digest (label excluded) or new unit (label included) (resolvable
  at implementation, affects AE5).

### repo-policy-rule-verified-only-with-provenance-witness

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no module; the Kernel trigger mechanism is exercised by
kernel tests
Guarantee: Every live `repository_policy` decision whose latest admission is
Verified and Visible on AutoInject was admitted under `ConfigObserved` at
`(trusted_local_code, current_config)` with `trigger_object_id` naming a live
`config_present` observation on the decision's exact `(source_kind, source_id,
source_revision)` whose evidence is a live artifact carrying non-empty
`RepositoryProvenance` and sensitivity Normal; a decision admitted without such
a trigger lands with event `Other`, outcome Deny, Candidate, and is Hidden on
AutoInject.
Check: `always` - for every admission row on a `repository_policy` object with
`effective_maturity='verified'`: `event_kind='config_observed'`,
`trigger_object_id` joins a live observation with matching lineage triple and
`observation_kind='config_present'`, that observation's `evidence_id` joins live
`evidence_meta` with `sensitivity_class='normal'`, and the artifact's provenance
fields are non-empty. `always` because a rule without a witness reaching
Verified is forbidden outcome F5.
Fault/timing angle: the observation and the admissions share one envelope; an
observation inserted in a later commit than the admission cannot be its trigger.
Required faults and enabling state: the normal path; a simulated admission with
the trigger omitted (plan test "degrades to Other"); an `AGENTS.md` containing a
token the redactor detects (artifact becomes Secret; the rule must be Hidden); a
nested file at a different lineage revision than the observation.
Confidence: high -
[evidence](evidence/repo-policy-rule-verified-only-with-provenance-witness.md).
Verified `validate_trigger` (`admission.rs:2726-2795`), degradation to `Other`
(`admission.rs:1640-1644`), `Other` denies (`admission.rs:4159`), trigger
sensitivity composition (`admission.rs:1604-1605, 1631`), the automatic ceiling
Verified for the pair (`admission.rs:4197-4206`), and the ingest classification
(`ingest.rs:184-207`).
Existing check: `crates/kernel/tests/kernel_admission.rs:1292`
`a_config_observation_authorizes_only_a_config_event`, `:1323`
`an_observation_at_another_source_revision_cannot_authorize`, `:1339`
`an_invalidated_observation_cannot_authorize`, `:4150`
`an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it`,
`:4211` `a_trigger_whose_evidence_is_gone_cannot_admit`.
Impact: the witness is what "verified" means in this design; without it
repository text is as trusted as a model guess.
Open questions:
- Whether the observation's `evidence_id` is the artifact's evidence id (so its
  sensitivity folds in) or the observation carries `evidence_id: None` and cites
  the artifact only in the payload; the trigger check only folds sensitivity
  through `evidence_id` (resolvable at implementation; the plan says "evidence").

### repo-policy-per-file-commit-atomic-and-pass-converges

Origin: proposed design obligation (not implemented at HEAD)

Type: liveness
Reachability: default-production
Status: active
Exercised: not yet - no module
Guarantee: Each file's observation, unit changes, and admissions commit in one
Kernel envelope keyed `project.operation_key("repository_policy",
"<path>:<oid>")`; a process crash at any point in a pass leaves every file
lineage either wholly at the old revision or wholly at the new one; after a
crash, one complete subsequent pass at the same HEAD brings every in-bound file
lineage to the new oid, replaying the files that already committed.
Check: `sometimes` for the crash state plus `always` for convergence - a
campaign must reach the state "pass interrupted after file k of n committed"
(`sometimes`), and after one further complete pass every in-bound file lineage
has a live observation whose evidence provenance revision equals HEAD and the
receipt ledger shows the first k files replayed (`always`). Bound: one pass.
`sometimes` because the interrupted state is situation coverage, not a code
location.
Fault/timing angle: crash between the artifact ingest commit and the envelope
commit for one file (artifact present, no observation); crash between files;
crash after the last file but before the cache is set.
Required faults and enabling state: a tree with at least two `AGENTS.md` files;
a fault point after the first file's envelope commit (test-support hook or
process kill); a repeat pass.
Confidence: medium -
[evidence](evidence/repo-policy-per-file-commit-atomic-and-pass-converges.md).
Verified envelope poisoning and single-transaction commit (`envelope.rs:348-369,
1310-1330`) and that artifact ingest is a separate commit with its own intent
(`ingest.rs:583-600`), so a file is two commits rather than the plan's one.
Existing check: none.
Impact: a half-reconciled pass that is not re-run serves some files at HEAD and
others at an older commit.
Open questions:
- The plan says "one commit per file"; the Kernel's artifact ingest is a
  separate commit, so the atomic unit is the envelope and the artifact must be
  idempotently re-ingested (same intent) on retry. Record as a plan-versus-code
  clarification (resolvable at implementation).
- A failed file envelope (for example `Conflict` from a stale predecessor) must
  not stop later files, or must; the plan does not say (needs human input).

### repo-policy-cached-head-oid-is-advisory-and-reconverges

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no cache, no module
Guarantee: The cached HEAD oid per binding only suppresses a reconcile; it never
authorizes a write or a serving verdict; it is set to oid X only after every
in-bound file lineage has a committed observation at X; and after any set of
overlapping passes for one binding completes, every live `repository_policy`
decision for that binding carries one provenance revision, and that revision
equals the cached oid, so the next pass at that HEAD is a no-op and the next
pass at any other HEAD reconciles.
Check: `always` - whenever `reconcile` returns early on cache equality, the live
root observation's evidence provenance revision equals the cached oid; after two
overlapping passes at oids X then Y complete, the set of provenance revisions
over live `repository_policy` decisions has size one and equals the cache.
`always` because a cache that names an oid the Kernel does not hold is the
forbidden stale-rule window (F6).
Fault/timing angle: pass A resolves HEAD X, pass B resolves HEAD Y; B commits
and sets the cache to Y; A commits later (Kernel at X) and sets the cache to X
(heals on next pass) or A commits later while B's cache write lands last (Kernel
at X, cache Y; stuck until HEAD moves).
Required faults and enabling state: two transform passes for one project started
while HEAD changes between their oid reads; the Kernel single writer serializing
their commits in the opposite order from their cache writes.
Confidence: medium -
[evidence](evidence/repo-policy-cached-head-oid-is-advisory-and-reconverges.md).
The cache is a plan statement; the single-writer serialization is verified
(`envelope.rs:1310-1330`); the interleaving is constructed from those two facts.
Existing check: none.
Impact: a stale rule from an older HEAD stays live while the cache claims the
current HEAD; nothing re-reconciles until HEAD moves again.
Open questions:
- Whether the cache write is derived from the Kernel's committed state (the
  provenance revision of the root observation after commit) or from the pass's
  local oid; only the former closes the interleaving (needs human input, or
  resolvable if the implementation reads the committed observation).
- Whether overlapping passes for one project are possible in the daemon's m0
  compose path at all (one session or many) (resolvable at implementation by
  inspecting `transform.rs` concurrency).

## Relationship map

- `directive-record-self-approves-at-hardcoded-class-pair` is the legal
  witness every other N1 record starts from; `directive-revise-*` and
  `directive-revoke-*` consume its row; `directive-confirmed-from-*` is a
  specialization of it with an added `ExplicitReject`.
- `directive-explicit-user-unreachable-from-model-paths` and
  `directive-explicit-user-subject-guard-refuses-model-commit` together
  close the trust boundary: the first keeps the model from creating
  explicit_user rows, the second from altering them.
- `directive-operation-key-replay-is-idempotent` shares the Kernel receipt
  mechanism with `repo-policy-per-file-commit-atomic-and-pass-converges`;
  both rely on `(producer, operation_key, request_digest)`.
- `repo-policy-rules-derive-from-head-tree-or-nothing` and
  `repo-policy-bounds-fail-closed-per-file` are the initial-admissibility
  gates in front of `repo-policy-segmentation-is-deterministic`, whose
  output feeds `repo-policy-unit-reconciliation-by-digest`.
- `repo-policy-rule-verified-only-with-provenance-witness` is the
  state predicate every reconciliation outcome must land in; it is the N2
  twin of the N1 self-approval record.
- `repo-policy-per-file-commit-atomic-and-pass-converges` and
  `repo-policy-cached-head-oid-is-advisory-and-reconverges` together carry
  forbidden outcome F6; the first handles crashes, the second overlapping
  passes and the derived cache.
- Existing-surface catalogs this part leans on: S1 (admission mechanics)
  for every Kernel fact cited; N3 for scope matching of compound scopes; N4
  for the serving side of F3.
