# ring-a-publish-failure-is-reported-as-a-clean-peer-close

## Discovery trigger

Mapping the outbound failure path in `run_endpoint` for the frame-lifecycle map.
The inbound failure path sent an explicit `ReadClose` before returning; the
outbound failure path of that revision did not. Tracing what the connection
engine then observed produced the finding. At HEAD both paths go through
`fail` (`crates/host-runtime/src/ring_transport.rs:897-908`), which sends the
close first, so the trigger survives only in its second clause: the engine
still folds the explicit cause into `ReadExit::Peer`.

## Evidence trail

**The outbound failure path, in full.** Superseded. The path this trail was
built on returned from `run_endpoint` without sending on `inbound`. At HEAD
`run_endpoint:696-704` and `:884-892` read:

```
if publisher.pump(&rings.first).is_err() {
    fail(
        &mut inbound,
        &mut queue,
        &root,
        ReadClose::Corrupt("shared-memory publish failed"),
    );
    return;
}
```

and `fail` (`:897-908`) sends the close through the reserved terminal permit
(`Inbound::close`, `:621-623`) before cancelling `queue.retired` and `root`.
So the outbound and inbound failure paths are the same shape at HEAD: an explicit
`ReadClose` first, then the two cancels, then `return`.

**What the connection engine sees.** `run_endpoint` owns
`inbound: Option<Inbound>` (`:649`), a sender plus one reserved terminal
permit (`:615-618`). Superseded in part: returning without sending would drop
the sender and close the channel, and `ShmReceiver::recv` (`:627-633`) is:

```
self.inbound.recv().await.unwrap_or(Err(ReadClose::CleanEof))
```

So a closed channel becomes `Err(ReadClose::CleanEof)` at `:631`. That is the
only `CleanEof` producer in the crate, and at HEAD no publish-failure path
reaches it, because every one sends `Corrupt` first.
`connection.rs:364-366` maps it, together with `Corrupt`:

```
Err(ReadClose::CleanEof) | Err(ReadClose::Corrupt(_)) | Err(ReadClose::Overloaded) => {
    return ReadExit::Peer;
}
```

`ReadExit::Peer` takes the silent-retirement arm at `connection.rs:315-318`:
`gen.token.cancel()` then `gen.writer.discard()`. The comment at
`connection.rs:309-314` states the intent - "a client that sent a corrupt frame
never receives terminals or a Goodbye after the close decision (protocol §6.3)".
That is correct handling for a peer-caused close. It is applied here to a
host-caused one.

**Cause erasure inside `Publisher::try_publish`.** `try_publish` returns
`Result<bool, ()>` (`:1221`) and `pump` returns `Result<(), ()>` (`:1159`).
Four distinct causes collapse into that unit:

1. Publication deadline expiry - a pending frame whose `deadline`
   (`Publisher::push`, `:1152`) has passed fails the whole `pump` at
   `:1168-1171`, and the `sleep_until(publisher.earliest_deadline())` arms at
   `:867-877` and `:1021-1023` retire the generation with the same string
   without going through `pump` at all. No `ProducerError::Deadline` is
   involved: `try_reserve_in` never blocks, and `Exhausted` (`ring.rs:1823`)
   only leaves the frame pending (`:1242`).
2. Wire-header/length disagreement - `commit` rejects it at
   `ring.rs:1723-1725` with `ProducerError::WireHeaderMismatch`, mapped to `()`
   inside `commit_before` at `:1316`.
3. A panic in the direct serializer - caught by the inner `catch_unwind` at
   `:1259-1262` and turned into `Err(())` by the `!matches!(result, Ok(Ok(())))`
   test at `:1263-1265`.
4. `ReservationWriter` exhaustion - `:1324-1329` produces
   `io::ErrorKind::WriteZero`, which `publish_direct` maps to `()` at `:1290`.

Every other `try_reserve_in` error is also erased, at `:1243`. The erasure
happens before `run_endpoint` ever sees it.

