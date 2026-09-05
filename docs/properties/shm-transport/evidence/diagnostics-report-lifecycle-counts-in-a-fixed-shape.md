# diagnostics-report-lifecycle-counts-in-a-fixed-shape

## Discovery trigger

Five records in the catalog say "no counter fires" for a failure they describe,
while `RingTransport::diagnostics` publishes peer-death, reclamation,
activation, and exhaustion counts and `docs/shm-transport.md:71` documents them.
The report had a test and no record.

## Evidence trail

- `crates/host-runtime/src/ring_transport.rs:85-88` declare `activations`,
  `peer_deaths`, `reclamations`, `exhaustions` as `AtomicU64`; `:122-125`
  initialise them to zero.
- `pub fn diagnostics(&self) -> serde_json::Value` at `:139`. The report carries
  `artifact` (`:180`), `bounds` (`:185`), and at `:187-190` the four counters as
  `activation.completed`, `peer_death.observed`, `reclamation.completed`,
  `exhaustion.observed`, each loaded with `Ordering::Acquire`.
- Increment sites: `record_activation` (`:194-195`), `record_peer_death`
  (`:198-199`), `record_reclamation` (`:202-203`), all `fetch_add(1, Relaxed)`;
  the exhaustion increment sits inside `prepare` at `:221`.
- Wiring: `crates/host-runtime/src/connection.rs:172` calls
  `shared.ring.record_activation()`; `:185` calls
  `peer_ring.record_peer_death()`; `:193` calls
  `shared.ring.record_reclamation()`; `:640` passes
  `shared_task.ring.diagnostics()` into the `HostStatus` response.
- Test: `diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts`
  (`ring_transport.rs:862`) calls `record_activation`, `record_peer_death`, and
  `record_reclamation` once each (`:865-867`) and asserts `state == "healthy"`,
  `error_class == null`, `artifact.profile == RING_PROFILE`, `bounds.arena_bytes
  == limits.arena_bytes`, both accounting `arena_bytes` equal to `0`, and the
  four counts `1, 1, 1, 0`.
- Documentation: `docs/shm-transport.md:68-72` lists process bounds, active and
  quarantined accounting, completed activation counts, observed peer-death
  count, and completed reclamation count as the reported surface.

## Failure scenario

A lifecycle path that forgets its `record_*` call, or a refactor that moves the
call behind a branch not taken on the failure path, turns a peer death or a
reclamation that happened back into silence. The report still validates against
its shape, and the only test increments the counters itself, so nothing fails.

## Timing windows and dependencies

Relaxed increments and Acquire loads: a report racing an event may lag by one
but never decreases. No other window.

## What a test must construct

A live connection driven through activation, peer death, and reclamation via
`connection.rs`, with a status request after each step, asserting the count
increments at each step and the shape stays closed.

## Investigation log

### Q: Are the counters bumped on the shipped path or only by the test?

- Sources examined: `connection.rs:172`, `:185`, `:193`; `ring_transport.rs:221`,
  `:862-890`.
- Findings: three of the four are called from `connection.rs`; exhaustion is
  counted inside `prepare`; the test bypasses `connection.rs`.
- Missing evidence: a test that reaches the `connection.rs` call sites.
- Conclusion: `default-production` for the surface; `partial` for exercise.
