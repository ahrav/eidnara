# Open questions: proposed write channels N1 and N2

Questions the plan and the code at HEAD `08de9051a` do not settle. Each is
tagged `(needs human input)` or `(resolvable at implementation)` with the
evidence that would resolve it. None is answered here.

## N1 directive channel

### OQ-N1-1 Scope of a revised directive (needs human input)

The `directive.revise` body is `{object_id, text, operation_key}` (plan U3
step 3). A directive may carry a project scope or project plus Entity `dir:`
(step 2). Whether the successor inherits the predecessor's `scope_id`, or the
body may change it, is not stated. Evidence to resolve: a plan amendment or
the U3 body definition. Affects record
`directive-revise-supersedes-same-lineage-and-reapproves`.

### OQ-N1-2 Revoke versus reject as the default "forget" (needs human input)

Plan OQ8. `revoke_approval` is irreversible invalidation with cascade
(`admission.rs:1351-1445`); `ExplicitReject` keeps the object as Rejected
(`review_only`) and bars later correction (`admission.rs:1343-1349`). Both
demote dependents. Evidence: user decision. Affects
`directive-revoke-cascades-and-stops-serving`.

### OQ-N1-3 Guard coverage beyond `explicit_user` (needs human input)

KTD9 names `source_class = explicit_user` only. A model `retire_decision` on a
`(trusted_local_code, current_config)` repository rule succeeds at HEAD and
would be re-inserted as "new" by the next reconcile (digest matching against
live rows), producing churn and a window where the rule is absent. Whether the
guard should also cover the repository class pair, or whether re-insertion is
the accepted remedy, is a design choice. Affects
`directive-explicit-user-subject-guard-refuses-model-commit`.

### OQ-N1-4 Compound-scope resolution for the directive route (resolvable at implementation)

`ProjectScope::names_project` evaluates `scope_matches` with a Project-only
context (`crates/kernel/src/eligibility.rs:86-93`); an Entity term with no
context value is `Uncertain` (`crates/kernel/src/scope.rs:1418-1420`), so a
scope with Project plus `dir:` fails `ScopeFilter::matches` at HEAD. If
`directive.revise` and `directive.revoke` resolve their target through
`scoped_object_state`, a path-scoped directive is `NotFound` to its own
route. Evidence: the U3 handler's scope resolution (for example a filter that
checks only the Project term's presence and value, which is what
`ScopeFilter` was built to answer) or U1's `InjectionScope` reused with the
directive's own `dir:` value. Also affects N2 for nested files and the
subject guard's compound-scope arm.

### OQ-N1-5 Replay result payload and `confirmed_from` validation (resolvable at implementation)

The receipt `result` for a replayed `record` must carry the object id so a
retrying client learns it (`envelope.rs:1301-1308` returns the stored result
string). `confirmed_from` naming a non-model, retired, or unknown object needs
a defined outcome. Evidence: the U3 handler and its tests.

## N2 repository policy

### OQ-N2-1 Detached HEAD (needs human input)

The brief lists "detached" among fail-closed inputs for N2; plan U5 step 1
lists no repo, unborn HEAD, and gix error; KTD4 treats detached HEAD as
"Branch absent" while a commit oid exists. A detached HEAD has a tree, so the
plan as written reconciles it. Evidence: a plan amendment. Affects
`repo-policy-rules-derive-from-head-tree-or-nothing`.

### OQ-N2-2 How "an unchanged blob writes nothing" is keyed (resolvable at implementation)

The per-file operation key is `<path>:<oid>` (U5 step 6), new at every HEAD;
the cache is per binding (step 1). Neither captures "same blob, new oid". The
module must compare the blob digest with the live observation's artifact
digest before committing. Evidence: the module's pre-commit comparison and a
test with a commit touching only another file. Affects
`repo-policy-unit-reconciliation-by-digest`.

### OQ-N2-3 Fate of rev N rules when a file becomes over-bound or non-UTF-8 (needs human input)

U5 step 2 says such a file yields "no units and no observation"; step 4
reconciles per unit. Skipping the file leaves rev N rules live at an older
HEAD (forbidden outcome F6); treating it as zero units retires them. Evidence:
a plan statement. Affects `repo-policy-bounds-fail-closed-per-file`.

