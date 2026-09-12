# scratch-pool-shortfall-reaches-the-parse-reservation

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

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


## Shortfall-witness evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/436)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The reservation is no longer one call before the parse; the
[`ResidentMeter`][meter] charges the scratch reserve as the decode visits
values. The shortfall path is the meter's transient refusal: the footprint the
decode has reached fits the capacity, the pool does not have the bytes, so
another holder has them. At that point the meter [records][need] its
[`ShortfallMarker`][marker] with the footprint reached, the bytes already held,
and the capacity, readable through [`shortfall`][shortfall]; under
`test-support` a process-wide [count][count] of transient refusals grows too.
The marker records the preconditions (`needed <= capacity`,
`charged < needed`), not the handler's outcome.

The [drained-pool test][t-drain] constructs the situation with a real byte
budget: another charge holds half the pool, a body that fits the capacity is
decoded, the meter refuses it as transient and releases its bytes, the count
rises, the meter's marker satisfies both preconditions, `resident_refusal`
maps it to `queue_full`, and once the holder releases the same body decodes. It
also shows a transient refusal of a body the pool could never hold classed as
too large. The [effect test][t-effect] drives the same transient refusal
through `dispatch_body` and shows no dispatch-side effect. A shortfall through
the ring is not constructed: the fixture's scratch pool is the fixed
`SCRATCH_RESERVED_BYTES`, and holding a first request's charge while a second
arrives needs a barrier inside the handler that the fixture does not expose.

### Focused execution, 2026-09-12

`cargo test -p daemon --locked` passed 1024 tests including the two above.

[meter]: ../../../../../crates/daemon/src/metered_decode.rs#L134-L142
[need]: ../../../../../crates/daemon/src/metered_decode.rs#L227-L287
[marker]: ../../../../../crates/daemon/src/metered_decode.rs#L113-L120
[shortfall]: ../../../../../crates/daemon/src/metered_decode.rs#L185-L190
[count]: ../../../../../crates/daemon/src/metered_decode.rs#L127-L129
[t-drain]: ../../../../../crates/daemon/src/lib.rs#L20146-L20194
[t-effect]: ../../../../../crates/daemon/src/lib.rs#L20256-L20304

[handle]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L11805-L11827
[footprint]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15427-L15455
[toolarge]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15458-L15463
[queuefull]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15465-L15470
[reserve]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L474-L484
[capacity]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L486-L491
[pools]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/config.rs#L21-L31
[try-charge]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/wire.rs#L430-L442
[t-budget]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/wire.rs#L825-L865
[t-pools]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/config.rs#L480-L503
[paging]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
