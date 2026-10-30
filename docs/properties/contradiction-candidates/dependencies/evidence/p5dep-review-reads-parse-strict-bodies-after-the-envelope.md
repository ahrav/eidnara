# p5dep-review-reads-parse-strict-bodies-after-the-envelope

Authored at `3a3b916b7`; re-verified at the #1164 branch tip. Surface S7.

## Discovery trigger

Plan R6/U4: "An owner observational read lists candidates per decision" in
the same family as P3's `review.use_outcome` (#1145 OQ3-2), the routed
`review.*` family of `docs/host-wire-protocol.md` section 7.8. The doc at
`:766-770` and `:790` claims strict bodies, the project check before the body,
and the `disabled`/`store_unavailable` terminals. Lens: protocol contracts;
C13; D6.

## Evidence trail

- `docs/host-wire-protocol.md:794` opens section 7.8; `:796`: the envelope
  is `method`, `session_id`, `project_root`; twelve method literals (#1157
  added `review.use_outcome`, making four `review.*` methods); "a strict body
  that carries a field named `v` answers `invalid_params` as any unknown
  field does."
- `:770`: "Every body is parsed strictly ... an unknown field is an
  `invalid_params` transport error. The route binding decides scope: a
  `project_root` the binding does not accept answers the kernel routes'
  `invalid`/`project_mismatch` state before the body is read."
- `:790`: `review.list` and `review.read` "are local reads of the bound
  project's completed MemoryReviewer review outcomes"; an uninstalled Memory
  Store or non-MODULE authority answers `disabled`; a store that cannot be
  read answers `store_unavailable`; a closed activation gate does not disable
  them; "Bodies are parsed strictly after the envelope keys are removed; any
  other unknown field is `invalid_params`."
- `crates/daemon/src/memory_reviewer/wire.rs:1` header; `:23-26` literals
  (`USE_OUTCOME` at `:26`); `:33-60` `ListRequest`, `ReadRequest`,
  `AcceptRequest`, `UseOutcomeRequest` with `#[serde(deny_unknown_fields)]`.
- `wire.rs:131-139` `authority_project`: `Module -> Ok(project)`, `NotModule
  -> Err("disabled")`, `Err(_) -> Err("store_unavailable")`.
- `wire.rs:203-220` `bind_review`: `self.kernel_request::<T>(channel,
  request, operation)?` then `self.store()` or `terminal("disabled")`.
- `wire.rs:222-258` `handle_review_list`: after binding, `after` validated as
  lower-hex sha256 (`:77-90`), `limit` default `MAX_RECEIPT_PAGE`; the store
  read in `blocking`; `Err(_) => terminal("store_unavailable")` (`:255`).
- `crates/daemon/src/kernel_routes/mod.rs:415-441` `kernel_route_scope`:
  `management_binding` (session/route authorization), `project_root` required
  (`:422-426`), `project.accepts(requested_root)` else `project_mismatch`
  (`:428-432`), `kernel_store()` else its state.
- `kernel_routes/mod.rs:446-455` `kernel_request`: scope first, then `parse_request_body`.
- `mod.rs:461-473`: `ENVELOPE_KEYS = ["method", "kind", "session_id",
  "project_root"]` removed before `serde_json::from_value::<T>`; a failure is
  `invalid_params_error("invalid {operation}: {error}")`.
- `crates/daemon/src/kernel_routes/read.rs:34-55` `ReadRequest` for
  `kernel.read` with `deny_unknown_fields`; `read.rs:1-4` header.
- Tests: `crates/daemon/tests/memory_reviewer_wire.rs:15` (`disabled`
  without MODULE at `:25-34`; strict bodies at `:238-252`); `:409` (`v` field
  on `review.accept` at `:643-649`); `crates/daemon/tests/kernel_routes.rs:1487`
  `a_request_naming_another_project_root_is_refused_before_any_work`.
- `wire.rs:448-463+`: a test that parses the wire doc's `review.accept`
  paragraph and compares the documented terminal set to the produced set.

## Failure scenario

A P5 route that parsed its body before the project check would answer
`invalid_params` for a malformed body on a mismatched project, leaking the
field-level validation to a caller outside the project; the family's stated
order prevents that. A route accepting unknown fields would let a client send
a future field the daemon ignores, and the doc's "literals change only by an
edit to this section" discipline would be weakened.

## Timing windows and dependencies

The authority lookup runs off the async thread (`blocking`); a store failure
there is `store_unavailable`.

## What a test must construct

- A route bound to root A; a request with `project_root` B and a malformed
  body; assert `project_mismatch` and that no parse error is produced.
- Each method with one extra field; assert `invalid_params`.
- A root whose `memories` authority row is not MODULE; assert `disabled`.
- An injected failure in `memories_authority_for_route`; assert
  `store_unavailable` (no seam found at HEAD; see fault map F18).

## Investigation log

### Q: D6. Is `kind` an intended envelope key?

- Sources examined: `crates/daemon/src/kernel_routes/mod.rs:461-473`, `host-wire-protocol.md:829`, `:861`.
- Findings: the code strips `kind` with the three documented keys; the doc
  does not name it. A body with a `kind` field is accepted silently.
- Missing evidence: author intent; whether the transport ever adds `kind`.
- Conclusion: needs human input (documentation or code alignment).
