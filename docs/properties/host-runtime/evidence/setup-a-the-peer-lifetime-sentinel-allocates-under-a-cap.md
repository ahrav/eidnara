# setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap

## Discovery trigger

An earlier re-scope pass left this open, in a document cited as
`docs/properties/part-2-rescope/scope-map-and-risk-ranking.md:744-746`:
"`setup_socket.rs:355` is named `read_message_unbounded` and `:369` is named
`read_message`. Whether the first is a real missing bound or a bounded read
under a misleading name is unresolved; I did not read the bodies." This record
reads the bodies and resolves the length axis of that question. That document
is not present at HEAD, so the quotation is carried from the previous revision
of this file and cannot be re-verified; see the investigation log.

This record was split from
`setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap-and-stays-cancellable`.
The cancellation clause is now
`setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input`,
a liveness record with its own evidence file. This file keeps only the
allocation cap.

## Evidence trail

All references are at HEAD `e16e39e`. The catalog record cites line numbers
from an earlier revision, offset by nine lines in `setup_socket.rs` and one
line for the constant; the HEAD lines are what follow.

`crates/host-runtime/src/setup_socket.rs:346-358`:

```
346: async fn read_message_unbounded<T: DeserializeOwned>(
347:     stream: &mut UnixStream,
348: ) -> Result<T, SetupError> {
349:     let mut len = [0u8; 4];
350:     stream.read_exact(&mut len).await?;
351:     let len = u32::from_le_bytes(len) as usize;
352:     if len > MAX_SETUP_MESSAGE_LEN {
353:         return Err(SetupError::MessageTooLarge);
354:     }
355:     let mut body = vec![0u8; len];
356:     stream.read_exact(&mut body).await?;
357:     serde_json::from_slice(&body).map_err(|_| SetupError::InvalidMessage)
358: }
```

The cap at `:352-354` precedes the allocation at `:355`. `MAX_SETUP_MESSAGE_LEN`
is `16 * 1024` (`:25`), a `pub const` with no configuration behind it.

Compare `read_message` at `:360-377`, which is byte-for-byte the same except
that both `read_exact` calls are wrapped in `timeout_at(deadline, ...)` at
`:365-367` and `:373-375`. Its cap is at `:369-371`. The third reader,
`read_message_from_prefix` at `:379-407`, caps at `:392-394`, then also computes
`4usize.checked_add(len)` at `:395` and rejects a prefix longer than the total
at `:396-398`. All three cap before they allocate. The difference between them
is exclusively the deadline. **The name means time-unbounded, not
length-unbounded.**

Sole caller, `observe_peer` at `:336-344`:

```
336: pub async fn observe_peer(stream: &mut UnixStream) -> PeerClose {
337:     match read_message_unbounded(stream).await {
338:         Ok(ClientMessage::Goodbye) => PeerClose::Goodbye,
339:         Err(SetupError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
340:             PeerClose::UnexpectedEof
341:         }
342:         _ => PeerClose::ProtocolError,
343:     }
344: }
```

A total function over the read outcome into the three `PeerClose` variants
(`:82-86`). `MessageTooLarge` falls into the `_` arm, so an over-cap length
surfaces to the connection as `ProtocolError`, which `connection.rs:184-186`
treats as peer death.

Reachability. `observe_peer` runs inside the sentinel task spawned for every
activated connection at `connection.rs:179-191`, after `record_activation` at
`:172`. There is no configuration on the path; the property is
`default-production`.

Existing tests. `goodbye_and_eof_have_distinct_outcomes` (`:806-820`) covers
the `Goodbye` and `UnexpectedEof` classifications through `observe_peer`.
`activation_and_commit_complete_on_setup_socket` (`:594-645`) covers the
`ProtocolError` classification by sending a `request` after commit and
asserting the result at `:641-644`. Neither sends an over-cap length, so the
cap itself is untested.

## Failure scenario

