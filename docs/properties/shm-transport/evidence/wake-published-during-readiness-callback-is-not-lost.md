# wake-published-during-readiness-callback-is-not-lost

## Discovery trigger

Three fix commits inside PR #131's branch: `ee73e7034` "preserve data wakes
published during readiness callbacks", `72284f04d` "redispatch readiness after
callback-side publication", `a51f019cf` "allow one readiness notification
while the prior callback returns". Leads only; mechanism re-verified at HEAD.

## Evidence trail

- The reactor allows one in-flight callback: after a successful dispatch the
  `shm-readiness` thread blocks in `wait_until_handled`
  (`packages/shm-native/src/scheduling.rs:85-101`) until JS acknowledges.
  During that window an epoll edge on a channel doorbell is not observed.
- The acknowledgement itself is the recovery point. `readiness_handled`
  (`packages/shm-native/src/lib.rs:1352-1391`) walks every registered
  channel, calls `complete_data_wait` (drains the coalesced token,
  `ring.rs:1237-1242`) then `arm_data_wait` (`ring.rs:1187-1220`). `arm_data_wait`
  returns `Ok(false)` when data or a generation change is already visible —
  exactly the state a publication during the callback leaves behind — and
  `readiness_handled` converts `Ok(false)` or `Err` into `redispatch = true`
  (`lib.rs:1378-1381`).
- The JS side re-enters on that value: `dispatchReadiness`
  (`packages/shm-native/index.ts:699-729`) runs
  `if (loaded?.readinessHandled()) queueMicrotask(dispatchReadiness)` in a
  `finally`, so a true return is a guaranteed next dispatch even when a
  handler threw. The raw-addon test drives the same contract manually
  (`mechanism.ts:592` `if (addon.readinessHandled()) queueMicrotask(onReady)`).
- A kick raised while the callback is pending is also preserved:
  `wait_until_handled` returning true with `kick` still set rewrites the
  control eventfd (`scheduling.rs:229-231`), so the reactor loop sees a
  control edge on its next `epoll::wait` instead of dropping the kick.
- Publisher side: `commit_reservation` signals the data doorbell through
  `signal_wake` (`ring.rs:2376`), which bumps the shared generation
  unconditionally (`:2032`) and writes the eventfd only when a parked epoch
  existed (`:2033-2035`). The generation bump is what `arm_data_wait`'s
  recheck observes even when no eventfd byte was written.
  At HEAD: The doorbell is an AF_UNIX socketpair (`:710-720`), so this writes a one-byte token through `Doorbell::signal` (`:783-798`) rather than incrementing an eventfd.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

Peer publishes frame N+1 while the JS callback for frame N is running. The
doorbell token for N+1 is either coalesced into the token the callback is
about to drain, or never written because no epoch was parked. Without the
re-arm-and-redispatch contract, the consumer parks again and sleeps until an
unrelated event: a delivered frame sits invisible with no error and no
backpressure signal, indistinguishable from an idle channel.

## Timing windows and dependencies

The window is the whole callback execution: from the reactor's `pending` CAS
(`scheduling.rs:214-218`) to `handled()` (`:353-356`). Bounded recovery: one
`readiness_handled` call. The property depends on every acknowledger honoring
the boolean; a caller that ignores a true return reintroduces the lost wake
(the raw addon API makes this the caller's obligation; `index.ts` honors it).

## What a test must construct

A publication strictly inside a callback, then an assertion that a second
callback delivers it with no further publication. Exists:
`readiness acknowledgement preserves a frame published during callback`
(`packages/shm-native/tests/mechanism.ts:527-650`) publishes frame 2 from
callback 1 and requires `received == [1, 2]` and `callbacks == 2`. Not yet
constructed: the same race through the `NativeChannel.startReadiness` wrapper
with multiple registered channels, and a kick raised by `poll`'s empty-path
re-arm (`lib.rs:1463-1469`) landing during a pending callback.

## Investigation log

### Q: can a saturated eventfd counter drop the wake?

- Sources examined: `Doorbell::signal` (`ring.rs:783-798`).
- Findings: `EAGAIN` on write means the counter is at its maximum, which
  already reads as `POLLIN`; treating it as success loses nothing.
- Missing evidence: none.
- Conclusion: resolved with answer — no.
  At HEAD: The doorbell is a socketpair, so the saturation case is a full socket buffer: `send_token` returning `WouldBlock` means unread wake tokens are already queued, which is the same readable outcome, and it is treated as success at `:793`.

### Q: is the generation bump alone sufficient when no epoch is parked?

- Sources examined: `signal_wake` (`ring.rs:2026-2037`), `arm_data_wait`
  rechecks (`:1205-1218`).
- Findings: an unparked consumer is by definition about to run
  `data_available` or `arm_data_wait`, both of which observe the published
  cursor or the changed generation before blocking.
- Missing evidence: none.
- Conclusion: resolved with answer — yes, for consumers using the arm
  protocol; a consumer blocking on the raw fd without arming would race, and
  none exists at HEAD.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 23, `lib.rs:1197-1200` now `lib.rs:1378-1381`: Only one case sets `redispatch` now: `(true, Ok(false))` with a lease that advanced (`:1378-1381`). A plain `Ok(false)` is ignored (`:1384`) because `poll` arms the channel once the ring is empty, and an `Err` or a failed `complete_data_wait` unregisters the channel (`:1385`).
  - line 37, `:1475-1477` now `:2081-2083`: The doorbell is an AF_UNIX socketpair (`:752-762`), so this writes a one-byte token through `Doorbell::signal` (`:825-840`) rather than incrementing an eventfd.
  - line 72, `ring.rs:416-428` now `ring.rs:783-798`: The doorbell is a socketpair, so the saturation case is a full socket buffer: `send_token` returning `WouldBlock` means unread wake tokens are already queued, which is the same readable outcome, and it is treated as success at `:793`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
