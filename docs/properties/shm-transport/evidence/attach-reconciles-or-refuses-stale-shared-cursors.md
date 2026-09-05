# attach-reconciles-or-refuses-stale-shared-cursors

## Discovery trigger

Every cursor that governs progress — `published`, `consumed`, `completed`,
`arena_write`, `arena_reclaimed`, `active_leases` — lives in the shared mapping, not
in either process. So a process death leaves them exactly where the dead process
left them. Reading `Ring::attach` to find the reconciliation step showed there is
none: attach validates geometry and identity, wires the transferred eventfd
doorbells, and returns. Then reading
`LifecyclePage` showed there is no field a reconciliation could consult even if one
were added.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1095-1150` `attach` — the whole
  function: `grant.checked_layout()?` (`:1103`), a `total_bytes` conversion (`:1105`),
  `Mapping::attach` (`:1106`), `validate_lifecycle` (`:1107`), then construct and
  return, including the two `Doorbell::from_fd` conversions (`:1123-1149`;
  `prefault_read` is gone from attach post-#131). It never reads a cursor, a slot
  state, or the quarantine flag.
- `ring.rs:2813-2831` `validate_lifecycle` — re-verified at post-#131 HEAD.
  It reads exactly eight fields (`:2818`): `magic`,
  `layout_version`, `descriptor_depth`, `arena_bytes`, `max_leases`, `total_bytes`,
  `incarnation`, `lane`, and compares each against the expected grant
  (`:2819-2826`). Notably it does not read `quarantined` either.
- `ring.rs:209-219` `LifecyclePage` — the complete field list is the eight above plus
  `quarantined: AtomicU8`. There is no holder count, attach epoch, heartbeat,
  generation, or peer pid, which confirms the catalog's claim that no field exists
  for a reconciliation to read.
- `ring.rs:147-154` `DescriptorSlot` — `state`, `completion_sequence`,
  `reservation_len`, `descriptor`. All four survive the death of whichever process
  last wrote them.
- `ring.rs:66-85` — `ProducerPage { published, arena_write }`,
  `ConsumerPage { consumed, active_leases }`,
  `ReclaimPage { completed, arena_reclaimed }`. Six cursors, all in shared memory,
  none reset on attach.
- Why the symptoms are all benign codes:
  `ring.rs:1417-1422` — `if active >= self.grant.max_leases { return Ok(None); }`, with
  the comment "A full lease set is backpressure, not a fault";
  `ring.rs:2090-2092` — `reclaim_completed` breaks at the first slot whose
  `completion_sequence` does not match the next expected sequence, so reclamation
  head-of-line blocks at the lowest stale sequence;
  `ring.rs:1293-1295` — `try_reserve` returns `ProducerError::Exhausted` once
  `published - completed` reaches `descriptor_depth`;
  `ring.rs:1354` — `reserve_until` converts sustained `Exhausted` into
  `ProducerError::Deadline`. None of these calls `enter_quarantine`, and
  `enter_quarantine` (`ring.rs:1915-1922`) is the only writer of the quarantine flag.
- Non-test attach callers: `packages/shm-native/src/lib.rs:286-288` `attach_ring`
  (`Ring::attach` at `:287`), reached from the bootstrap at `:740-741`, and
  `ring.rs:979` inside `RingAttachment::attach`. The host-side
  the host-side `attach_ring` that opened `/proc/{pid}/fd/{fd}` is gone: `ed487e11`
  deleted it with `shm_provider.rs`, and its successor
  `crates/host-runtime/src/ring_transport.rs:855-882`
  `RingClientEndpoint::attach_with_descriptors` receives already-transferred
  descriptors instead of opening one.
- Existing check: none, confirmed against the rewritten
  `crates/host-runtime/tests/shm_failure_modes.rs` (post-#131). Its kill-based tests
  (`:213`, `:225`) and the restart test (`:302`) never perform a post-kill attach
  to the same object; the file has no attach call at all. The pre-rewrite
  six-test inventory (`:105`…`:358`) no longer exists.

## Failure scenario

1. A receiver attaches and takes `K == max_leases` leases. Each lease sets its slot
   to `RECEIVER_LEASED` (`ring.rs:1452`), advances `consumed` (`:1453`), and increments
   `active_leases` (`:1454`).
2. The receiver is killed. None of `ReceiveLease::Drop`
   (`crates/shm-transport/src/lease.rs:366-372`) runs, so no release is recorded
   and `completion_sequence` stays 0 for all K slots.
3. A fresh process attaches with the same grant. `validate_lifecycle` compares the
   eight geometry and identity fields, all of which still match, and attach succeeds.
4. The new receiver calls `try_receive`. `active_leases` still reads `K`, so the
   check at `:1417-1422` short-circuits and returns `Ok(None)` — indistinguishable from
   an empty ring.
5. The producer calls `try_reserve`. `reclaim_completed` cannot advance past the
   lowest stale sequence (`:2090-2092`), so `completed` is frozen; `published`
   continues until `published - completed == descriptor_depth`, then `:1293-1295`
   returns `Exhausted`, and `reserve_until` reports `Deadline` (`:1354`).
6. Consequence: the channel is permanently dead in both directions, every symptom is
   a legal backpressure code, `is_quarantined()` is false, `conservation()` still
   conserves, no charge is retained as quarantined, and no recovery episode starts.
   Nothing distinguishes this from a slow but healthy peer.

## Timing windows and dependencies

There is no narrow window: the stale state is permanent from the moment the receiver
dies until the object is destroyed. The kill must land while at least one lease is
held, which in the shipped client path means between `poll`'s
`std::mem::forget(lease)` (`packages/shm-native/src/lib.rs:1256` (source tree; not at HEAD)) and the
corresponding `detach_active` completion (`:332-357`) — that is, while a frame is in
JavaScript hands. The worst case is `K == max_leases`, because then even the lease
bound alone kills the receive direction. Configuration dependencies:
`HostConfig.liveness` is `None` by default
(`crates/host-runtime/src/config.rs:239`, `:251`), so nothing probes the peer and the
receive side waits on the data doorbell indefinitely (`wait_for_data`,
`ring.rs:1476-1499`); with a liveness policy configured the
ring instead fills and a failed publish makes the close unclean, which is the same
divergence `dead-peer-charges-are-reclaimed-or-declared` records. Platform gating:
post-#131 attach receives already-transferred descriptors plus two eventfd
doorbells (`ring.rs:1095`), so the attach path is Linux-only via `eventfd`
(`ring.rs:389` (source tree; not at HEAD)); the former `/proc/{pid}/fd/{fd}` open is gone.

One scoping correction worth stating plainly. In the shipped two-process topology a
*replacement* peer does not attach to the dead peer's object — each candidate gets a
fresh `DuplexRing` (`ring.rs:2604-2612`) with a fresh random incarnation (`:1051`).
The literal "fresh attach inherits stale leases" sequence therefore requires the
same descriptor to be re-offered, which the activation-token fence formerly
exercised by `shm_failure_modes.rs:358`
`restart_with_same_identity_rejects_stale_activation` was designed to prevent at
the negotiation layer, not at attach (that test was removed with the pre-#131
harness; `907746f7b`). The shipped-topology manifestation is the
surviving side keeping a ring whose peer-side cursors are frozen. Both framings share
one root, no reconciliation and no liveness field, and the property is worth keeping
in its attach form because `validate_lifecycle` is where a reconciliation or refusal
would have to live.

## What a test must construct

An actual process termination while leases are held, then an attach — fault class F1,
which the harness formerly implemented, at a kill point it did not yet offer. The
pre-#131 harness had `RoleProcess::kill`
(former `crates/host-runtime/tests/support/shm_process.rs:257-263`),
`reap_killed` asserting signal-9 status (former `:272-292`), and
`observation_window` (former `:266-269`); that support file was deleted by
`907746f7b`, so the scenario harness must be rebuilt. Its five
scenarios were `idle`, `publish`, `pending`, `roundtrip`, and
`roundtrip_park` (former `:712-749`), and none of them could hold a lease across
the kill, because the client endpoint's `recv` releases inside itself at
`crates/host-runtime/src/ring_transport.rs:952-953` before returning. So a new scenario is
required that receives without releasing — K frames, ideally `K == max_leases` — and
emits a barrier record before parking. The oracle after the attach: either the attach
fails, or `active_leases == 0` and no slot remains in `RECEIVER_LEASED`. Add the
stronger liveness arm too: after the attach, a newly published frame must arrive
within `frame_deadline`, since `Ok(None)` forever is the actual failure and an
`active_leases` assertion alone would not catch a partial reconciliation. Coverage
check to emit: `shm_kill_with_leases_held`.

## Investigation log

### Q: Is a peer crash meant to be recoverable at all? If yes, something must reset the cursors or force quarantine; today it does neither

- Sources examined: `ring.rs:1095-1150`, `:2813-2831`, `:209-219`, `:66-85`,
  `:147-154`, `:1915-1940`, `:1417-1422`, `:1293-1295`, `:1354`, `:2070-2151`, `:2604-2612`;
  `packages/shm-native/src/lib.rs:286-288`, `:634-741`, `:1394-1500`;
  `crates/host-runtime/src/config.rs:239`, `:251`;
  `crates/host-runtime/tests/support/shm_process.rs:256-292`, `:644-757` (file since
  deleted by `907746f7b`);
  `crates/host-runtime/tests/shm_failure_modes.rs` test inventory;
  `crates/host-runtime/src/ring_transport.rs:855-955` (the branch formerly at
  `shm_provider.rs:363-371` is gone; `ed487e11` replaced it with the
  unconditional `admission.release()` at `ring_transport.rs:360`).
- Findings: the mechanism half is fully resolved and matches the catalog. There is no
  reconciliation, no reset, and no field to reconcile against; the three progress
  paths all degrade to legal backpressure rather than to a fault; and the quarantine
  flag is written only by explicit `enter_quarantine` calls, none of which are on a
  crash path. The recovery machinery that does exist operates one level up, on
  candidate custody and activation tokens
  (the suspect-versus-release branch and `CandidateCustody`, both deleted by
  `ed487e11`), and its answer to a
  dead peer is to retire or isolate the *candidate*, never to repair the *mapping*.
  That is internally consistent with "a crashed peer ends this candidate", which
  would make cursor reconciliation unnecessary by design.
- Missing evidence: no document states which of the two intentions holds.
  `docs/shm-transport.md` describes a recovery contract in terms of
  candidates and charges and does not say whether a mapping is ever meant to outlive
  a peer. If the intent is "a crashed peer ends the candidate", the property should
  be restated as a refusal obligation on attach and the record's liveness framing is
  wrong; if the intent is "the mapping is reusable", something must reset six cursors
  and the `LifecyclePage` needs a field it does not have. The code is consistent with
  both readings, so it cannot arbitrate.
- Conclusion: needs human input. The mechanism is established with evidence and
  requires no further investigation; the normative question decides whether the test
  above asserts reconciliation or asserts refusal, and those are different tests with
  different oracles.

### Q: Does attach inspect the shared cursors at HEAD? (added 2026-09-05)

- Checked: `Ring::attach` (`ring.rs:1095-1150`) loads `published`, `arena_write`, `completed`, `arena_reclaimed`, `consumed`, and `active_leases`, refuses a quarantined ring (`:1141-1142`), and runs `conservation_inner(true)` (`:1148`). Six unit tests (`:3494`, `:3567`, `:3603`, `:3697`, `:3766`, `:3780`) refuse inconsistent cursors, phantom leases, orphaned receiver slots, a quarantined ring, a write cursor past the committed frames, and a live slot whose descriptor does not validate.
- Conclusion: yes, for inconsistent state. Stale-but-consistent state left by a receiver killed while holding leases still passes conservation and is inherited; that residual is what the record now describes.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 16, `crates/shm-transport/src/backend/ring.rs:783-798` now `crates/shm-transport/src/backend/ring.rs:1095-1150`: At HEAD `attach` loads all six shared cursors as this handle's baseline, refuses a quarantined mapping, and runs `conservation_inner(true)`, so it does read cursors, slot states, and the quarantine flag.
  - line 90, `packages/shm-native/src/lib.rs:1256`: The lease is owned by the channel's `active` map from `poll` until `detach_active` releases it, so the window is the life of that entry.
  - line 101, `ring.rs:783` now `ring.rs:1095`: Attach receives three already-transferred descriptors and each doorbell is one end of an AF_UNIX socketpair rather than an eventfd.
  - line 152, `ring_transport.rs:276` now `ring_transport.rs:360`: The release is not unconditional at HEAD: a quarantined ring whose peer has not released its attachment moves its charges through `admission.quarantine()` instead (`ring_transport.rs:353-361`).
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 90, `packages/shm-native/src/lib.rs:1256` (std::mem::forget(lease) in poll): `poll` no longer forgets the lease; it moves it into `channel.active` as an `ActiveLease` at `packages/shm-native/src/lib.rs:1445-1451`.
  - line 102, `ring.rs:389` (eventfd doorbell creation): No eventfd remains in the ring backend: `Doorbell::create` builds an AF_UNIX socketpair at `ring.rs:727-738`, and Linux-only support is enforced by the `compile_error!` at `ring.rs:18-19`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
