# admission-chain-charges-before-decode-and-refuses-effect-free

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit proposes fusing the `Value` parse with the typed decode so a body
is read once. That moves the resident charge relative to allocation and moves
the admission decision relative to the post-parse side effects. The contract
to preserve is the order of the four pre-dispatch steps, the pool the charge
draws on, the two refusal codes, and the absence of any dispatch-side effect
on refusal. The parent's [E2][e2] owns the charge's lifetime; this record owns
its position and magnitude.

## Evidence trail

- [`Handler::handle`][handle] runs, in order:
  [`enforce_request_byte_cap`][bytecap],
  [`value_footprint_bound`][footprint], `ctx.try_reserve_resident(footprint)`,
  then `serde_json::from_slice::<Value>` with failure mapped to `Value::Null`.
  The charge binding `_parse_charge` lives until the function returns, so it
  spans dispatch and [`settle_prepared`][settle].
- [`try_reserve_resident`][reserve] is `self.scratch.try_charge(bytes)`; its
  doc says reserving after allocation lets the allocation escape the envelope.
  [`resident_capacity`][capacity] is `self.scratch.capacity()`. The scratch
  pool is one host-wide [`ByteBudget`][pools] sized by
  [`SCRATCH_RESERVED_BYTES`][scratchconst]; the constant evaluates to
  184,878,336 bytes, about 176 MiB, with `MAX_BODY_LEN` at 64 MiB.
- [`ByteBudget::try_charge`][try-charge] uses `try_acquire_many_owned` and
  returns `None` on a `u32` overflow or a short pool; it never awaits.
- The refusal helpers are [`request_too_large_error`][toolarge], which emits
  `invalid_params`, and [`resident_capacity_error`][queuefull], which emits
  `queue_full`. `handle` selects the first when the footprint scan overflows or
  when `footprint > resident_capacity()`, and the second otherwise.
- The footprint arithmetic is `nodes * size_of::<Value>() * VALUE_NODE_SLACK +
  string_bytes * RETAINED_STRING_COPIES + VALUE_ENVELOPE_BYTES` with
  [slack 2, envelope 4096, copies 3][copies]; the copies doc names the three
  holders: the decoded `Value`, the `WireMessage` `original`, and the
  `WireBlock` `original`. [`WireMessage`][wiremsg] and [`WireBlock`][wireblock]
  decode through a `Value` and keep it as `original`.
- The post-parse side effects sit inside the transform arm:
  [`freeze_prompt_surface_selection`][freeze] and the
  [`transform_route_channels` insert][routechan] run before
  [`ticket.accept()`][accept]; the [`TransformDispatchTicket`][ticket] is
  created by `handle_transform_dispatch`, which a refused body never reaches.
- [`RequestOutcome::error`][outcome] sets `retry_after_ms: None`. The wire
  contract at [§6.3][wire63] names `invalid_params` for the 1 MiB and 32 MiB
  limits; [§7.5.1][wire751] scopes the `queue_full` retry hint and the
  `schema_violation` code to Synapse. The string `request_too_large` appears
  only in the [direct-host fixture control channel][fixture].
- The plugin [pages bodies over 512 KiB][paging], so the over-cap arms need
  constructed input; the plugin sends both [`method` and `kind`][plugin].

## Failure scenario

A fused design starts the typed decode before the charge is held. A body that
fits the cap but not the pool allocates its `WireMessage` trees, then fails
the reservation; the allocation escaped the envelope while peers were
admitted. A design that decodes before deciding reaches the prompt freeze and
the route-channel insert for a body the current code refuses, so a refused
request leaves session state behind. A design that awaits the reservation
parks while holding the pending and task permits, changing admission order for
every peer.

## Timing windows and dependencies

None in time for the ordering clause. The `queue_full` arm depends on
concurrent holders of the shared scratch pool, which is A3's situation. The
charge magnitude depends on `RETAINED_STRING_COPIES` matching what the typed
decode retains; a change to either `Deserialize` impl changes the truth of the
bound without changing the constant.

## What a test must construct

