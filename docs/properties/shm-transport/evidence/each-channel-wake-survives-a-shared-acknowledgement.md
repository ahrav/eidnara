# each-channel-wake-survives-a-shared-acknowledgement

## Discovery trigger

`reactor-callback-is-one-in-flight` notes that no test lands edges from several
channels in one pending window, but its guarantee constrains only the global
callback count. The per-channel progress claim had no owner.

## Evidence trail

- `packages/shm-native/src/lib.rs:1315-1341`: one walk over all registered
  channels computes a single `redispatch` boolean from every channel's
  `complete_data_wait` and `arm_data_wait` results.
- `packages/shm-native/index.ts:652-676`: `dispatchReadiness` runs every
  registered handler on each batch, so a redispatch triggered by one channel
  re-runs all handlers.
- `crates/shm-transport/src/backend/ring.rs:1179-1196`: `arm_data_wait` returns
  `Ok(false)` when data or a generation change is already visible, with the
  documented meaning "poll again instead of blocking".
- `packages/shm-native/tests/mechanism.ts:525-648`: the single-channel suite
  cited by `wake-published-during-readiness-callback-is-not-lost`.
- `packages/shm-native/tests/mechanism.ts:222-259`: two channels are watched
  through `startReadiness` (`:228`, `:232`), the first handler throws, the
  second channel is published to (`:246`), and delivery is asserted within one
  second; this is two-channel delivery under the shared acknowledgement, without
  the edge timed inside the first channel's pending window.

## Failure scenario

Channel B's edge lands while A's batch is pending. The shared epoll wake has
already been consumed; B's only route to a handler run is its re-arm returning
`Ok(false)`. If that walk is skipped, short-circuited, or reports per-channel
state that the dispatcher ignores, B stalls until its next unrelated edge.

## Timing windows and dependencies

The pending window of another channel's batch, from the reactor's `pending`
CAS to `handled()`.

## What a test must construct

Two registered channels; a publication to B timed between A's callback start
and acknowledgement; assert B's handler observes the frame in the next batch.

## Investigation log

### Q: Is the property emergent or structural today?

- Sources examined: the re-arm walk and the dispatcher.
- Findings: it holds because every handler runs on every redispatch and
  `Ok(false)` forces one; nothing per-channel is tracked.
- Missing evidence: a two-channel test.
- Conclusion: recorded as unexercised with the structural alternative as the
  open question.