### OQ-N2-4 Deterministic file selection at the 32-file cap (resolvable at implementation)

Which 32 files are taken when more exist is unstated; parity (R13) requires a
stated order (for example lexicographic path). Evidence: the module.

### OQ-N2-5 Duplicate units and label participation in the digest (resolvable at implementation)

Two identical normalized units in one file collide on
`rule_<sha256(source_id:rev:unit_digest)>`; the second `insert_decision`
fails and poisons the file's envelope. Whether the heading label is hashed
decides whether a heading rename supersedes every unit beneath it. Evidence:
the segmentation specification and golden fixtures. Affects
`repo-policy-segmentation-is-deterministic` and
`repo-policy-unit-reconciliation-by-digest`.

### OQ-N2-6 Whether one failed file stops the pass (needs human input)

A per-file envelope can fail (`Conflict` from a stale predecessor, `NotFound`,
ingest refusal). Continuing reconciles the other files; stopping keeps the
pass atomic in outcome but leaves earlier files advanced either way. The
cache write rule depends on the answer. Evidence: a plan statement. Affects
`repo-policy-per-file-commit-atomic-and-pass-converges`.

### OQ-N2-7 Source of the cache value (needs human input)

Setting the cache from the pass's locally resolved oid admits the inverted
commit-order window in
`repo-policy-cached-head-oid-is-advisory-and-reconverges` (Kernel at X, cache
Y, HEAD Y, no reconcile until HEAD moves). Setting it from the Kernel's
committed state (the live root observation's artifact provenance revision
after the file loop) closes it. Evidence: a plan amendment choosing one.

### OQ-N2-8 Overlapping passes for one project (resolvable at implementation)

Whether the m0 compose path can run two transform passes for one bound project
concurrently was not examined in this pass. Evidence: `crates/daemon/src/
transform.rs` request handling and any per-project serialization. If
overlapping passes cannot occur, OQ-N2-7's window reduces to the benign
HEAD-moved case.

### OQ-N2-9 Observation object id and evidence citation (resolvable at implementation)

The `config_present` observation's object id is unnamed by the plan and must
be unique per revision; its `evidence_id` must cite the artifact for the
Kernel to fold provenance-derived sensitivity into the trigger
(`admission.rs:2745-2770` allows a null citation). Evidence: the module.
Affects `repo-policy-rule-verified-only-with-provenance-witness`.

### OQ-N2-10 Two Kernel commits per file (plan-versus-code clarification, resolvable at implementation)

Plan KTD2 and U5 step 6 say one commit per file. Artifact ingest is a
store-level operation with its own commit (`crates/kernel/src/cas/ingest.rs:
583-600`), so each file is an ingest commit followed by the envelope commit.
The atomic unit the design needs (observation, unit changes, admissions) fits
in the envelope; the ingest is idempotent under its own intent. No
requirement is contradicted; the plan text should say two commits.

## Verified facts the design relies on (recorded for the implementer)

- `evaluate_admission` refuses a prior with different classes
  (`admission.rs:268-274`); re-admitting a subject under new classes is
  impossible, so promotion in place cannot happen through `record_admission`.
- `disposition_request` admits under the subject's stored classes
  (`crates/daemon/src/kernel_routes/commit.rs:566-600`), so the Kernel's
  class-continuity check does not stop a model-mediated disposition on a
  directive; the KTD9 guard is required for all three operations.
- `Envelope::subject_admission` (`admission.rs:1001-1008`) reads the latest
  admission through the same-transaction cache; it is the natural read for
  the guard.
- `validate_successor` requires a strictly greater revision, not `+1`
  (`crates/kernel/src/slice/write.rs:1281-1283`).
- `validate_trigger` matches the exact lineage triple and composes evidence
  sensitivity (`admission.rs:2726-2795`).
- Affirmative provenance keeps an inspected Normal payload Normal
  (`ingest.rs:184-207`).
- `ProjectBinding::operation_key` namespaces by digest and family
  (`crates/daemon/src/kernel_routes/project.rs:73-78`).
- Line-reference corrections: the authority cascade call in
  `correct_decision_inner` is at `write.rs:448-452` (plan: `443-448`); the
  test seed is `kernel_routes.rs:4355-4386` (plan: `4356-4384`).