**The asymmetry.** Superseded. The publish failure raised from inside the
ingress-budget wait returns the same cause it always did:

```
// ring_transport.rs:1000-1008
queued = queue.recv(), if publisher.can_accept() => match queued {
    Some(queued) => {
        publisher.push(queued);
        if publisher.pump(&rings.first).is_err() {
            return Err(ReadClose::Corrupt("shared-memory publish failed"));
        }
    }
    None => return Err(ReadClose::Cancelled),
},
```

That `Err` propagates out of `receive_one` into `run_endpoint`'s `Err(close)`
arm at `:738-741`, which calls `fail`. The outbound loop calls `fail` with the
identical string (`:696-704`, `:884-892`), so the cause does not depend on
which loop was driving the publication.

## Failure scenario

A peer attaches and then stops receiving. The host-to-peer ring fills to its
eight-descriptor depth. The next `Publisher::try_publish` call gets
`ProducerError::Exhausted` from `try_reserve_in` (`:1240-1242`) and leaves the
frame pending; `run_endpoint` arms `Ring::arm_capacity_wait` (`:787-802`) and
parks on the capacity readiness fd and on
`sleep_until(publisher.earliest_deadline())`. The peer never returns a block,
so the deadline arm fires (`:867-877`) and calls `fail` with
`ReadClose::Corrupt("shared-memory publish failed")`. The connection engine
reads `Corrupt`, classifies `ReadExit::Peer` (`connection.rs:364-366`), retires
silently, and discards the queued frames. The `CleanEof` reading this scenario
once described is superseded; the disposition is unchanged.

Consequences:

- Every pending correlation becomes `outcome_unknown` at the close, with no
  terminal and no recorded reason. `docs/host-wire-protocol.md:298` says
  "Once publication begins, a missing terminal leaves the request outcome
  unknown", which is satisfied, but the host has no record of *why*.
- Diagnostics records nothing. `peer_deaths` is incremented only from
  `connection.rs:185`, on an unexpected setup-socket close, which did not happen
  here. `exhaustions` is incremented only in `prepare` (`:224`). So
  `diagnostics()` shows `state: "healthy"` with all four counters unchanged
  (`ring_transport.rs:180-195`; post-#131 the `attachment` counter is removed,
  leaving four).
- An operator investigating sees a client disconnect. The host caused it.

The wire-header-mismatch cause is the sharper version: it means the host encoded
a frame whose header disagrees with its body, a host-side encoder defect, and it
is reported as the peer going away.

## Timing windows and dependencies

No interleaving is required; the misreport is the straight-line behaviour of the
`fail` path (`:696-704`, `:884-892`) followed by the fold at
`connection.rs:364-366`.

There is one ordering subtlety worth stating. `fail` sends the close and then
cancels `queue.retired` and `root` *before* `run_endpoint` returns, so the
`FrameSender` is retired and the generation token is cancelled by the time the
engine reads `Corrupt`. So the engine's `ReadExit::Peer` arm finds `gen.token`
already cancelled. That does not change the classification - the
`ReadExit::HostCancelled` arm at `connection.rs:363` is reached only by
`ReadClose::Cancelled`, and the cause here is `Corrupt` - but it does mean a
host that wanted to distinguish the two cases already has the signal available
in `gen.token`. (The superseded shape reached the same point through
`CleanEof`.)
already has the signal available in `gen.token`.

Dependencies: Part 2a's
`close-disposition-is-a-total-function-of-the-read-exit-cause` is the property
this one feeds. That record establishes that disposition follows cause
deterministically; this one establishes that the cause is wrong.

## What a test must construct

Cheapest construction, using the existing harness shape:

1. Use `RingFactory::connect` (`frame_channel/contract_tests.rs:498-521`) or an
   equivalent, which builds a real `RingTransport`, calls the production
   `prepare`, and attaches a real `RingClientEndpoint`.
