# plugin-commit-classes-are-closed-at-model-inference

## Discovery trigger

Plan KTD1: "`kernel.commit` keeps `derive_classes` as the closed table where
every plugin write is `model_inference`"; the route method is the trust
boundary, and the plan rejects adding a new plugin `source_kind` to that
table. R3 and AE2 depend on the table staying closed.

## Evidence trail

- `crates/daemon/src/kernel_routes/commit.rs:237-252` `derive_classes`:
  `"assistant" | "model"` -> `(ModelInference, AssistantInference)`;
  `"memory_classifier"` -> `(ModelInference, MemoryClassifierInference)`;
  `"user"` -> `(ModelInference, UserInferred)`; `_` -> `None`.
- `:255-264` `source_rank`: `ModelInference` is 5, the least trusted;
  `:267-275` `taint_rank`.
- `:283-316` `resolve_classes`: `derive_classes(..).ok_or(InvalidInput)`;
  an asserted source or taint with a lower rank than the derived one is
  `ClassOverDeclared`; a pair the kernel table forbids is `InvalidInput`.
  Because `ModelInference` has the highest rank, the only accepted asserted
  source is `model_inference` itself.
- `:451-467` `admit`: every insert and fresh successor is admitted with
  `EventKind::Other`, no trigger, no approval.
- `:787-850` `SupersedeDecision` admits a fresh successor with
  `plan.classes` (`:842-847`); `:870-900` `InsertObservation` likewise.
- `crates/kernel/src/admission.rs:4197-4219` `automatic_ceiling` for every
  `ModelInference` pair is `Candidate`; `:4159` `event_effect(Other)` keeps
  the current maturity, so a first decision is `Candidate`; `:4248-4251`
  `visibility_row(Candidate, Active)` is `ExplicitLabeled`.
- `CommitRequest` is `deny_unknown_fields` (`commit.rs:54-73`); the classes
  fields are `asserted_source_class` and `asserted_taint_class` only.
- Tests: `commit.rs:1071` `a_plugin_write_is_capped_at_inference_classes`;
  `:1117` `a_pair_is_accepted_exactly_when_not_raised_and_legal_for_admission`
  (every `SourceClass x TaintClass` for the four kinds);
  `crates/daemon/tests/kernel_routes.rs:1145`
  `a_plugin_route_cannot_declare_a_class_above_the_derived_one` (asserted
  `explicit_user` and `current_code` refused with `class_over_declared`;
  unknown kind `invalid_input`; the admitted row is labeled on
  `explicit_search` and absent on `auto_inject` and `auto_search`).
- The wire document does not specify this table; the contract is the code
  comment (`commit.rs:237-239`).

## Failure scenario

A new arm in `derive_classes` maps a plugin `source_kind` to
`TrustedLocalCode` or `ExplicitUser`, or `resolve_classes` accepts an
asserted class above the derived one. A model then writes a row whose
`source_kind` is `user_directive` or `repository_policy` with a `Verified`
ceiling, and the plan's required-phase partition (by `source_kind`) packs
model text as a rule.

## Timing windows and dependencies

- Classes are resolved once per request before any operation (`:974`,
  inside `handle_kernel_commit`, `:941`); every operation in the request
  shares them.
- `Disposition` operations do not use the resolved classes; they reuse the
  subject's stored classes (`:589-590`). That is the baseline the plan's KTD9
  guard changes (lead L4), and it is outside this record.

## What a test must construct

1. `derive_classes` over `["assistant", "model", "memory_classifier",
   "user"]` returns `Some((ModelInference, _))`; over `["user_directive",
   "repository_policy", "kernel_route", ""]` returns `None`.
2. `resolve_classes("user", Some("explicit_user"), None)` and
   `resolve_classes("user", None, Some("user_explicit"))` are
   `ClassOverDeclared`; `resolve_classes("user", None, Some("current_config"))`
   is `ClassOverDeclared`.
3. Route request with `source_kind: "user_directive"` and one
   `insert_decision`: state `invalid`, reason `invalid_input`, tip unchanged.
4. Route request with `source_kind: "user"`, `insert_decision` of
   `decision_kind: "adr_accepted"`: the stored admission row has
   `source_class == model_inference`, `taint_class == user_inferred`,
   `effective_maturity == candidate`, `visibility == explicit_labeled`,
   `event = other`; `read_visible` on `auto_inject` omits it.
5. Route `supersede_decision` of that row: the successor's admission row has
   the same classes and `event = other`.

## Investigation log

### Q: Is the table documented outside the code?

- Sources examined: `docs/host-wire-protocol.md` (grep `model_inference`,
  `derive`, `asserted_source_class`, `kernel.commit`),
  `docs/memory-reviewer-operations.md`.
- Findings: no occurrence; the `kernel.commit` body shape is specified by
  the shared kernel client in `packages/opencode-plugin/src/shared/kernel-client/`
  and the route code.
- Missing evidence: none; the code comment is the only contract text.
- Conclusion: resolved with answer: the contract lives in code; the plan's
  U3 adds the directive methods to the wire document but leaves this table
  undocumented there.
