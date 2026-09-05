# dead-peer-charges-are-reclaimed-or-declared

## Citation refresh, 2026-08-31 (eventfd rewrite)

PR #131 (merge `5d638e3e8`) replaced the polling wake mechanism with sparse
eventfd doorbells, and the surrounding host code changed with it. Three claims
below are now historical: the endpoint no longer polls `try_receive` in a sleep
loop (it parks on the `data_ready` doorbell); `docs/shm-transport.md` is
now 85 lines and no longer contains the retention-gap paragraph formerly at
`:106-108` or the unqualified accounting sentence formerly at `:57`; and the
pinning test `killed_victim_holding_active_charges_is_never_reclaimed` no longer
exists in `crates/host-runtime/tests/shm_failure_modes.rs`. The Discovery trigger and
Investigation log are kept as history; the sections between them are rewritten
against HEAD.

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

`docs/shm-transport.md:57` states the accounting claim without
qualification: "Admission accounts active and quarantined descriptor, arena,
lease, mapping, and pinned-worker commitments." Reading the close path for the
provider's owner thread showed only two ways charges leave `active`, and neither
is reachable when a peer dies silently. The transport document already records
this as a gap at `:106-108`, so the record exists to make the gap a claim under
test rather than a paragraph.

## Evidence trail

- Detection is out of band at HEAD. The setup socket is kept open as the
  peer-lifetime sentinel: `crates/host-runtime/src/connection.rs:195-207` spawns a
  watcher whose `observe_peer` arm records a peer death for any non-`Goodbye`
  closure (`:200-202`) and cancels the generation and read tokens (`:203-204`).
  `docs/shm-transport.md:49` states the contract: "Unexpected closure
  records peer death, cancels ring work, and tears down the exact connection."
- The ring path alone detects nothing, by construction. A dead peer never signals
  the `data_ready` doorbell, so the endpoint arms the wait
  (`crates/host-runtime/src/ring_transport.rs:566`) and parks in the readiness select
  (`:582-617`) until a cancellation token or queue event fires. `try_receive`
  returning `Ok(None)` on emptiness
  (`crates/shm-transport/src/backend/ring.rs:1424-1426`) is not an error, so
  nothing quarantines. The former shape — a 50-microsecond poll loop observing
  `Ok(false)` forever — is gone; the steady state is now a parked thread.
- Release is unconditional. The endpoint thread runs `run_endpoint` under
  `catch_unwind` (`ring_transport.rs:331-342`) and then calls
  `admission.release()` (`:360`) whether the endpoint returned or panicked. The
  pre-refactor release-versus-suspect branch (former `shm_provider.rs:364-371`)
  has no successor; both a sentinel-triggered cancellation and a publish failure
  (`ring_transport.rs:622-630`) end at the same release.
- `crates/host-runtime/src/config.rs:239` and `:251` — `pub liveness:
  Option<LivenessPolicy>` still defaults to `None`, so by default nothing on the
  host side writes to the ring on a timer; with outbound traffic queued, a dead
  consumer surfaces as `reserve_until` parking on `capacity_ready` and returning
  `Deadline` at `frame_deadline` (`ring.rs:1379-1382`, `:1383-1384`).
- `crates/host-runtime/tests/shm_failure_modes.rs:213-222`
  `setup_active_and_idle_sigkill_each_return_exact_capacity` — the current
  exercise. For setup, active, and idle victims it SIGKILLs a child holding a
  connection (`Victim::kill` requires a signal-9 wait status, `:154-158`) and
  then proves reclaim by readmitting at `max_connections = 1`
  (`connect_after_reclamation`, `:170-181`). `:225-255`
  `repeated_crashes_do_not_ratchet_single_connection_capacity` repeats the cycle
  twelve times against a process-resource envelope. The former pinning test that
  asserted retention is gone; the suite now asserts the opposite outcome.
