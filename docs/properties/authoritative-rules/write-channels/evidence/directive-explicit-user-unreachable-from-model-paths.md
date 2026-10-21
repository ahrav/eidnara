# directive-explicit-user-unreachable-from-model-paths

## Discovery trigger

Plan requirement R1 ("no model tool call can reach that route") and R3; KTD1
("the route method is the trust boundary"); KTD9 context. Forbidden outcome
F1: an admission row at `explicit_user` produced by a model-mediated commit.

## Evidence trail

Verified at HEAD:

- `crates/daemon/src/kernel_routes/commit.rs:236-248` `derive_classes` is a
  closed table: `assistant|model`, `memory_classifier`, `user` all map to
  `SourceClass::ModelInference` with a taint that records the topic. Any other
  `source_kind` is `None`, which `resolve_classes` turns into
  `InvalidReason::InvalidInput` (`commit.rs:279`).
- `commit.rs:271-312` `resolve_classes`: an asserted source or taint that
  ranks more trusted than the derived one is `ClassOverDeclared`; the
  derived pair can only be lowered.
- `commit.rs:451-470` `admit` records `EventKind::Other` with the plan's
  classes; `EventKind::Other` can never raise support (`admission.rs:4184-
  4195`), so a plugin write lands at or below its ceiling.
- `commit.rs:566-600` `disposition_request` uses the subject's stored classes,
  which is why the guard record exists; it does not create new rows.
- `crates/daemon/tests/kernel_routes.rs:1145`
  `a_plugin_route_cannot_declare_a_class_above_the_derived_one` pins the
  over-declaration refusal.
- Test-only exceptional writer: `crates/daemon/tests/kernel_routes.rs:4355-
  4386` `seed_approval` writes `explicit_user` rows through the store API;
  `crates/daemon/src/lib.rs:13838-13889` test-support helpers write
  `trusted_local_code` rows. Both are `cfg(test)` or `test-support`.
- MemoryReviewer acceptance (`crates/daemon/src/memory_reviewer/settlement.rs:
  1116-1154`) commits under producer `eidnara-daemon/review-accept` with
  actor `memory-reviewer`; the brief and tracker settle it at
  `model_inference`.

Plan-only: the `directive.*` methods and the claim the model tool client
"never constructs" them. The client at HEAD has no such method because the
method does not exist; the obligation is that U3 and U4 keep it out of the
tool client's method set.

## Failure scenario

Legal witness: the model calls `eidnara_memory create` with `source_kind:
user` and directive-shaped text; `derive_classes("user")` yields
`(ModelInference, UserInferred)`; the row is Candidate, `explicit_labeled`,
Hidden on AutoInject (AE2).

Smallest violating traces:

1. A new `derive_classes` arm `"user_directive" => (ExplicitUser,
   UserExplicit)`: any plugin body naming that `source_kind` becomes an
   approval root.
2. A new `kernel.commit` operation literal (for example `record_directive`)
   whose handler hard-codes the pair: the model tool client can build the
   same JSON.
3. A daemon-internal module calling `record_admission` with caller-chosen
   classes on model text (the writer inventory is open).

## Timing windows and dependencies

None; the invariant is over a static table and the ledger.

## What a test must construct

- Static: a test iterating every string `derive_classes` accepts and
  asserting `ModelInference`.
- Dynamic: a `kernel.commit` with `asserted_source_class: "explicit_user"`
  and each accepted `source_kind`, asserting `ClassOverDeclared` and no new
  `admission_decisions` row.
- Ledger: after a mixed campaign (directive records, model writes, reviewer
  acceptances), `SELECT producer FROM commit_log JOIN admission_decisions ...
  WHERE source_class='explicit_user'` returns only the directive route's
  producer.
- Client: the kernel-client wire module's method union contains no
  `directive.*` literal in the tool client build (TypeScript type test).

## Investigation log

### Q: Is there any production writer of `explicit_user` rows at HEAD?
- Sources examined: grep for `SourceClass::ExplicitUser` under
  `crates/daemon/src` and `crates/kernel/src`.
- Findings: matches are in `resolve_classes` ranking, test seeds, and
  test-support; no production route constructs the pair.
- Missing evidence: a full inventory of `record_admission` callers in
  non-route daemon modules.
- Conclusion: unresolved, needs a caller inventory at implementation.

### Q: Does the wire reject unknown method names rather than route them?
- Sources examined: `docs/host-wire-protocol.md` §7.8 method list (plan cites
  `:737-767`).
- Findings: a method literal must be listed to dispatch; the plan adds four.
- Missing evidence: how the tool client's method union is bounded.
- Conclusion: unresolved, needs the U4 client change (resolvable at
  implementation).
