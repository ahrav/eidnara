# reporter-class-derived-never-asserted-up

## Discovery trigger

Plan P4 rule A5 and index decision D3: authority never derives from model
confidence. A dispute records who filed it; that class must come from the
route's derivation, never from a stronger claim by the client.

## Evidence trail

- `derive_classes` maps `assistant` and `model` to
  `(ModelInference, AssistantInference)`, `memory_classifier` to
  `(ModelInference, MemoryClassifierInference)`, `user` to
  `(ModelInference, UserInferred)`, and anything else to `None`
  (`crates/daemon/src/kernel_routes/commit.rs:368-380`).
- `resolve_classes` refuses an asserted class ranked above the derived one
  with `ClassOverDeclared` and accepts one at or below it
  (`commit.rs:411-444`; ranks at `:382-404`).
- The handler passes the resolved taint to `compose_reason`
  (`commit.rs:1111-1125`), which renders it as `reporter_taint`
  (`crates/daemon/src/kernel_routes/dispute.rs:85-89`).
- `disposition_request` builds the admission request with the subject's
  prior classes (`commit.rs:725-726`), so the row's class columns are the
  subject's; the Kernel refuses a request whose classes differ from the
  prior (`crates/kernel/src/admission.rs:268-274`).
- The command sends `sourceKind: "user"`
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:248`).
- The route stores `actor` and `cause` as sent; neither enters the class
  derivation (`IntentRequest`, `crates/daemon/src/kernel_routes/project.rs:140-146`).

## Failure scenario

A client sends `source_kind: "assistant"` with `asserted_taint_class:
"user_explicit"`. If `resolve_classes` clamped instead of refusing, or
`compose_reason` read the asserted class directly, the reason would name
an explicit-user reporter for model output.

## Timing windows and dependencies

None. The derivation is a pure function of the request.

## What a test must construct

All constructed except the tool arm:

- `asserted_taint_class: user_explicit` under `assistant` and `user`:
  `class_over_declared`, nothing written; an unknown `source_kind`:
  `invalid_input` (`a_dispute_reporter_class_is_derived_and_never_asserted_up`,
  `crates/daemon/tests/kernel_routes.rs:6025`).
- `user` lowered to `assistant_inference`: the reason records
  `AssistantInference` (same test).
- A command-shaped and a tool-shaped intent on one store: `user_inferred`
  and `assistant_inference`, row classes equal to the prior
  (`kernel_routes.rs:5723`). The subject was written under `assistant`, so
  the `user` dispute constructs a reporter class that differs from the
  subject's.
- The command sends `source_kind: "user"` on preview and commit
  (`memory-mark-command.test.ts:181`).
- Not constructed: the model tool arm's own surface (#1161).

The check is a per-request route oracle (portfolio refinement R4). It
reads the request's `source_kind`, `asserted_source_class`, and
`asserted_taint_class` (`commit.rs:115-119`). `commit_log` stores no
`source_kind`, and the admission row's `source_kind` column holds the
subject's facts, so no store scan can evaluate it.

## Investigation log

### Q: Does the route verify `source_kind` against the binding?

- Sources examined: `kernel_route_scope`
  (`crates/daemon/src/kernel_routes/mod.rs:415-441`), `RouteScope`
  (`:405-412`), `handle_kernel_commit` (`commit.rs:1078-1207`).
- Findings: no. `RouteScope.harness` is bound but `handle_kernel_commit`
  reads only `scope.project` and `scope.store` (`commit.rs:1126`,
  `:1141`, `:1144`); `source_kind` is whatever the body says. The
  derivation bounds what a claim can reach: a client can claim `user`
  (taint rank 2) from any surface but never `user_explicit`.
- Missing evidence: an owner decision on binding `source_kind` to the
  surface.
- Conclusion: needs human input; recorded as portfolio gap G1 in
  `commit-actor-cause-and-source-kind-are-caller-asserted`
  ([`../../disposition-path/catalog.md`](../../disposition-path/catalog.md)).
