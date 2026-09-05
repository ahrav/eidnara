# diagnostics-report-lifecycle-counts-in-a-fixed-shape

## Discovery trigger

Five records in the catalog say "no counter fires" for a failure they describe,
while `RingTransport::diagnostics` publishes peer-death, reclamation,
activation, and exhaustion counts and `docs/shm-transport.md:70-73` documents them.
The report had a test and no record.

## Evidence trail

- `crates/host-runtime/src/ring_transport.rs:132-138` declare `activations`,
  `peer_deaths`, `reclamations`, `exhaustions`, and `endpoint_panics` as
  `AtomicU64`; `:172-176` initialise them to zero.
- `pub fn diagnostics(&self) -> serde_json::Value` at `:190`. The report carries
  `artifact` (`:231`), `bounds` (`:236`), and at `:238-242` the five counters as
  `activation.completed`, `peer_death.observed`, `reclamation.completed`,
  `exhaustion.observed`, and `endpoint_panic.observed`, each loaded with
  `Ordering::Acquire`.
- Increment sites: `record_activation` (`:246-247`), `record_peer_death`
  (`:250-251`), `record_reclamation` (`:254-255`), all `fetch_add(1, Relaxed)`;
  the exhaustion increment sits inside `prepare` at `:273`, and the endpoint-panic
  increment sits in the `catch_unwind` failure arm of the endpoint thread at
  `:344`, which also retires the connection and pushes a `Corrupt` close into the
  inbound queue (`:345-348`).
- Test: `endpoint_panic_is_reported_while_the_inbound_queue_is_full`
  (`ring_transport.rs:1686-1740`) drives a real panic through the endpoint body
  and asserts `endpoint_panic.observed == 1` (`:1739`).
- Wiring: `crates/host-runtime/src/connection.rs:188` calls
  `shared.ring.record_activation()`; `:201` calls
  `peer_ring.record_peer_death()`; `:209` calls
  `shared.ring.record_reclamation()`; `:625` passes
  `shared_task.ring.diagnostics()` into the `HostStatus` response.
- Test: `diagnostics_report_fixed_identity_bounds_accounting_and_lifecycle_counts`
  (`ring_transport.rs:1137`) calls `record_activation`, `record_peer_death`, and
  `record_reclamation` once each (`:1140-1142`) and asserts `state == "healthy"`,
  `error_class == null`, `artifact.profile == RING_PROFILE`, `bounds.arena_bytes
  == limits.arena_bytes`, both accounting `arena_bytes` equal to `0`, and the
  four counts `1, 1, 1, 0`.
- Documentation: `docs/shm-transport.md:68-72` lists process bounds, active and
  quarantined accounting, completed activation counts, observed peer-death
  count, and completed reclamation count as the reported surface.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

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

- Sources examined: `connection.rs:188`, `:201`, `:209`; `ring_transport.rs:273`,
  `:1137-1176`.
- Findings: three of the four are called from `connection.rs`; exhaustion is
  counted inside `prepare`; the test bypasses `connection.rs`.
- Missing evidence: a test that reaches the `connection.rs` call sites.
- Conclusion: `default-production` for the surface; `partial` for exercise.

### Q: Does the test pin the closed key set?

- Sources examined: `ring_transport.rs:1137-1176`.
- Findings: the assertions cover `state`, `error_class`, three `artifact` keys,
  `bounds.arena_bytes` (`:1156`), one `accounting` field per side (`:1157-1158`),
  and the four counts; the seven-name absence check at `:1164-1175` is by key
  name, not by planted value. No assertion says the key set is exactly the
  closed set.
- Missing evidence: a test that drives the events through `connection.rs` and
  a value-level redaction check on the report.
- Conclusion: Exercised covers the counter-to-key mapping and part of the shape;
  the Check drives the events through the connection path.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 16, `:187-190` now `:238-241`: The report carries a fifth lifecycle counter at HEAD, `endpoint_panic.observed` (`ring_transport.rs:242`), backed by the `endpoint_panics` field (`:138`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
