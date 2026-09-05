# release-failure-is-observable

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

## Discovery trigger

Two `let _ =` sites on completion paths. `ReceiveLease::Drop` discards whatever
`release_once` returns, and the host's clean-close branch discards whatever
`custody.release()` returns. Both are on paths that only run when everything else
looked fine, which is exactly where a lost signal is least likely to be noticed by
anything else.

## Evidence trail

- `crates/shm-transport/src/lease.rs:366-372` — the drop-path discard:
  ```rust
  impl Drop for ReceiveLease<'_> {
      fn drop(&mut self) {
          if !self.released {
              let _ = self.release_once();
          }
      }
  }
  ```
  `release_once` (`:350-357`, span re-verified at post-#131 HEAD) calls through
  to `Ring::release`, so every error that function can produce — `Quarantined`
  (`ring.rs:1530`), `WrongIncarnation` (`:1538`), `WrongLane` (`:1541`),
  `InvalidSequence` (`:1545`, `:1557`, `:1573`, `:1586`), `DuplicateRelease` (`:1584`) —
  is silently dropped here.
- former `crates/host-runtime/src/shm_provider.rs:363-371` — the clean-close branch:
  `if clean && !quarantine_next_close.swap(false, Ordering::AcqRel) { let _ = custody.release(); } else { recovery.report_suspect(custody); }`. The suspect path
  is the `else`, so on a clean close no recovery record is created regardless of what
  `release()` reported.
- **Correction to the catalog record.** The catalog describes former `:365` as discarding "a
  clean-path charge-release failure" whose reachability "depends on `AdmissionError`".
  Verified against the code, that is the wrong mechanism. `custody` is a
  `CandidateCustody` (former `crates/host-runtime/src/provider_recovery.rs:141`) and
  `CandidateCustody::release(&self) -> bool` (former `:167-179`) returns a **`bool`**, not a
  `Result`: `true` when the record was `Active` and the charges were returned,
  `false` when the state was already `Released` or `Quarantined`, in which case the
  previous state is restored and aggregate counters are untouched (former `:174-177`).
  `AdmissionError` does not appear on this path at all — it is produced by
  `quarantine`, not `release` (`crates/shm-transport/src/profile.rs:561-565`,
  `:508-532`). So the discarded signal is real, but it is "this record was not in a
  releasable state", not an error value.
- `crates/shm-transport/src/profile.rs:553-556` — `Admission::release(mut self)`
  returns `()`. There is no fallible surface between custody and the controller.
- `profile.rs:498-506` — the controller's `release`, and two further silent
  discards beneath the two above: `let Ok(mut accounting) = self.accounting.lock() else { return; }` (`:499-501`) drops the charges on a poisoned mutex, and
  `if let Some(active) = accounting.active.checked_sub(charges)` (`:502`) has no
  `else`, so a charge set larger than `active` leaves the counters unchanged with no
  report. Both are relevant to `charge-release-never-silently-strands` as well.
- **Where release failure *is* observable.** The host's explicit receive-path
  releases propagate: `ring_transport.rs:685-687` and `:734-736` both use
  `lease.release().map_err(|_| ReadClose::Corrupt("shared-memory completion failed"))?`,
  and `ReadClose::Corrupt` ends the generation through the uniform error path
  (`:545-548`; the former unclean classification and `report_suspect` routing were
  deleted with `shm_provider.rs`). On the TypeScript surface the addon path also reports: a failed
  `Ring::release` inside `detach_active` becomes
  `error("receive completion failed")` (`packages/shm-native/src/lib.rs:350-354`),
  which throws through `packages/shm-native/index.ts:608-618` into either
  `shm-frame-channel.ts:227-245` (source tree; not at HEAD), where `close()` reports
  `onClosed("quarantined", error)` and rethrows, or
  `shm-frame-channel.ts:411-416` (source tree; not at HEAD), where the doorbell-driven drain path reports
  `onClosed("protocol_violation", error)` (the pre-#131 poll loop was replaced by
  the eventfd reactor drain). So the gap is specifically the Rust
  drop path and the host's clean-close bool, not the transport as a whole.

## Failure scenario

The drop path is reachable in the shipped host topology without any injected fault:

1. `receive_one` acquires a lease at `ring_transport.rs:674-680`. The lease is alive
   and the slot is `RECEIVER_LEASED`.
2. The ingress budget is saturated, so control enters the wait loop at `:702-730`.
3. Either `read_cancel.is_cancelled()` is true and the function returns
   `Err(ReadClose::Cancelled)` (`:712`), or the frame deadline elapses and it
   returns `Err(ReadClose::Overloaded)` (`:715-720`). In both cases `lease` is still
   in scope and is dropped on the way out.
4. `Drop` calls `release_once`, which calls `Ring::release`. If the ring was
   quarantined in the meantime — by the peer, or by a validation failure on the other
   direction — the call returns `LeaseError::Quarantined`, discarded at
   `lease.rs:369`.
5. `run_endpoint` classifies both `Cancelled` and `Overloaded` as **clean** (former
   `shm_provider.rs:498`), so the thread takes the `custody.release()` branch at
   former `:365`. Both were deleted by `ed487e11`; the surviving host path is the
   unconditional `admission.release()` at
   `crates/host-runtime/src/ring_transport.rs:360`.
6. If the custody record was already moved out of `Active` — for example by a suspect
   report on another path — `release()` returns `false` and the charges are not
   returned. That `false` is discarded.
7. Consequence: an unreclaimed frame whose slot stays `RELEASE_PENDING`-less and
   whose arena bytes head-of-line block reclamation at `ring.rs:2090-2092`, plus
   possibly a stranded charge, with no counter, no diagnostic, and no suspect record.
   The operator's only signal is that shared-memory capacity gradually stops being
   offered.

## Timing windows and dependencies

The drop-path window is the interval between `try_receive` returning a lease and the
explicit `lease.release()` at `ring_transport.rs:734-736`. In the shipped host that
interval contains the whole ingress-budget wait loop (`:702-730`), so it is not
narrow — it is as long as ingress is saturated, bounded by `frame_deadline`. The
custody-bool window is a single call at close. Configuration dependencies: none for
the drop path itself, but the *reachability* of step 2 depends on ingress budget
sizing and `frame_deadline`; and `HostConfig.liveness = None` by default
(`crates/host-runtime/src/config.rs:238`, `:250`) keeps the endpoint waiting on the
data doorbell rather than
failing, which lengthens the window in practice. No platform gating. This record is
the reason the other three charge-conservation properties would go unnoticed:
`quarantine-charge-transition-is-atomic`, `charge-release-never-silently-strands`,
and `custody-terminal-transition-exactly-once` all lose their evidence through these
same discards. It also overlaps `cancelled-frame-disposition-is-declared`, which owns
the *frame* loss in the same window; this record owns the *silence*.

## What a test must construct

A release that fails while the surrounding operation is otherwise clean. Two arms.
Arm 1, drop path: acquire a lease, quarantine the ring from the other side, then drop
the lease without releasing it, and assert that some counter, diagnostic, or suspect
record fires. This needs no failpoint — `Ring::enter_quarantine` is public
(`ring.rs:1915-1923`) — but it does need a second party, so a same-process two-`Ring`
arrangement or the existing two-process harness. Arm 2, custody bool: drive a clean
close on a candidate whose custody record has already been moved out of `Active`, and
assert the `false` return is surfaced rather than dropped. The oracle must be an
observation of a reporting surface, not of the ring state, because the property is
about observability. Fault class F3 is not strictly required for arm 1, which makes
this the cheapest of the group to make non-vacuous.

## Investigation log

### Q: Is silent loss on the drop path intended, given the addon `mem::forget`s leases and releases through its own table instead?

- Sources examined: `crates/shm-transport/src/lease.rs:324-372`;
  `packages/shm-native/src/lib.rs:332-357` (`detach_active`) and `:1345-1451`
  (`poll`, with `std::mem::forget(lease)` at `:1208` (source tree; not at HEAD));
  `packages/shm-native/index.ts:608-623`;
  `packages/plugin/src/shared/host-client/shm-frame-channel.ts:209-230` (source tree; not at HEAD) and
  `:323-370` (source tree; not at HEAD); former `crates/host-runtime/src/shm_provider.rs:363-371`, former `:546-619`;
  former `crates/host-runtime/src/provider_recovery.rs:137-179`;
  `crates/shm-transport/src/profile.rs:498-506`, `:551-566`.
- Findings: the addon genuinely does not use the drop path — `poll` forgets the
  lease at `lib.rs:1208` (source tree; not at HEAD) and completes through its own `active` table at `:332-357`,
  and that route *does* report failure all the way to `onClosed`. The host's
  explicit releases also report. So every deliberate completion path in the
  repository observes failure, and `Drop` is the fallback for paths that exit
  without completing. That makes the discard look like a considered choice — a
  destructor cannot return a `Result` — rather than an oversight. What it does not
  explain is why there is no counter or diagnostic at the discard site, which is a
  separate decision from not returning the error.
- Missing evidence: no comment at `lease.rs:366-372` states the reasoning. The
  pre-#131 doc comment on `release` (former `:172`) mentioned reporting stale or
  duplicate release but said nothing about the drop case; at HEAD `release`
  (`lease.rs:324-326`) carries no doc comment at all. `docs/shm-transport.md` does not cover
  drop-time completion. No plan requirement was found that names it.
- Conclusion: partially resolved. The mechanism is fully traced and the catalog's
  `AdmissionError` premise is corrected — there is no fallible admission surface on
  the clean-close path, so the record's `medium` confidence rested on a
  misidentified mechanism and the actual discarded signal is a `bool` plus two
  silent returns inside the controller. Whether the *silence* is intended still
  needs human input, because the fix shape differs: returning the error is
  impossible in `Drop`, but emitting a counter is not, and choosing between them is
  a design decision rather than something the code reveals.

## Refresh outcome, 2026-08-30

`Reaches production:` moved from `yes` to `no`; `Status:` stays `active`. This
record had two discard sites and the refactor removed one of them.

The transport-side site is unchanged and verified at `e447c927`:
`ReceiveLease::Drop` calls `release_once()` and discards the result
(`crates/shm-transport/src/lease.rs:366-372`).

The host-side site is gone. `let _ = custody.release()` at the former
`crates/host-runtime/src/shm_provider.rs:365` is now `admission.release()` at
`crates/host-runtime/src/ring_transport.rs:360`, and `Admission::release`
(`crates/shm-transport/src/profile.rs:553`) takes `self` and returns `()`.
There is therefore no host-side result to discard and no clean-path host release
failure to observe. The silent-no-op risk inside `AdmissionController::release`
did not disappear; it is now wholly owned by
`charge-release-never-silently-strands`, which cites the transport crate directly.
The `recovery.report_suspect(custody)` branch named as the existing check was
deleted with `provider_recovery.rs`, so the record's existing check is now none.

Reachability moved to `no` because the surviving discard is on the transport-side
lease drop path, and no shipped configuration selects the shared-memory transport.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 73, `packages/shm-native/src/lib.rs:327-331` now `packages/shm-native/src/lib.rs:350-354`: The failure is raised as `consumed_error("receive completion failed")`, which marks the token consumed so the wrapper releases its handle instead of offering a retry.
  - line 75, `shm-frame-channel.ts:227-245`: There is no plugin-side frame channel above the addon here, so a reported release failure stops at the `Error` thrown by `NativeReceiveLease.release` (`packages/shm-native/index.ts:608-618`).
  - line 90, `:525` now `:712`: Read cancellation inside the budget wait returns `Ok(false)` so the writer keeps draining; the endpoint loop then closes the inbound channel with `ReadClose::Cancelled` (`crates/host-runtime/src/ring_transport.rs:541-543`).
  - line 101, `crates/host-runtime/src/ring_transport.rs:276` now `crates/host-runtime/src/ring_transport.rs:360`: The host release is not unconditional: a ring that latched quarantine without a peer release takes `admission.quarantine()` instead (`ring_transport.rs:353-361`).
  - line 167, `lease.rs:160-162` now `lease.rs:324-326`: `release` carries a doc comment at HEAD (`crates/shm-transport/src/lease.rs:322-323`) stating that `Drop` does the same thing but discards the error.
  - line 189, `crates/host-runtime/src/ring_transport.rs:276` now `crates/host-runtime/src/ring_transport.rs:360`: The call is reached only on the non-quarantined branch (`ring_transport.rs:353-361`), so it is conditional rather than unconditional.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 75, `shm-frame-channel.ts:227-245` (close() reporting onClosed("quarantined", error)): `packages/plugin` does not exist in this tree; `packages/shm-native/index.ts` is the only TypeScript surface.
  - line 77, `shm-frame-channel.ts:411-416` (drain path reporting onClosed("protocol_violation", error)): `packages/plugin` does not exist in this tree, so no drain-path reporter above the addon remains.
  - line 149, `:1208` (std::mem::forget(lease) in poll): `poll` moves the lease into `channel.active` as an `ActiveLease` (`packages/shm-native/src/lib.rs:1397-1403`); the only remaining `mem::forget` is for a quarantined channel (`:472`).
  - line 151, `packages/plugin/src/shared/host-client/shm-frame-channel.ts:209-230` (plugin-side frame channel close path): `packages/plugin` does not exist in this tree.
  - line 152, `:323-370` (plugin-side drain path): `packages/plugin` does not exist in this tree.
  - line 156, `lib.rs:1208` (std::mem::forget(lease) in poll): The lease is stored in `channel.active` instead (`packages/shm-native/src/lib.rs:1397-1403`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