- What no test asserts: a per-identity ledger. Readmission at a one-connection
  cap shows enough aggregate capacity returned; it does not show the killed
  candidate's exact tuple returned, and no accounting snapshot exposes an
  "unreclaimable" class.
  At HEAD: Release is conditional at HEAD: `:353-361` quarantines the admission when either ring is quarantined and the peer has not released it, and calls `admission.release()` only otherwise.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A peer commits a candidate, holds it idle, and is killed. Under the eventfd
mechanism the ring goes silent rather than busy: the endpoint is parked on the
`data_ready` doorbell and nothing on the ring path will ever wake it on the
peer's behalf. The guarantee now rests entirely on the out-of-band chain: kernel
closes the setup socket, the sentinel watcher observes non-`Goodbye` closure and
cancels (`connection.rs:200-204`), the endpoint's select wakes on the
cancellation token (`ring_transport.rs:589`, `:616`), the thread joins, and
`admission.release()` returns the charges (`:360`). A defect anywhere in that
chain — the watcher not spawned, the cancellation arm not wired, the endpoint
parked outside the select, or release skipped on a panic path — strands the
charges silently: readiness stays healthy, the parked endpoint is
indistinguishable from an idle one, and with single-candidate limits the next
admit is refused, permanently ending shared-memory eligibility for the process.

## Timing windows and dependencies

The reclaim window is bounded by the sentinel, not the ring: it opens at the
kernel's socket-closure edge on peer exit and closes when `admission.release()`
runs after the endpoint thread joins.

**The join is bounded without a draining inbound receiver.** Every
`select!` arm the endpoint parks in is cancellation-aware
(`ring_transport.rs:582-617`) and the one synchronous wait, `reserve_until`, is
deadline-bounded by `frame_deadline` (`ring.rs:1379-1382`, `:1383-1384`).
`receive_one`'s two bounded-channel hand-offs, the oversized-control rejection
(`ring_transport.rs:688-694`) and the ordinary frame hand-off (`:737-745`), both
go through `deliver` (`:649-661`), whose `select!` races `inbound.send(event)`
against `queue.discard.cancelled()` and `root.cancelled()` and maps either
cancellation to `ReadClose::Cancelled`. In the source tree this record was written
against those two sends were direct `inbound.send(...).await` calls with no
cancellation arm, so a connection task that retained the receiver without draining
it could park the endpoint indefinitely and `admission.release()` might never run;
at HEAD a full channel yields to `discard` or `root` cancellation instead.
At HEAD: both hand-offs go through `deliver` (`:649-661`), whose `select!` carries `queue.discard` and `root` cancellation arms, so the send is no longer uncancellable.

The fault-free bound is therefore one socket-closure delivery plus at most one
`frame_deadline` plus the cancellation of `root` or `queue.discard`: `fail`
cancels `root` (`ring_transport.rs:635-646`) and `FrameSender::discard` cancels
`discard` (`crates/host-runtime/src/frame_channel.rs:233-237`). A non-draining receiver delays the
join only until that cancellation lands; it no longer retains the admission
charge permanently. Nothing polls for peer liveness on the
ring, and the ring carries no holder count, attach epoch, heartbeat, or peer pid
a reaper could read. Depends on `custody-terminal-transition-exactly-once` for
release being correct at all, and shares its root cause with
`attach-reconciles-or-refuses-stale-shared-cursors` and
`crashed-producer-does-not-wedge-the-sequence`.

## What a test must construct

An actual `SIGKILL` of a process holding a committed candidate, with signal-9
wait status required — the harness in `crates/host-runtime/tests/shm_failure_modes.rs`
already does this (`:154-158`). The oracle must be a per-identity charge ledger,
not an aggregate: after reap, either the killed candidate's exact tuple returns
to free capacity, or the snapshot exposes it under a distinct unreclaimable
class that the admission contract subtracts from its cap. The existing SIGKILL
tests assert readmission at a one-connection cap, which is the aggregate form of
the first arm only. A complete test also bounds the window: assert the
readmission succeeds within an explicit bound anchored to the reap (one
socket-closure delivery plus one `frame_deadline` plus recorded slack), so a
teardown that leaks the endpoint thread and only releases at daemon exit fails
rather than passes slowly. The idle-victim arm matters most under eventfd,
because it is the arm where the ring provides no wake at all and the sentinel
chain is the only mechanism under test.

