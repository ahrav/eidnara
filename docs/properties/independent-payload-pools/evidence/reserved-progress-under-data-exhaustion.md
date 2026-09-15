# reserved-progress-under-data-exhaustion

## Discovery trigger

With ordinary blocks and descriptors exhausted, eligible reserved control and terminal frames still publish within the bounded attempt, and returns still complete (R11). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:1042`
- `crates/shm-transport/src/pool.rs:31`
- `crates/host-runtime/src/ring_transport.rs:1272`
- `packages/shm-native/src/lib.rs:1040`
- `packages/opencode-plugin/src/shared/host-client/shm-frame-channel.ts:123`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2462` proves control and terminal reservations succeed while ordinary descriptor headroom is exhausted; `crates/host-runtime/src/ring_transport.rs:3139` shows the host publisher publishing an eligible Ping and an unrelated terminal past a blocked ordinary ticket with the smallest ordinary class empty, then resuming admission order as blocks return; `crates/host-runtime/src/client.rs:7636` shows the Rust client's `Pong` publishing from the control reserve while its data frame waits on ordinary headroom; at the native addon, `a pure-header control publishes from its reserve while ordinary headroom is exhausted, and a channel-0 Request does not` in packages/shm-native/tests/mechanism.ts publishes `Pong`, `Cancel`, and `Goodbye` with every ordinary descriptor outstanding, and `the pending publication queue is bounded and a liveness reply bypasses it` in packages/opencode-plugin/src/shared/host-client/shm-frame-channel.test.ts shows the TypeScript channel's `Pong` publishing past 64 waiting data frames.

## Failure scenario

A draining peer that cannot exchange controls under data backpressure never recovers.

## Timing windows and dependencies

Ordinary exhaustion by block class and by descriptor headroom, separately and together.

## What a test must construct

Ordinary descriptors at 32 outstanding; every ordinary class empty; both at once.

Situation markers that must fire independently of the safety check:

- `pool.ordinary_class_and_descriptors_exhausted_together`
- `pool.reserved_depth_exhausted`

Check semantics: `always` - `try_reserve_in(Inventory::Control | Terminal, ..)` succeeds while `try_reserve_in(Ordinary, ..)` is `Exhausted`, until the reserved depth itself is full.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
