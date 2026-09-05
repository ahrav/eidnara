# operation-counters-are-observed-not-declared

> Refresh note, 2026-08-31: PR #131 (merge `5d638e3e8`) rewrote
> `hardware_envelope.rs` and `evidence.rs` in ways that change this record's
> mechanism, not just its line numbers: `park_wakes` is now observed through a
> shared `AtomicU64` incremented at the wait site (`hardware_envelope.rs:283` (source tree; not at HEAD),
> `:354` (source tree; not at HEAD)), producer-side copy/allocation counting mixes per-site increments
> (`:716-718`) with bulk arithmetic (`:686-687`), the `SchedulingMode` label and
> the `iceoryx`/stream arms are gone, and
> `OperationCounters::disqualifications` now takes an
> `eventfd_wake_qualified` flag (`evidence.rs:28`) that waives the wake, syscall,
> and handoff gates. The per-counter provenance table and the derived prose
> below describe the pre-#131 bench and need mechanism-level re-derivation; the
> table's internal line numbers are left as pre-rewrite evidence. Citations
> outside the table were re-verified at HEAD.

## Discovery trigger

The release gate in `benches/manifests/v1.json` names six counter fields as
`required_counter_fields` (lines 147-154) and sets
`injected_gate_control_must_be_disqualified: true` (line 155). A gate that
decides whether a shared-memory provider may ship is only as good as the
provenance of the numbers it reads, so each counter was traced back from the
gate to the site that writes it.

## Evidence trail

`crates/shm-transport/src/evidence.rs` declares the six fields at lines 7,
9, 11, 13, 15, and 17, and reads them at lines 24, 27, 30, 34, 37, and 40 to
emit reason codes. The type performs no counting; it classifies values handed
to it.

Every write to any of the six fields, repository-wide, is one of:

- `crates/shm-transport/tests/evidence.rs:6-12` — a literal `1` per field
  in the `purity_gate_rejects_injected_copy_allocation_queue_and_wake` fixture
  (`#[test]` at line 461, `fn` at line 462).
- `crates/shm-transport/benches/hardware_envelope.rs:337-345` — construction
  from the `measure` tuple; `generic_queue_hops: 0` and `scheduler_handoffs: 0`
  are literals here.
- `benches/hardware_envelope.rs:346-359` — all six overwritten with `1` under
  `if arm == "injected_avoidable_operations"`.
- `benches/hardware_envelope.rs:374-382` — copied into the emitted
  `Measurement`.
- `benches/hardware_envelope.rs:400-408` — all six zeroed in `failed()`.

`OperationCounters` is imported by exactly two files outside its own module:
`tests/evidence.rs:1` and `benches/hardware_envelope.rs:11`. No production
(non-test, non-bench) code path increments any of the six fields.

Per-counter provenance in the bench, by arm family:

| Counter | `ring` / `h1` / ablations (`run_ring`) | `unix_socket` / `tcp` | `h0` | `iceoryx_0_9_3` |
| --- | --- | --- | --- | --- |
| `body_copies` | producer side observed at the `body.clone()` site (`:376-377`); receiver side computed as `copied_receiver as u64 * iterations` (`:397-398`) | `iterations * 2` (`:523`) | `0` (`:314`) | `0` (`:597`) |
| `native_allocations` | producer side observed at `:378`; receiver side computed at `:399` | literal `3` (`:524`) | `0` | `0` |
| `syscalls` | literal `0` (`:409`) | `iterations * 4` (`:525`) | `0` | `0` |
| `park_wakes` | `u64::from(scheduling == ColdParkWake) * iterations` (`:410`) | `0` (`:526`) | `0` | `0` |
| `generic_queue_hops` | literal `0` (`:187`) | literal `0` | literal `0` | literal `0` |
| `scheduler_handoffs` | literal `0` (`:188`) | literal `0` | literal `0` | literal `0` |

For the selectable `ring` arm, `copied_producer` and `copied_receiver` are both
`false` (`:323`), so none of the six counters is observed at an operation site:
four are literals and `park_wakes` is derived from the schedule label.

The receiver copy is `lease.to_vec()` in `ring_consumer` at `:757`, which runs
in the child forked at `:645`. The count is added in the parent at `:686-687`,
after `wait_child(child)` at `:672` — a different process, after the process
that performed the copy exited.

`park_wakes` derives from the mode label, not from wakes: the sleep is
`std::thread::sleep(Duration::from_micros(50))` at `:429` (source tree; not at HEAD), reached only when
`try_receive()` returns `Ok(None)`, so the true count is data-dependent while
the reported count is `iterations` exactly.

The gate control `injected_avoidable_operations` dispatches to
`run_ring(scheduling, iterations, payload, false, false)` at `:323` — argument
for argument the same body as the selectable `ring` arm at `:322-324` — and
then has all six counters replaced by `1` on the strength of its arm name.

## Failure scenario

A body copy is added to the `ring` receive path. `syscalls`,
`generic_queue_hops`, and `scheduler_handoffs` are literal zeros;
`body_copies` and `native_allocations` for the leased-receiver configuration
are `0 + (false as u64 * iterations) == 0`. `disqualifications()` returns an
empty vector, the reason string becomes "smoke evidence is never
designated-host qualification", and the arm reports as clean. The gate reads
`body_copies == 0` from a run that performed one copy per frame.

## Timing windows and dependencies

None. This is a static property of the harness: the values are assigned from
constants, arm labels, booleans, and iteration counts before any observation
could contradict them. No interleaving, fault, or race is needed to reach the
defect, and no timing makes it go away.

## What a test must construct

