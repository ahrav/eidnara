# attach-validates-doorbell-sockets

Supersedes `attach-validates-doorbell-eventfds` (2026-09-05). The eventfd gate that
record described (`O_NONBLOCK` in `F_GETFL` plus a `/proc/self/fd` readlink to
`anon_inode:[eventfd]`) does not exist in this tree; the retired trail resolved
against the source tree and is not carried forward.

## Discovery trigger

`Doorbell` (`crates/shm-transport/src/backend/ring.rs:714-720`) is a
`socketpair` end, and `doorbell_attachment_requires_connected_unix_stream_socket`
(`:3096-3134`) rejects an eventfd outright, so the predecessor record specified the
opposite acceptance condition from the code.

## Evidence trail

- `Doorbell::create` (`:727-738`) calls `socketpair(AF_UNIX, SOCK_STREAM |
  SOCK_CLOEXEC | SOCK_NONBLOCK, 0)` and keeps both ends; `take_peer_end`
  (`:778`) moves the remote end out exactly once, and `attachment`
  (`:1247-1261`) does so for both doorbells beside the mapping duplicate.
- `Doorbell::from_fd` (`:744-756`) is the attach-side gate: `socket_option`
  (`:699` (source tree; not at HEAD)) must return `AF_UNIX` for `SO_DOMAIN` and `SOCK_STREAM` for `SO_TYPE`,
  and `getpeername` must succeed. Any failure is `RingError::DoorbellFailed`.
- `Ring::attach` (`:1095`) routes both transferred doorbell descriptors through
  `Doorbell::from_fd` (`:1127-1128`) after mapping validation, before returning a
  usable ring.
- `signal` (`:783-798`) sends one byte with `MSG_DONTWAIT | MSG_NOSIGNAL`,
  treating `EAGAIN` as delivered; `drain` (`:801-816`) receives up to
  `DRAIN_BYTES` (`:724`) with `MSG_DONTWAIT`, treating `EAGAIN` as empty and a
  zero-length read (`:806`) as a closed peer; `wait_until` (`:818-841`) polls the
  local end with a zero-timeout probe and then a bounded poll.
- Per-call `MSG_DONTWAIT` plus one open file description per end is why the
  gate does not inspect `O_NONBLOCK`:
  `doorbell_never_blocks_after_either_end_clears_nonblock` (`:3137-3160`) clears the
  flag on both ends and still completes a bounded wait and a million signals.
- `closed_peer_doorbell_fails_instead_of_blocking` (`:3163-3169`) drops the
  creating side and asserts `signal` and `drain` both return `DoorbellFailed`.
  At HEAD: The gate requires SO_TYPE == SOCK_STREAM (`:745-748`) and a successful peer_addr() (`:750`), which is what proves the descriptor is a connected AF_UNIX end; no SO_DOMAIN query exists.
  At HEAD: Doorbell::create builds the pair with UnixStream::pair() and then sets O_NONBLOCK on both ends, so no raw socketpair call with SOCK_CLOEXEC or SOCK_NONBLOCK remains in ring.rs.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A peer transfers a regular file, an eventfd, or an unconnected socket in a
doorbell slot. Without the gate, the first `send` or `recv` fails with
`ENOTSOCK` or `ENOTCONN` and the ring reports `DoorbellFailed` on first use
rather than at attach; a validated mapping may already exist by then.

## Timing windows and dependencies

None at the gate. The gate is a pure predicate over the descriptor.

## What a test must construct

- Present: the three rejection arms and the positive arm at unit level.
- Missing: a full `Ring::attach` with one substituted doorbell slot, asserting
  `DoorbellFailed` and no mapping side effects. That is the attach-ordering
  half of the record and has no test.

## Investigation log

### Q: Does a peer that clears O_NONBLOCK on its end reintroduce the unbounded block the old record feared? (2026-09-05)

- Checked: `Doorbell` holds its own end of the socketpair, a separate open file
  description from the peer's. Every `send` and `recv` passes `MSG_DONTWAIT`.
  `doorbell_never_blocks_after_either_end_clears_nonblock` clears the flag on both
  ends and asserts completion inside two seconds.
- Conclusion: no. The predecessor record's open question is closed by design
  and by test.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 17, `:527-549` now `:727-738`: Doorbell::create builds the pair with UnixStream::pair() and then sets O_NONBLOCK on both ends, so no raw socketpair call with SOCK_CLOEXEC or SOCK_NONBLOCK remains in ring.rs.
  - line 22, `:699`: The gate requires SO_TYPE == SOCK_STREAM (`:745-748`) and a successful peer_addr() (`:750`), which is what proves the descriptor is a connected AF_UNIX end; no SO_DOMAIN query exists.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 22, `:699` (socket_option): No generic getsockopt helper remains in ring.rs; from_fd calls sys::socket_type (`crates/shm-transport/src/backend/sys.rs:247-266`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
