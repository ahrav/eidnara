# iceoryx-completion-is-observable-to-the-host

Record invalidated 2026-08-31: iceoryx2 backend removed in `0f336d3c`; absent at
HEAD `46278f47a` after PR #131 (merge `5d638e3e8`).

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

An earlier pass characterized the iceoryx lease's `release` as a no-op. It is
not. `pub fn release(self) {}` (`backend/iceoryx.rs:349`) has an empty body, but
it takes `self` **by value**, so the closing brace runs the compiler-generated
drop glue for `IceoryxReceiveLease`, which drops its `sample: ByteSample` field
(`:320`), and iceoryx2's `Drop for Sample` calls
`receiver.release_offset(...)` (`iceoryx2-0.9.3/src/sample.rs:105-113`),
returning the chunk to the provider and freeing one borrow slot. The reclamation
is real. What the call does not do is anything else: it validates no identity,
increments no counter, publishes no completion, and returns no error. That is the
property to catalog, because the ring's release does all four.

## Evidence trail

- **The cited mechanism is gone.** `0f336d3c` ("refactor(shm): collapse to fixed
  ring transport") deleted `crates/shm-transport/src/backend/iceoryx.rs`,
  `crates/shm-transport/tests/iceoryx.rs`, and the `iceoryx` Cargo feature, so
  `backend/mod.rs` now declares only `ring` and `sample`. Every `iceoryx.rs`
  citation below is kept as a record of what the removed backend did and did not
  guarantee, and resolves against `9c1eb4d1`, not HEAD. No successor backend
  exists in the tree.

- `backend/iceoryx.rs:319-355` — the whole lease. There is **no** `impl Drop`
  anywhere in the file, so `release(self)` and simply letting the lease fall out
  of scope are the same operation, byte for byte. Nothing distinguishes a
  completed lease from an abandoned one, and there is no second release to
  reject, because the move consumed the value.
- `backend/ring.rs:1528-1600` — the counterpart, for the difference. It takes an
  identity, then checks quarantine (`:1529`), incarnation (`:1537`), lane (`:1540`),
  a zero sequence (`:1544`), `sequence <= consumed` (`:1556-1558`), and three
  descriptor fields re-read from shared memory (`:1565-1574`), before the
  arbitrating compare-exchange at `:1575-1580` maps a second attempt to
  `DuplicateRelease` (`:1581-1589`). Only then does it store
  `completion_sequence` and decrement `active_leases` (`:1591-1593`).
- `crates/shm-transport/src/lease.rs:350-357`, `:366-372` — the ring lease
  also carries a local `released` flag and a `Drop` that calls `release_once`, so
  an abandoned ring lease still completes and a duplicate is still named. The
  iceoryx lease has neither, so those two obligations are met by move semantics
  rather than by a check, which is sound but silent.
- `backend/ring.rs:1607-1624` `conservation` and `:1887-1892` `probe` — the ring's
  entire reporting surface: per-slot descriptor counts across six states and
  per-state byte charges. `backend/iceoryx.rs` has no equivalent. It exposes
  `try_reserve`, `try_receive`, and the associated `stale_node_observed`
  (`:177-188`), and nothing else. A caller cannot ask it how many samples are
  outstanding, how many bytes are charged, or whether it is healthy.
- former `crates/host-runtime/src/provider_recovery.rs:530` — readiness is decided by
  `shared.backend.probe() && shared.backend.admission_fits()`. There is no
  iceoryx path into that predicate, because there is no iceoryx `probe`.
- `backend/iceoryx.rs:178-189` `stale_node_observed` is the only observation the
  backend offers, and its own doc comment scopes it: it "reports a
  `NodeState::Dead` without performing cleanup or creating ports or services".
  It is a process-wide `Node::list` walk over the whole host, keyed to nothing
  about this backend's samples, so it cannot answer a question about this
  channel's outstanding leases.
- `benches/hardware_envelope.rs:597` (source tree; not at HEAD) — `run_iceoryx` returns
  `Ok((start.elapsed(), 0, 0, 0, 0, checksum))`. All five operation counters are
  literal zeros written by hand, not observations. `:177` (source tree; not at HEAD) dispatches the
  `iceoryx_0_9_3` arm into that function, `:186-197` (source tree; not at HEAD) copies those zeros into
  `OperationCounters`, and `:360` computes `disqualifications`, which is
  therefore empty. `:219` (source tree; not at HEAD) and `:256` (source tree; not at HEAD) then set
  `selectable: matches!(arm, "ring" | "iceoryx_0_9_3")`.
- `benches/manifests/v1.json:107-110` (source tree; not at HEAD) lists `ring` and `iceoryx_0_9_3` as the two
  `selectable` arms, and `:143-153` (source tree; not at HEAD) names all six counter fields as
  `required_counter_fields` for the selection gate. The gate's required inputs
  are supplied as constants on this arm.
- `benches/hardware_envelope.rs:141` (source tree; not at HEAD) — the bench's own report labels the arm
  `loopback_smoke_arms: ["iceoryx_0_9_3"]`, distinct from the nine
  `paired_process_arms` at `:289`.

## Failure scenario

A host that adopted this backend would have no way to observe reclamation, and
the release gate would not notice. Concretely: a caller that drops an
`IceoryxReceiveLease` on a cancellation path reclaims the sample correctly and
produces no record of it, so the outcome is identical to a caller that leaks the
lease into a long-lived collection — up to the borrow cap, at which point the
symptom surfaces on the *other* side as `ReceiveFailed` from
`ExceedsMaxBorrows`, attributed to the receive mechanism rather than to the
retained leases. There is no counter to check and no snapshot to compare, so the
diagnosis has no evidence.

The measurement half is live today rather than latent. The arm reports zero body
copies, zero allocations, zero syscalls, and zero park-wakes because those are
the literal values at `:597` (source tree; not at HEAD), and on that basis it is marked selectable. A copy
introduced anywhere in `run_iceoryx` would change none of them. This is the same
shape as `operation-counters-are-observed-not-declared`, but stricter: on the
ring arm the counters are at least derived from parameters, and here they are
constants.

## Timing windows and dependencies

No window and no fault. The absent surface is a static fact about the module, and
the hardcoded counters are a static fact about the bench. The only dependency is
the `iceoryx` feature, which is **on by default** for the transport crate
(`crates/shm-transport/Cargo.toml:9-10` (source tree; not at HEAD)) and off for both consumers, since
`crates/host-runtime/Cargo.toml:25` (source tree; not at HEAD) and `packages/shm-native/Cargo.toml:16` (source tree; not at HEAD) both
set `default-features = false`. So the backend and this bench arm compile
whenever the transport crate is built or tested on its own, and are absent from
every artifact the host or the addon ships.

## What a test must construct

Nothing exotic; two static assertions and one behavioural one. First, assert on
the bench report that every arm marked `selectable` produced its counters from an
observation on its own path — the negative control is to add a body copy inside
`run_iceoryx` and require `body_copies` to rise; today it stays zero. Second,
assert the iceoryx backend exposes a readiness and conservation observation with
the same shape the recovery predicate at former `provider_recovery.rs:530` consumes, or
assert the arm is not selectable without one. Third, for the completion half:
take `max_leases` leases, drop half by scope exit and release the rest
explicitly, and assert the two disposals are equivalent *and* that some
observation distinguishes outstanding from reclaimed. The third assertion cannot
be written against the current surface, which is the finding. Coverage check to
emit: `shm_iceoryx_lease_abandoned_without_release`.

## Investigation log

### Q: What does `release(self)` actually do, and can the host's accounting consume anything the iceoryx path produces?

- Sources examined: `backend/iceoryx.rs:319-355`, `:178-189`, and the whole file
  searched for `impl Drop`, `conservation`, `probe`, and `quarantine`, all
  absent; `backend/ring.rs:1528-1600`, `:1607-1624`;
  `crates/shm-transport/src/lease.rs:350-372`;
  former `crates/host-runtime/src/provider_recovery.rs:530`;
  `benches/hardware_envelope.rs:141` (source tree; not at HEAD), `:177` (source tree; not at HEAD), `:186-260` (source tree; not at HEAD), `:531-598` (source tree; not at HEAD);
  `benches/manifests/v1.json:100-155` (source tree; not at HEAD); and
  `iceoryx2-0.9.3/src/sample.rs:105-113`.
- Findings: `release(self)` performs a real reclamation through drop glue, not a
  no-op, and it is exactly equivalent to dropping the lease. Move semantics give
  exactly-once completion for free, so the ring's `DuplicateRelease` concern does
  not arise here. Everything else the ring's release provides — identity
  authority, a completion publication, an error on a wrong or stale identity, and
  a decrementable outstanding count — is absent, and so is the conservation and
  probe surface the host reads. The bench arm supplies the gate's required
  counters as constants and is nonetheless selectable.
- Missing evidence: whether `selectable: true` on this arm is deliberate given
  that the same report classifies it as a loopback smoke arm at `:141` (source tree; not at HEAD). The
  manifest's `selectable` list and the report's arm classification disagree in
  intent, and nothing reconciles them.
- Conclusion: resolved with answer, and the discovery input's "no-op"
  characterization is corrected. The exactly-once and reclamation halves hold by
  construction; the observability half does not exist.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 105, `:597`: The measurement half of this record is no longer live: the bench has no iceoryx arm and no `selectable` field at HEAD.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 76, `benches/hardware_envelope.rs:597` (run_iceoryx): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 78, `:177` (iceoryx_0_9_3 dispatch arm): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 79, `:186-197` (the iceoryx arm's OperationCounters fill): The counters are built at `benches/hardware_envelope.rs:337-358` at HEAD, with no iceoryx arm to fill.
  - line 81, `:219` (selectable assignment): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 81, `:256` (selectable assignment): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 83, `benches/manifests/v1.json:107-110` (the selectable arm list): `v1.json` is 122 lines at HEAD and names no `selectable` arms; its `arms` object starts at `:78`.
  - line 84, `:143-153` (required_counter_fields): `v1.json` has no `required_counter_fields` key and no line 143 at HEAD.
  - line 87, `benches/hardware_envelope.rs:141` (loopback_smoke_arms): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 105, `:597` (run_iceoryx's literal counter values): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 116, `crates/shm-transport/Cargo.toml:9-10` (the default-on iceoryx feature): `crates/shm-transport/Cargo.toml` has no `[features]` section at HEAD.
  - line 117, `crates/host-runtime/Cargo.toml:25` (default-features = false on shm-transport): Line 25 is now a plain `shm-transport = { workspace = true }` with no feature selection.
  - line 117, `packages/shm-native/Cargo.toml:16` (default-features = false on shm-transport): The dependency moved to `packages/shm-native/Cargo.toml:17` and selects no features.
  - line 146, `benches/hardware_envelope.rs:141` (loopback_smoke_arms): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 146, `:177` (iceoryx_0_9_3 dispatch arm): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 146, `:186-260` (the report assembly with its selectable field): The report is assembled at `benches/hardware_envelope.rs:262-320` at HEAD and has no `selectable` field.
  - line 146, `:531-598` (run_iceoryx): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
  - line 147, `benches/manifests/v1.json:100-155` (the manifest's selectable arms and counter-field gate): `v1.json` is 122 lines at HEAD with neither key.
  - line 158, `:141` (loopback_smoke_arms): No iceoryx arm remains in the bench at HEAD: `run_iceoryx`, the `iceoryx_0_9_3` dispatch, `selectable`, `loopback_smoke_arms`, and `required_counter_fields` are all absent.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