Two negative controls, each asserting that a counter *drops* when the
corresponding operation is removed:

1. Run the copied-receiver ablation, record `body_copies`, then run the same
   arm with the `lease.to_vec()` at `:757` replaced by the leased path, and
   assert the value falls by `iterations`. Today it falls because the boolean
   changed, not because the copy went away, so the control must instead keep
   `copied_receiver == true` and remove the copy — which currently produces no
   change and is therefore the discriminating case.
2. Run the cold arm, record `park_wakes`, then remove the `:429` (source tree; not at HEAD) sleep and
   assert the value falls. It will not.

Both controls require the counter to be incremented in the process and at the
site that performs the operation, which means the child must report its own
counts across the fork boundary rather than having them inferred after
`waitpid`.

## Investigation log

### Q: Is `OperationCounters` intended to be wired to real instrumentation, or is it permanently a report-schema type? If the latter, the "counts copies" language in `docs/shm-transport.md:25` (source tree; not at HEAD) overstates what any artifact can prove.

- Sources examined: `crates/shm-transport/src/evidence.rs` in full;
  repository-wide search for each of the six field names and for
  `OperationCounters` and `disqualifications`;
  `benches/hardware_envelope.rs` in full; `benches/manifests/v1.json`
  `selection_gate`; `tests/evidence.rs:3-30`;
  `docs/shm-transport.md:25` (source tree; not at HEAD).
- Findings: the type has no constructor that observes anything and no
  production caller. Its doc comment calls it "Operation counters used to
  produce disqualification reason codes", which describes a classifier, not an
  instrument. `docs/shm-transport.md:25` (source tree; not at HEAD) reads "Owned-buffer adapters
  count their copies separately and are never zero-copy evidence", which
  asserts that counting happens. No code performs that counting for the
  transport arms.
- Missing evidence: nothing in the repository states the intended design. The
  manifest requires the fields but not their provenance. No plan document
  reviewed for this part assigns ownership of instrumentation.
- Conclusion: needs human input. The mechanical facts are settled — no
  production path writes any counter, and the bench derives all six for the
  selectable arm — but whether that is a gap or the intended scope is a design
  decision that is not recorded anywhere in the tree.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 6, `hardware_envelope.rs:283`: `park_wakes` is now `syscalls.parks + peer_parks` at `crates/shm-transport/benches/hardware_envelope.rs:693`, with the child publishing its own `syscalls.parks` at `:785`.
  - line 8, `:302-304` now `:716-718`: Only the producer body copy is still counted per site (`copies += 1` at `:717`); allocations are bulk arithmetic, and the syscall, park, and scheduler counters come from `Ring::syscall_counters()` and `getrusage` in both processes.
  - line 11, `evidence.rs:22` now `evidence.rs:28`: The flag is named `doorbell_wake_qualified`, `OperationCounters` now has seven fields because `syscalls` split into `doorbell_syscalls` and `other_syscalls`, and the waiver applies only when `park_wakes` is nonzero.
  - line 38, `crates/shm-transport/benches/hardware_envelope.rs:131-137` now `crates/shm-transport/benches/hardware_envelope.rs:337-345`: Only `generic_queue_hops` is a literal here; `doorbell_syscalls`, `other_syscalls`, `park_wakes`, and `scheduler_handoffs` are taken from the measured `ArmRun` and its `SyscallSplit`.
  - line 41, `benches/hardware_envelope.rs:139-146` now `benches/hardware_envelope.rs:346-359`: Seven counters plus a synthetic `SyscallSplit` are overwritten for the gate-control arm.
  - line 45, `benches/hardware_envelope.rs:183-189` now `benches/hardware_envelope.rs:400-408`: `failed()` zeroes eight counter fields and sets `syscalls: None`.
  - line 63, `:166` now `:323`: Four of the seven counters are observed at HEAD: doorbell syscalls, other syscalls, and parks come from `Ring::syscall_counters()` in both processes, and `scheduler_handoffs` from `getrusage`, so the claim that none is observed at an operation site no longer holds.
  - line 72, `:429`: `park_wakes` is no longer derived from a mode label; it is the doorbell's own park count from both processes, so removing a sleep is not the relevant control.
  - line 78, `:165-167` now `:322-324`: The gate control, `direct_producer_leased_receiver`, and the selectable `ring` arm are literally the same match arm at HEAD, and the override sets seven counters, not six.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 6, `hardware_envelope.rs:283` (shared park_wakes increment at a wait site): The bench no longer increments a shared counter at a wait site; parks are counted by `Doorbell::record` in `crates/shm-transport/src/backend/ring.rs:762-769` and read through `Ring::syscall_counters()`.
  - line 7, `:354` (second shared park_wakes increment site): The construct is gone; at HEAD `:354` is the injected-arm override of `doorbell_syscalls`.
  - line 72, `:429` (std::thread::sleep in the consumer poll loop): The consumer now blocks on `ring.wait_for_data(deadline)` at `crates/shm-transport/benches/hardware_envelope.rs:749`.
  - line 109, `:429` (the cold-arm sleep): There is no `std::thread::sleep` in the bench; the consumer parks through the doorbell at `:749`.
  - line 119, `docs/shm-transport.md:25` (the owned-buffer copy-counting sentence): The trimmed document contains no copy-counting claim at all.
  - line 126, `docs/shm-transport.md:25` (the owned-buffer copy-counting sentence): The trimmed document contains no copy-counting claim at all.
  - line 130, `docs/shm-transport.md:25` (the owned-buffer copy-counting sentence): The trimmed document contains no copy-counting claim at all.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