2. Do not call `recv` on the peer. Publish host-to-peer frames until the ring's
   eight descriptor slots (profile-pinned; asserted at `ring_transport.rs:903`)
   are full.
3. Send one more frame with a short `write_deadline` (`ContractConfig` carries
   it, `:505`), so the pending frame's publication deadline
   (`Publisher::push`, `ring_transport.rs:1152`) expires quickly.
4. Assert the cause the receiver observes. At HEAD it is
   `ReadClose::Corrupt("shared-memory publish failed")`, which satisfies the
   first clause; the superseded shape produced `CleanEof` here.
5. Carry the scenario through the connection engine and assert the final
   disposition or operator-visible classification is distinct from a peer close.
   The intermediate enum is not enough: in the charge-wait path the receiver can
   observe `ReadClose::Corrupt` while `read_loop` (`connection.rs:362-365`) folds
   it into the same `ReadExit::Peer` arm as `CleanEof`, which is the predicted
   violation of the check's second clause.

A second, cleaner construction for the wire-header cause: admit an
`OutboundFrame` whose `bytes[0..4]` declares a length that disagrees with
`bytes.len() - HEADER_LEN + tail.len()`. `publish_owned` computes `body_len`
from the actual bytes (`:621-622`) and passes the header through untouched, so
`commit_reservation` rejects it at `ring.rs:1585-1593`. This needs a test-only
constructor for a malformed `OutboundFrame`, since the production encoders in
`wire.rs` presumably keep the two consistent.

No existing check covers either. `connection.rs:401-404` is the consuming match,
not a check.

## Investigation log

### Q: Should `Publisher::try_publish` carry a cause enum rather than `()`?

- Sources examined: `ring_transport.rs:1221-1277` (`try_publish`),
  `:1280-1318` (the two publish helpers, `commit_before`, and their
  `map_err(|_| ())` sites), `:1159-1217` (`pump`, including the unit deadline
  failure at `:1168-1171`), `frame_channel.rs:24-37` (the `ReadClose`
  taxonomy, four variants).
- Findings: the information exists at every failure site and is thrown away
  at `:1243`, `:1263-1265`, `:1290`, `:1302-1303`, and `:1316`. The receiving
  taxonomy already has the shape to carry it: `ReadClose::Corrupt(&'static
  str)` takes a static string, and every caller already passes exactly one
  such string, `"shared-memory publish failed"` (`:701`, `:889`, `:973`,
  `:1004`, `:1018`, `:1022`, `:874`). So the change is small: give `pump` a
  `Result<(), &'static str>` and have the callers forward the reason into
  `fail` and the direct returns.
- Missing evidence: none on the sending side. `fail` (`:897-908`) already
  sends through the reserved terminal permit, which cannot fail for lack of
  capacity, so the pattern the superseded shape lacked is the only one at HEAD.
- Conclusion: resolved with answer. The change is mechanical and the receiving
  side already handles it. Whether to make it is a fix decision, out of scope
  for this catalog.

### Q: Is the asymmetry between the two publishing loops for the identical fault deliberate?

- Sources examined: at the time, both sites and the polling-era comment that
  explained why the budget wait serviced outbound frames; at HEAD,
  `run_endpoint:696-704` and `:884-892`, `receive_one:1000-1008` and
  `:1021-1023`, and `fail` (`:897-908`).
- Findings: the superseded shape returned `Corrupt` from inside `receive_one`
  and `CleanEof` (by dropping the sender) from the outbound loop, and the two
  sites differed exactly where the enclosing function's return type differed,
  which read as an ergonomic default rather than a decision. At HEAD both
  loops report `ReadClose::Corrupt("shared-memory publish failed")`, and the
  outbound loop does so through `fail`, which sends before it cancels.
- Missing evidence: none.
- Conclusion: resolved by mechanism change. The asymmetry does not exist at
  HEAD; the record keeps only its second clause, the `ReadExit::Peer` fold at
  `connection.rs:364-366`.