## Investigation log

### Q: Which behaviour is normative when a liveness policy is configured — retention, or quarantine via a failed publish?

- Sources examined: `crates/host-runtime/src/config.rs:235-297` and `:371-382` for
  the policy shape and its default; `crates/host-runtime/src/connection.rs:291-301`
  for where a liveness loop is spawned per generation;
  former `crates/host-runtime/src/shm_provider.rs:475-503` and former `:538-541` for both close
  classifications; `docs/shm-transport.md:96-112` for the documented
  failure and close contract.
- Findings: both outcomes are reachable and neither is written down. The
  document describes the retention outcome only, and does so as a gap rather
  than as a contract. the source shared-memory transport task is the umbrella T3 transport task
  in `IN_PROGRESS`, not a defect record for this behaviour; its description and
  notes do not mention dead-peer reclamation or a retained-tuple manifest, so
  the document's "pending the frozen retained-tuple manifest
  (the source shared-memory transport task)" points at a task that does not itself scope the
  work.
- Missing evidence: any statement of intent that ranks the two outcomes. No
  plan, manifest, or bead expresses a preference, and
  `crates/shm-transport/benches/manifests/v1.json` carries an empty
  retained-tuple list rather than a policy.
- Conclusion: needs human input. The catalog record should keep both outcomes
  listed as reachable; a test cannot be written until one is chosen, because the
  two arms have opposite oracles.

### 2026-08-31: re-derivation against the eventfd doorbell mechanism

- Sources examined: `crates/host-runtime/src/ring_transport.rs:305-363`, `:472-632`,
  `:622-630`; `crates/host-runtime/src/connection.rs:195-207`;
  `crates/host-runtime/src/config.rs:239-251`;
  `crates/shm-transport/src/backend/ring.rs:1187-1220`, `:1345-1390`,
  `:1424-1426`; `crates/host-runtime/tests/shm_failure_modes.rs:118-166`, `:170-199`,
  `:212-255`; `docs/shm-transport.md` (whole file, 85 lines).
- Findings: the record's original premise — an endpoint that polls
  `try_receive → Ok(false)` forever and never becomes a suspect — has no referent
  at HEAD. Under eventfd a dead peer looks like a parked endpoint: no doorbell
  signal, no poll, no ring-path detection ever. Detection and reclaim moved
  wholly out of band to the setup-socket sentinel
  (`connection.rs:200-204`), and release became unconditional at
  `ring_transport.rs:360`, so the former release-versus-suspect open question is
  resolved by code change: both close classifications end in release. The suite
  flipped with it — the retention-pinning test is gone and
  `setup_active_and_idle_sigkill_each_return_exact_capacity` now asserts reclaim
  by readmission. The guarantee survives with the reclaim arm exercised in
  aggregate; the declared-exception arm has no remaining code mechanism and no
  remaining documented claim to test against.
- Missing evidence: a per-identity charge ledger oracle, and any measured bound
  on sentinel-to-release latency to anchor the window assertion.
- Conclusion: resolved with answer for the mechanism (sentinel plus unconditional
  release, both cited); the per-identity oracle remains the open test gap carried
  in the catalog record.
  At HEAD: Release is conditional at HEAD: `:358` quarantines the admission when a ring is quarantined and the peer has not released it, and `:360` releases it otherwise.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 59, `:276` now `:360`: Release is conditional at HEAD: `:353-361` quarantines the admission when either ring is quarantined and the peer has not released it, and calls `admission.release()` only otherwise.
  - line 109, `:510-515` now `ring_transport.rs:688-694`: At HEAD both hand-offs go through `deliver` (`:649-661`), whose `select!` carries `queue.discard` and `root` cancellation arms, so the send is no longer uncancellable.
  - line 184, `ring_transport.rs:276` now `ring_transport.rs:360`: Release is conditional at HEAD: `:358` quarantines the admission when a ring is quarantined and the peer has not released it, and `:360` releases it otherwise.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
