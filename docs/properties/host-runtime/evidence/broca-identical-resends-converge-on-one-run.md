# broca-identical-resends-converge-on-one-run

## Discovery trigger

The supervisor module doc at `supervisor.rs:3` says the shared index lock
"linearizes deduplication, capacity release, and deletion". The `Run` struct
carries a `fingerprint: [u8; 32]` with the comment "byte-identical resend
matches, anything else conflicts" (`supervisor.rs:163-165`). Two harness
clients retrying the same prompt is the ordinary failure mode this guards, so
the question was whether the fingerprint is computed over the exact wire bytes
and whether the check happens under the same lock that publishes the run.

## Evidence trail

All references are at `e16e39e`.

The fingerprint is `Sha256::digest(body)` at `supervisor.rs:331`, taken from
the `body: &[u8]` parameter of `Supervisor::send` (`:323-328`). The only
production caller is `BrocaComponent::handle`, which passes `&ctx.body` at
`mod.rs:236`, the request frame's exact bytes. So the fingerprint is over wire
bytes, not over the decoded `SendRequest`. `send_pair` in the test file builds
the body by serialising the same JSON the wire carries
(`tests/broca_supervisor.rs:28-43`).

The dedup decision sits under the index lock. `lock_index` is taken at
`supervisor.rs:349`; the match on `index.sessions.get(key)` at `:356-375`
returns `Ok(run.run_id.clone())` when the live run's fingerprint equals the
new one (`:358-359`), returns `idempotency_conflict` otherwise (`:361-364`),
and returns `session_deleted` when the entry is a tombstone (`:367-373`). Only
when there is no entry does admission continue to `:376-415`, where the run is
inserted into both indices (`:407-410`) and spawned while the lock is still
held (`:414`). The comment at `:354-355` records the ordering rule: the
byte-identity check precedes the capacity check, so a resend of an existing
run returns its ID even when every reservation at `:338-345` failed.

The reservations taken before the lock (`run_permit` at `:338`, `charge` at
`:339-345`) are locals. On the dedup and conflict early returns they drop, so
a losing racer leaves no permit or charge behind. This matters for the sibling
record on baseline accounting but also here: the loser creates no state.

Backend starts are counted by the supervisor's single spawn per admitted run.
`spawn_run` at `:662-784` is called once, at `:414`, only on the fresh-entry
path. A deduplicated send never reaches it.

Existing checks, verified:

- `identical_resend_dedups_and_any_byte_difference_conflicts`
  (`tests/broca_supervisor.rs:125-161`). Two sends with the same body return
  the same ID (`:131-137`); a body with one inserted space returns
  `idempotency_conflict` (`:140-145`); a different prompt returns the same
  code (`:147-151`); after the gate opens, `backend.starts() == 1`
  (`:153-160`). The inserted-space case is the one that proves byte identity
  rather than semantic identity.
- `racing_identical_sends_converge_on_one_run_and_one_backend_start`
  (`:164-203`). Two `spawn_blocking` racers release on a `Barrier` (`:168-178`)
  and both succeed with the same run ID (`:188`); the metrics show one session
  and one live run (`:189-190`); after completion `backend.starts() == 1`
  (`:198-202`). The runtime is `multi_thread` with two workers (`:164`), so
  the racers can genuinely contend for the index lock.

`ScriptedBackend::starts` increments in `execute` at
`tests/support/broca.rs:160`, so the counter measures backend entries, not
supervisor admissions.

Both tests are ordinary `#[tokio::test]` functions in a default-harness
integration binary. CI runs `cargo test --workspace --all-targets` on
`ubuntu-latest` (`.github/workflows/ci.yml:14`, `:118`, `:126`), and the file
is Linux-gated at `tests/broca_supervisor.rs:4`, so both run in CI.

## Failure scenario

The property is about a lost linearization point, so the scenario is the one
the lock forbids.

1. Two clients send byte-identical bodies for the same session key.
2. Both compute the fingerprint and take their reservations outside the lock
   (`:331-345`). This is fine; the reservations are candidates.
3. If the dedup check and the insert were not under one lock, both could
   observe "no entry", both insert, and both call `spawn_run`. Two backend
   starts follow, two model calls are billed, and two transcripts diverge.
4. As written, the second racer takes the lock after the first inserted, sees
   `SessionEntry::Live` with an equal fingerprint, and returns the first
   racer's ID.

The conflict half is a different shape: a client that retries with a
re-serialised body (different whitespace, different key order) is refused with
`idempotency_conflict` rather than silently given a second run. That is what
`:140-145` pins.

## Timing windows and dependencies

The only window is between the reservations at `:338-345` and the lock at
`:349`, and nothing in that window is observable except the loser's transient
hold on a run slot and a retained-byte charge. The dedup itself has no window:
it is a single critical section.

The guarantee is bounded by retention. A `Live` entry persists until
`sweep_for` expires it after `terminal_retention` (`supervisor.rs:1085-1110`,
15 minutes per `config.rs:126`) or `enforce_terminal_cap` evicts it past 256
sessions (`:1005-1056`, `config.rs:122`). After either, the session has no
entry and a byte-identical resend admits a fresh run with a second backend
start. The record's `Exercised` line names this as uncovered.

## What a test must construct

The two existing tests cover the concurrent and the differing-body cases. The
uncovered case is the post-eviction resend:

1. Build a supervisor with `max_terminal_sessions` small or use
   `start_paused` time and advance past `TERMINAL_RETENTION`, as
   `every_path_returns_permits_and_charges_to_baseline` does at
   `tests/broca_supervisor.rs:1019`.
2. Send, complete, let the entry expire or be evicted, then resend the same
   bytes.
3. Assert the resend returns a different run ID and `backend.starts() == 2`.

That test would document the bound rather than the guarantee. Whether the
bound should appear in the guarantee is the open question below.

## Investigation log

### Q: Does the dedup key match the exact bytes the wire carried?

- Sources examined: `mod.rs:213-239`, `supervisor.rs:323-331`,
  `tests/broca_supervisor.rs:27-43`, `:139-145`.
- Findings: yes. `handle` parses `ctx.body` at `mod.rs:213` and then passes
  the same `&ctx.body` to `send` at `:236`. The fingerprint at
  `supervisor.rs:331` is over that slice. The inserted-space test proves the
  key is not the decoded request.
- Missing evidence: none.
- Conclusion: resolved. Byte identity is the contract and the code enforces
  it.

### Q: Is the guarantee unbounded, or bounded by retention?

- Sources examined: `supervisor.rs:356-375`, `:1048-1055`, `:1085-1110`;
  `config.rs:122`, `:126`; the catalog record's `Guarantee` and `Exercised`
  lines.
- Findings: the code bounds convergence to the lifetime of the session entry.
  After 15 minutes of terminal retention, or eviction under the 256-session
  cap, an identical resend starts a new run. The `Guarantee` sentence states
  no bound; the `Exercised` line acknowledges the gap but treats it as a test
  gap rather than a contract bound.
- Missing evidence: no test constructs the post-eviction resend.
- Conclusion: needs human input. Either the guarantee should say "within the
  retention window" or the design intends unbounded dedup, which the current
  process-local index cannot provide (`supervisor.rs:9`).
