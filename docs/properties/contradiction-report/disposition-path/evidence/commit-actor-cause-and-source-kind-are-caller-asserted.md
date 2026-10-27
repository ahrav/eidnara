# commit-actor-cause-and-source-kind-are-caller-asserted

## Discovery trigger

Portfolio gap G1 ([`../portfolio-evaluation.md`](../portfolio-evaluation.md)):
the reporter identity a dispute records is built from fields the caller
asserts. The specification's Q4 asked whether the route verifies `actor`
against the bound harness. PR A records the reporter taint in the reason,
which makes the provenance of `source_kind` part of the dispute record.

## Evidence trail

- `IntentRequest` carries `producer`, `operation_key`, `request_digest`,
  `actor`, and `cause`, all `String`, under `deny_unknown_fields`
  (`crates/daemon/src/kernel_routes/project.rs:138-146`).
  `into_intent` copies `producer`, `actor`, and `cause` unchanged and
  namespaces only the key by project and family (`project.rs:148-165`).
- `CommitRequest` carries `source_kind` and the optional asserted classes
  (`crates/daemon/src/kernel_routes/commit.rs:108-127`); `resolve_classes`
  derives the classes from `source_kind` and refuses an assertion above
  the derivation (`commit.rs:411-444`); the handler passes the resolved
  taint to the dispute reason (`commit.rs:1109-1123`).
- `kernel_route_scope` binds the store, the project, the root, the
  harness, and the capabilities (`crates/daemon/src/kernel_routes/mod.rs:415-441`;
  `RouteScope` at `:405-412`). `handle_kernel_commit` reads only
  `scope.project` and `scope.store` (`commit.rs:1124`, `:1139`, `:1142`);
  `scope.harness` is not read on this route.
- `management_binding` requires a non-empty `session_id` and refuses one
  that differs from the channel's bound session with `session_mismatch`
  (`crates/daemon/src/lib.rs:6596-6631`). `kernel_request` then strips
  `method`, `kind`, `session_id`, and `project_root` before parsing the
  body (`ENVELOPE_KEYS`, `mod.rs:461`; `parse_request_body`,
  `:463-474`), so the session reaches no Kernel row.
- The Kernel redacts `actor` and `cause` before the `commit_log` insert
  (`RedactedIntent`; insert at `crates/kernel/src/envelope.rs:1420-1436`).
- The commands send `actor = "user:opencode"`
  (`packages/opencode-plugin/src/hooks/context/command-handler.ts:391`) or
  `"user:pi"` (`packages/pi-plugin/src/commands/eidnara-memory-mark.ts:18`)
  and `sourceKind: "user"`
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:220`).

## Failure scenario

A model tool, or any client holding a bound channel, sends a dispute with
`actor: "user:opencode"`, `cause: "/eidnara-memory-mark ..."`, and
`source_kind: "user"`. The route stores the claims; the dispute row's
reason records `reporter_taint=user_inferred`. A reader treating `actor`
as verified would attribute the dispute to the user. The derivation keeps
the taint at or below `user_inferred`, so standing is unaffected.

## Timing windows and dependencies

None in time. The record depends on the binding's session check, which
runs before body parsing for every `kernel.*` route.

## What a test must construct

- An `actor` naming a harness other than the binding's, stored as sent.
  Constructed: `kernel_routes.rs:5723` sends `user:opencode` and
  `agent:opencode` on a `test` binding (`route_identity(&project, "test",
  "session-a")`, `crates/daemon/tests/kernel_routes.rs:70`) and reads them
  back from `commit_log`.
- `source_kind: "user"` accepted on any binding, with the reporter taint
  derived from it. Constructed by the same test.
- A `producer` naming another plugin. Not constructed.
- A redacted `actor` or `cause`. Not constructed.
- A `session_id` that differs from the channel's bound session, refused
  `session_mismatch`. Not constructed in `kernel_routes.rs`; the check is
  read in code.

## Investigation log

### Q: Does the route verify `actor` against `RouteScope.harness`?

- Sources examined: `mod.rs:405-441`, `commit.rs:1076-1205`,
  `project.rs:138-165`.
- Findings: no. `harness` is bound and unused on `kernel.commit`; `actor`
  is copied from the body.
- Missing evidence: an owner decision on whether the route should derive
  or check `actor` from the binding.
- Conclusion: needs human input.

### Q: Is `source_kind` bound to the calling surface?

- Sources examined: `commit.rs:368-380`, `:411-444`, `:1109-1123`.
- Findings: no. Any bound client may send any of `assistant`, `model`,
  `memory_classifier`, or `user`; the derivation bounds the resulting
  taint at `user_inferred` for the strongest claim.
- Missing evidence: an owner decision; the #1161 tool arm will send
  `assistant`.
- Conclusion: needs human input.