If `:352-354` were deleted, moved below `:355`, or the comparison inverted, a
post-commit peer sends a length prefix of `0xFFFFFFFF` and the host allocates
4 GiB at `:355`. The allocation happens before any parse, so no JSON validity is
required, and before the second `read_exact`, so the peer need send nothing
after the four bytes. That peer is authenticated, so this is a
post-authentication denial rather than a pre-authentication one, but it costs
one `vec![0u8; len]` per connection and `max_connections` defaults to 64
(`config.rs:77`).

The read has no deadline, so the allocation, once made, is held until the peer
sends the body, closes, or the generation is cancelled. That makes the cap the
only thing between a post-commit peer and a long-lived 4 GiB allocation.

## Timing windows and dependencies

None for the cap. The check and the allocation are straight-line code four
lines apart with no await between them (`:351-355`). The property is that this
ordering does not regress.

Depends on `MAX_SETUP_MESSAGE_LEN` staying a compile-time constant. If it became
configurable, the record's reachability class would need re-verifying.

## What a test must construct

At unit level with a `UnixStream::pair`, the shape `:806-820` already uses:

1. write a 4-byte little-endian prefix of `u32::MAX` and nothing else;
2. call `observe_peer` and assert `PeerClose::ProtocolError`;
3. assert no allocation of that size occurred. A resident-set delta is the
   practical oracle; `crates/host-runtime/tests/support/process_resources.rs`
   provides `ResourceCounts` (`:18`) and is already used by
   `shm_failure_modes.rs` and `shm_soak.rs`.

Step 3 is what makes the test non-vacuous. Asserting only `ProtocolError`
passes on an implementation that allocated 4 GiB and then failed to fill it.

A boundary pair is worth pinning at the same time: a prefix of exactly
`MAX_SETUP_MESSAGE_LEN` followed by that many bytes of valid `Goodbye`-padded
JSON must be accepted, and `MAX_SETUP_MESSAGE_LEN + 1` must classify as
`ProtocolError`. That converts the cap from "some limit exists" into a pinned
edge. `receive_grant` (`:179`) already sizes its receive buffer to
`MAX_SETUP_MESSAGE_LEN + 4` at `:183`, so the same edge is load-bearing on the
grant path too.

## Investigation log

### Q: Is `read_message_unbounded` length-unbounded, as its name suggests?

- Sources examined: `setup_socket.rs:346-358` against `:360-377`, and
  `:379-407` for a third comparison.
- Findings: no. All three cap at `MAX_SETUP_MESSAGE_LEN` before allocating:
  `:352-354`, `:369-371`, `:392-394`. `read_message_from_prefix` additionally
  guards the prefix-versus-total relationship at `:395-398`. The only
  difference between the three is the deadline treatment.
- Missing evidence: none.
- Conclusion: resolved with answer. Length is bounded in all three, and only
  `read_message_unbounded` lacks a deadline. The name is the hazard, and it is
  the record's open question.

### Q: Can the discovery-trigger quotation be re-verified?

- Sources examined: `docs/properties/` at HEAD, `git log --all` for
  `docs/properties/part-2-rescope`, and the previous revision of this evidence
  file, which cited commit `e447c927`.
- Findings: the directory does not exist at HEAD and has no history in this
  repository; `e447c927` is not a valid object here. Six other evidence files
  in this directory cite the same document. The quotation is retained because
  it explains why the record exists, but it cannot be checked against a file.
- Missing evidence: the re-scope document or the commit that contained it.
- Conclusion: unresolved, needs the re-scope document restored or the
  reference dropped across the affected evidence files.

### Q: Does `observe_peer` classify every outcome?

- Sources examined: `setup_socket.rs:336-344`, `:89-99` (`SetupError`
  variants), `:82-86` (`PeerClose`).
- Findings: total. `SetupError` has nine variants; `Goodbye` maps from one `Ok`
  shape, `UnexpectedEof` from one specific `Io` kind, and the `_` arm absorbs
  the remaining eight variants plus every other `Ok` variant.
  `ClientMessage::Activate` or `Commit` arriving post-commit therefore yields
  `ProtocolError`, which `:594-645` asserts at `:641-644`. `MessageTooLarge`
  takes the same arm.
- Missing evidence: none.
- Conclusion: resolved with answer. Totality holds and is partially tested.