Drive `Handler::handle`, not `dispatch_value`; `RequestCtx` is transport-private
so [`dispatch_value_for_test`][testentry] enters after admission. Bodies: one
over 1 MiB without a transform-class discriminator; one over 32 MiB with one;
one whose footprint exceeds about 176 MiB (a scalar-dense body bounds far above
its wire size); one admitted while a barrier holds a concurrent charge. Assert
the code, that no ticket, route channel, prompt freeze, page staging, or store
row changed, and that the pool's available bytes return when the future ends.
For the magnitude clause, decode a large text block and count owned copies in
the resulting `TransformRequest`. The
[ingress checks](../existing-checks.md#ingress-admission-and-decode)
cover the cap and footprint as pure functions; none enters at `handle`.

The real-host fixture `FixtureProcess` in
`crates/daemon/tests/support/direct_host.rs`, used by `direct_host.rs:48-70`,
reaches `Handler::handle` over the ring and is the seam. Count copies
structurally (the `Value` node, the `WireMessage` `original`, and the
`WireBlock` `original` for one known block) or through a counting allocator,
because the decoded type exposes no copy count.

## Investigation log

### Q: Does the context module owe a `retry_after_ms` on `queue_full`?

- Sources examined: [`RequestOutcome::error`][outcome], [§7.5.1][wire751],
  [§6.3][wire63], [`resident_capacity_error`][queuefull].
- Findings: The helper carries no hint and the outcome constructor fixes
  `retry_after_ms: None`. The contract's hint rule is written for Synapse
  `embed.query`; routed transform bodies are opaque to the contract.
- Missing evidence: A written rule for the context module.
- Conclusion: needs human input.

### Q: Is the page staging budget the intended cover for the assembled whole?

- Sources examined: the [page apply][pageapply] call into
  `handle_transform_unpaged_value`, the [512 KiB page cap][hostpagecheck], and
  the staging constants at [`:742-743`][hostpage] (128 MiB staged maximum).
- Findings: The assembled `Value` reaches the typed decode with the per-page
  reservation history only; no `value_footprint_bound` runs on the whole.
- Missing evidence: A statement of which budget covers the assembled tree.
- Conclusion: needs human input.

### Q: Is sharing the Synapse-sized scratch slice intended?

- Sources examined: [`SCRATCH_RESERVED_BYTES`][scratchconst] and its doc,
  the [pool split][pools].
- Findings: The doc lists Synapse terms only; the transform footprint is not
  in the sizing or in `validate_serving_limits`.
- Missing evidence: A sizing statement that names the transform footprint.
- Conclusion: needs human input.

[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
[handle]: ../../../../../crates/daemon/src/lib.rs#L11882-L11904
[bytecap]: ../../../../../crates/daemon/src/lib.rs#L15574-L15590
[footprint]: ../../../../../crates/daemon/src/lib.rs#L15516-L15552
[copies]: ../../../../../crates/daemon/src/lib.rs#L15511-L15514
[toolarge]: ../../../../../crates/daemon/src/lib.rs#L15559-L15565
[queuefull]: ../../../../../crates/daemon/src/lib.rs#L15567-L15572
[freeze]: ../../../../../crates/daemon/src/lib.rs#L8085-L8086
[routechan]: ../../../../../crates/daemon/src/lib.rs#L8113-L8116
[accept]: ../../../../../crates/daemon/src/lib.rs#L8141
[ticket]: ../../../../../crates/daemon/src/lib.rs#L581-L638
[pageapply]: ../../../../../crates/daemon/src/lib.rs#L9487-L9510
[hostpage]: ../../../../../crates/daemon/src/lib.rs#L742-L743
[hostpagecheck]: ../../../../../crates/daemon/src/lib.rs#L9375-L9381
[settle]: ../../../../../crates/daemon/src/lib.rs#L12130-L12145
[testentry]: ../../../../../crates/daemon/src/lib.rs#L12568-L12577
[wiremsg]: ../../../../../crates/memory-store/src/lib.rs#L126-L143
[wireblock]: ../../../../../crates/memory-store/src/lib.rs#L250-L264
[reserve]: ../../../../../crates/host-runtime/src/handler.rs#L474-L484
[capacity]: ../../../../../crates/host-runtime/src/handler.rs#L486-L491
[outcome]: ../../../../../crates/host-runtime/src/handler.rs#L230-L235
[pools]: ../../../../../crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: ../../../../../crates/host-runtime/src/config.rs#L21-L31
[try-charge]: ../../../../../crates/host-runtime/src/wire.rs#L430-L442
[paging]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L759-L761
[fixture]: ../../../../../crates/daemon/tests/direct_host.rs#L285-L290
[wire63]: ../../../../host-wire-protocol.md#L308
[wire751]: ../../../../host-wire-protocol.md#L440
