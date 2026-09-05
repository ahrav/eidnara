# setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input

## Discovery trigger

An earlier re-scope pass left open whether `read_message_unbounded` was a real
missing bound or a bounded read under a misleading name, in a document cited as
`docs/properties/part-2-rescope/scope-map-and-risk-ranking.md:744-746`. Reading
the body showed it is length-capped and time-unbounded, by design. The time
axis is this record: since the read has no deadline, its only exit from a parked
state is cancellation, and that exit needs a liveness record with an explicit
bound. The re-scope document is not present at HEAD; the citation is carried
from the previous revision and cannot be re-verified.

This record was split from
`setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap-and-stays-cancellable`,
where the cancellation clause was an unbounded "always yields to `read_cancel`"
attached to a safety record. The allocation cap is now
`setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap`. An earlier revision
rejected writing any liveness record for this part on the grounds that the
available bounds were wall-clock durations; the catalog record explains why
that misread METHOD.md, and this file states the bound in the unit the code
actually bounds.

## Evidence trail

All references are at HEAD `e16e39e`. The catalog record cites line numbers
from an earlier revision, offset by nine lines in `setup_socket.rs` and by
sixteen in `connection.rs`; the HEAD lines are what follow.

The read with no deadline, `crates/host-runtime/src/setup_socket.rs:346-358`:
`read_exact` on the four-byte prefix at `:350`, then `read_exact` on the body at
`:356`, neither wrapped in `timeout_at`. Compare `read_message` at `:360-377`,
which wraps both at `:365-367` and `:373-375`. A peer that writes three of the
four prefix bytes and stops parks the read at `:350` indefinitely.

Sole caller, `observe_peer` at `:336-344`, awaits it directly and maps the
result into `PeerClose` (`:82-86`).

The bound, `crates/host-runtime/src/connection.rs:179-191`:

```
179:    shared.spawn_tracked(generation.read_tasks.track_future(async move {
180:        tokio::select! {
181:            biased;
182:            () = peer_read_cancel.cancelled() => {}
183:            close = crate::setup_socket::observe_peer(&mut stream) => {
184:                if close != crate::setup_socket::PeerClose::Goodbye {
185:                    peer_ring.record_peer_death();
186:                }
187:                peer_gen.token.cancel();
188:                peer_gen.read_cancel.cancel();
189:            }
190:        }
191:    }));
```

`biased` at `:181` with `peer_read_cancel.cancelled()` as the **first** arm at
`:182`. Under `biased`, arms are polled in source order, so once the token is
cancelled the first arm is ready on the next poll and the `observe_peer` future
is dropped where it stands, mid-`read_exact`. No byte from the peer is needed.
That is the bound: one cancellation edge plus one poll of the select. It is not
a duration.

`peer_read_cancel` is a clone of the generation's `read_cancel` (`:177`), the
token destructured from the prepared connection at `:145` and passed to
`new_generation` at `:175`. The peer-driven arm cancels it itself at `:188`, so
both exits converge on the same token.

The task is registered in `generation.read_tasks` via `track_future` (`:179`),
which is what makes the exit observable: once the sentinel completes, the
tracked set for the generation loses this entry.

Reachability. The sentinel is spawned for every activated connection, after
`record_activation` at `:172`, with no configuration on the path. The property
is `default-production`.

The behaviour is contractually intended. `docs/shm-transport.md:45` lists
"Keep the setup socket open as the peer-lifetime sentinel" as the final setup
step, so a deadline on this read would manufacture a false peer death on any
idle connection. The correct bound for an intentionally idle read is
cancellation, and the `biased` select is what provides it.

Existing tests. `goodbye_and_eof_have_distinct_outcomes`
(`setup_socket.rs:806-820`)
reaches `observe_peer` through a written `Goodbye` and through a dropped peer
end. `activation_and_commit_complete_on_setup_socket` (`:594-645`) reaches it
through a post-commit `request`. All three outcomes arrive through the peer.
None parks the read and cancels; none exercises the `connection.rs:182` arm.

## Failure scenario

If the `biased` keyword at `:181` were removed, the arm order changed, or
`observe_peer` awaited directly instead of inside the select, a peer that sends
a partial length prefix and stops parks `read_exact` at `setup_socket.rs:350`
with no exit but the peer's own. The task stays in `read_tasks`, so shutdown
joins a future that never completes on its own. Part 2a's
`read-task-quiescence-implies-no-further-registration` and
`draining-rendezvous-is-released-or-the-loss-is-declared` are the neighbouring
obligations, and this is exactly the input that would violate them.

