# scratch-pool-shortfall-reaches-the-parse-reservation

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

A1's `queue_full` arm and its release path are evaluated only when the scratch
pool is short while the footprint fits the ceiling. One request at a time never
produces that state: the pool is about 176 MiB and the plugin pages at
512 KiB, whose footprint is about 1.6 MiB. A green admission suite can pass
without the transient refusal ever running. This record asks a campaign to
witness the preconditions, not the handler's outcome.

## Evidence trail

- [`Handler::handle`][handle] calls `ctx.try_reserve_resident(footprint)` and
  selects [`resident_capacity_error`][queuefull] (`queue_full`) when the result
  is `None` and `footprint <= ctx.resident_capacity()`. The other `None` arm
  is [`request_too_large_error`][toolarge] (`invalid_params`).
- [`try_reserve_resident`][reserve] is `self.scratch.try_charge(bytes)` and
  [`resident_capacity`][capacity] is `self.scratch.capacity()`. Its doc says
  requests above the ceiling are permanent rejection, not backpressure.
- [`ByteBudget::try_charge`][try-charge] is `try_acquire_many_owned` on a
  semaphore; `None` for a short pool is transient because a held
  `ByteCharge` releases its permits on drop.
- The scratch pool is one host-wide budget at
  [`scratch_budget: ByteBudget::new(SCRATCH_RESERVED_BYTES)`][pools], and
  [`SCRATCH_RESERVED_BYTES`][scratchconst] is sized from Synapse terms; it
  evaluates to 184,878,336 bytes with `MAX_BODY_LEN` at 64 MiB. The context
  handler, Synapse, and Broca all draw on it.
- The footprint of a body is [`value_footprint_bound`][footprint]; string
  bytes count three times and nodes count `2 * size_of::<Value>()`, so two
  bodies near the 32 MiB transform cap sum above the pool while each fits
  alone, and a scalar-dense body bounds far above its wire size.
- The [`ByteBudget` tests][t-budget] cover permanent versus transient `None`
  and all-or-none acquisition on the budget alone; the
  [pool split test][t-pools] covers the three non-overlapping pools. Neither
  drives `handle`.
- The plugin [pages at 512 KiB][paging], so production bodies on the unpaged
  lane are small; whether Synapse and context load together drain the pool in
  production is not verified here.

## Failure scenario

Not a violation; a coverage gap. Without the constructed shortfall, a
campaign never evaluates the branch that distinguishes `queue_full` from
`invalid_params`, never observes the charge release on refusal, and cannot
tell a design that awaits the reservation from one that fails fast.

## Timing windows and dependencies

The window is between the first request's `try_charge` and the drop of its
`_parse_charge` at the end of [`handle`][handle]. A second request must reach
its own `try_reserve_resident` inside that window with a footprint above the
remaining bytes. Because the charge is held through dispatch and settlement,
holding the first request in its handler is enough to keep the window open.

## What a test must construct

Two or more concurrent bodies whose footprints sum above
[`SCRATCH_RESERVED_BYTES`][scratchconst] while each is at or below it; a
barrier inside the first request's dispatch (or a slow settlement) that holds
its charge until the second reaches the reservation. The marker records, at
entry to [`try_reserve_resident`][reserve]: the footprint,
`resident_capacity()`, the pool's available bytes, and the identities of the
concurrent holders. It asserts `footprint <= capacity` and
`available < footprint`; it does not assert the returned code. The
[`ByteBudget` checks](../existing-checks.md#ingress-admission-and-decode)
are the only existing coverage and none is a handler-level witness.

## Investigation log

### Q: Is the shortfall reachable in default production?

- Sources examined: [`SCRATCH_RESERVED_BYTES`][scratchconst], the shared
  [pool][pools], the plugin [paging threshold][paging], the
  [footprint bound][footprint].
- Findings: The pool is shared by three components, so concurrent Synapse
  parses and a context transform can compete. No production trace or load
  figure was supplied, and the plugin's 512 KiB pages keep a single context
  body far below the pool.
- Missing evidence: A production observation of `queue_full` from this arm.
- Conclusion: unresolved, needs a production observation; the record stays
  `test-only` as the catalog states.

[handle]: ../../../../../crates/daemon/src/lib.rs#L11812-L11834
[footprint]: ../../../../../crates/daemon/src/lib.rs#L15434-L15462
[toolarge]: ../../../../../crates/daemon/src/lib.rs#L15465-L15470
[queuefull]: ../../../../../crates/daemon/src/lib.rs#L15472-L15477
[reserve]: ../../../../../crates/host-runtime/src/handler.rs#L474-L484
[capacity]: ../../../../../crates/host-runtime/src/handler.rs#L486-L491
[pools]: ../../../../../crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: ../../../../../crates/host-runtime/src/config.rs#L21-L31
[try-charge]: ../../../../../crates/host-runtime/src/wire.rs#L430-L442
[t-budget]: ../../../../../crates/host-runtime/src/wire.rs#L825-L865
[t-pools]: ../../../../../crates/host-runtime/src/config.rs#L480-L503
[paging]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
