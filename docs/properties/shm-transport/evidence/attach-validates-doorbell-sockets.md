# attach-validates-doorbell-sockets

Supersedes `attach-validates-doorbell-eventfds` (2026-09-05). The eventfd gate that
record described (`O_NONBLOCK` in `F_GETFL` plus a `/proc/self/fd` readlink to
`anon_inode:[eventfd]`) does not exist in this tree; the retired trail resolved
against the source tree and is not carried forward.

## Discovery trigger

`Doorbell` (`crates/shm-transport/src/backend/ring.rs:510-520`) is a
`socketpair` end, and `doorbell_attachment_requires_connected_unix_stream_socket`
(`:3021-3059`) rejects an eventfd outright, so the predecessor record specified the
opposite acceptance condition from the code.

## Evidence trail

- `Doorbell::create` (`:527-549`) calls `socketpair(AF_UNIX, SOCK_STREAM |
  SOCK_CLOEXEC | SOCK_NONBLOCK, 0)` and keeps both ends; `take_peer_end`
  (`:591`) moves the remote end out exactly once, and `attachment`
  (`:1127-1143`) does so for both doorbells beside the mapping duplicate.
- `Doorbell::from_fd` (`:552-570`) is the attach-side gate: `socket_option`
  (`:699`) must return `AF_UNIX` for `SO_DOMAIN` and `SOCK_STREAM` for `SO_TYPE`,
  and `getpeername` must succeed. Any failure is `RingError::DoorbellFailed`.
- `Ring::attach` (`:969`) routes both transferred doorbell descriptors through
  `Doorbell::from_fd` (`:1006`) after mapping validation, before returning a
  usable ring.
- `signal` (`:596-619`) sends one byte with `MSG_DONTWAIT | MSG_NOSIGNAL`,
  treating `EAGAIN` as delivered; `drain` (`:622-648`) receives up to
  `DRAIN_BYTES` (`:524`) with `MSG_DONTWAIT`, treating `EAGAIN` as empty and a
  zero-length read (`:638`) as a closed peer; `wait_until` (`:650-683`) polls the
  local end with a zero-timeout probe and then a bounded poll.
- Per-call `MSG_DONTWAIT` plus one open file description per end is why the
  gate does not inspect `O_NONBLOCK`:
  `doorbell_never_blocks_after_either_end_clears_nonblock` (`:3062-3085`) clears the
  flag on both ends and still completes a bounded wait and a million signals.
- `closed_peer_doorbell_fails_instead_of_blocking` (`:3088-3094`) drops the
  creating side and asserts `signal` and `drain` both return `DoorbellFailed`.

## Failure scenario

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
