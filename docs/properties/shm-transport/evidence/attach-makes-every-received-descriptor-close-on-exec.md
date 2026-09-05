# attach-makes-every-received-descriptor-close-on-exec

## Discovery trigger

Round 18 review of the PR: `attach_sets_close_on_exec_on_every_descriptor` is a
claim-bearing check in the inventory with no owning record, and the gate it
covers is the only defence against an execed child inheriting the mapping and
the peer's doorbell ends.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1095-1101`: `Ring::attach` loops
  over the three received descriptors and calls `sys::set_cloexec` on each,
  mapping a failure to `RingError::ObjectValidationFailed`, before any other
  validation; the comment at `:1096-1098` records why (descriptors received
  without `MSG_CMSG_CLOEXEC` arrive inheritable).
- `crates/shm-transport/src/backend/sys.rs:233`: `set_cloexec` is the
  `fcntl(F_SETFD)` wrapper; `is_cloexec` and `clear_cloexec` sit beside it for
  the test.
- Callers: the addon's `attach_ring` (`packages/shm-native/src/lib.rs:287`) and
  the bridge's `attach_with_descriptors`
  (`crates/host-runtime/src/ring_transport.rs:877-879`); the addon also dups its
  descriptors with `CLOEXEC` (`lib.rs:270-284`) and opens the setup socket with
  `SOCK_CLOEXEC` (`packages/shm-native/src/setup.rs:159`).
- Test: `attach_sets_close_on_exec_on_every_descriptor` (`ring.rs:3474-3491`)
  clears the flag on a fresh attachment's descriptors, attaches, and asserts
  `is_cloexec` on each raw descriptor while the ring holds it.

## Failure scenario

The gate is removed. A client that execs a helper after attaching hands the
helper the mapping and both doorbell ends. When the client exits, the peer's
setup-socket sentinel and doorbell drain keep seeing live ends held by the
helper, so peer death is not observed until the helper exits, and the charges
for that connection stay pinned for the same time.

## Timing windows and dependencies

None on the gate itself; the impact window is the lifetime of the execed child.

## What a test must construct

Descriptors with the flag cleared, an attach, and a check of the flag on every
held descriptor: present. Missing: the `fcntl`-failure arm, and an end-to-end
fork-and-exec witness that the child holds no ring descriptor.

## Investigation log

### Q: Is any other caller responsible for the flag?

- Sources examined: both `Ring::attach` callers, the addon's descriptor
  duplication, the setup socket creation.
- Findings: the addon sets `CLOEXEC` on its own copies before attaching; the
  bridge relies on `Ring::attach` alone.
- Missing evidence: none.
- Conclusion: the transport gate is the one every path shares.