Without `biased`, the failure is probabilistic rather than certain: an
unbiased select picks randomly among ready arms, and the `observe_peer` arm is
never ready while parked, so cancellation would still usually win. The
guarantee here is that it wins deterministically on the first poll, which is
what lets the bound be stated as one poll rather than "eventually".

The partial-prefix case is the sharp one. It is not an error and not an EOF; it
is a peer holding the read open with three bytes of legitimate-looking data.
There is no deadline to end it, by design.

## Timing windows and dependencies

Unbounded in duration by construction. The sentinel sits on the socket for the
whole life of an activated connection. Its exit bound is one cancellation edge
followed by one poll, and the test bound must be an explicit attempt count on
polling the tracked task set, not a timeout.

Depends on `read_cancel` being fired on every teardown path, which is Part 2a's
territory: `close-disposition-is-a-total-function-of-the-read-exit-cause` and
`no-task-outlives-the-generation-it-serves`.

Depends on the sentinel being the only reader of `stream` after commit. It is:
`stream` is the `mut stream: UnixStream` parameter at `connection.rs:88`, lent
to `activate_server` at `:156`, and then moved into the spawned block at
`:179`, where `:183` is the only remaining use.

## What a test must construct

1. Complete a full setup over a `UnixStream::pair` so the sentinel task is
   running and tracked in the generation's `read_tasks`.
2. Park it: write three bytes of a length prefix from the peer end and stop.
   Confirm the sentinel has not completed.
3. Cancel `read_cancel`. **Send nothing further** from the peer.
4. Poll the generation's tracked task set for emptiness, with an explicit
   attempt count stated in the test, yielding between attempts. Assert it
   emptied. The attempt count is the test's bound; a generous timeout would
   not distinguish one poll from a thousand.
5. Assert the negative side: `record_peer_death` was **not** called, because the
   exit was the cancellation arm at `:182`, not the peer arm at `:183-189`.
   That separates "exited on cancellation" from "exited because the peer
   arm also happened to fire".

A unit-level variant is possible without the connection machinery: wrap
`observe_peer` in the same `biased` select against a fresh
`CancellationToken`, park it, cancel, and assert the select resolves to the
cancel arm within one `tokio::task::yield_now`.

## Investigation log

### Q: Is the missing deadline correct?

- Sources examined: `connection.rs:179-191`, `setup_socket.rs:336-344`,
  `docs/shm-transport.md:45`, `:47`, `:49`.
- Findings: yes. The doc says the setup socket is kept open as the
  peer-lifetime sentinel (`:45`), so a deadline would manufacture a false peer
  death on any idle connection, and `:49` requires unexpected closure to be
  distinguished from clean `Goodbye`. The correct bound for an intentionally
  idle read is cancellation, and that is what the `biased` select provides.
- Missing evidence: none.
- Conclusion: resolved with answer. The behaviour is right; the name
  `read_message_unbounded` invites the wrong conclusion and already caused
  one. That is the sibling safety record's open question.

### Q: Does `biased` alone guarantee the first-poll exit?

- Sources examined: `connection.rs:180-182`; Tokio's documented `select!`
  semantics for `biased;`, which poll arms in declaration order and stop at the
  first ready one.
- Findings: with `peer_read_cancel.cancelled()` first, a cancelled token makes
  that arm ready, so the very next poll of the task takes it. The poll itself
  is scheduled by the runtime; the bound is therefore "one poll after
  cancellation", and a test must poll the tracked set rather than assume the
  poll has already happened.
- Missing evidence: none at the source level. The claim about Tokio's
  `biased` semantics is from its public documentation, not re-derived from
  Tokio's source here.
- Conclusion: resolved with answer.

### Q: Can the discovery-trigger citation be re-verified?

- Sources examined: `docs/properties/` at HEAD and `git log --all` for
  `docs/properties/part-2-rescope`.
- Findings: the directory does not exist at HEAD and has no history in this
  repository. The previous revision of this evidence file pinned its references
  to commit `e447c927`, which is not a valid object here. The citation is
  retained because it explains the record's origin; it cannot be checked.
- Missing evidence: the re-scope document or the commit that contained it.
- Conclusion: unresolved, needs the re-scope document restored or the
  reference dropped across the affected evidence files.
