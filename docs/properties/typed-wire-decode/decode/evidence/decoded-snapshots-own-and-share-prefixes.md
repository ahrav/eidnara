# decoded-snapshots-own-and-share-prefixes

## Discovery trigger

KTD3 rejects RawValue, Cow, and lifetime-parameterized requests. The accepted
replacement must retain B1's shared-prefix behavior without body borrows.
The state, concurrency, and lifecycle lenses identify distinct owner drops.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Issue 438 supplies thread-mobility context; issue 524 supplies later
private-input-release context. Neither issue is execution evidence.

## Evidence trail

- `crates/daemon/src/wire.rs:25-36` owns ingress shells through Arc.
- `crates/daemon/src/wire.rs:89-114` keeps an Arc owner in SharedWireBlock.
- `crates/daemon/src/lib.rs:1803-1828,2003-2007` stores and clones owned
  TransformRequest snapshots.
- `crates/daemon/src/lib.rs:4351-4367` reattaches a projection prefix or
  clones ready-snapshot prefix handles, then appends new suffix messages.
- `crates/daemon/src/lib.rs:4369-4400` similarly retains native prefix
  Values and installs the assembled request.
- `crates/daemon/src/wire.rs:540-559` shares an existing typed shell only
  when original is absent and synthetic status matches. KTD1 removes the
  original condition, so newly decoded shells can share immediately.
- `crates/daemon/src/wire.rs:1796-1814` checks Send plus static and malformed
  decode parity. `:1749-1792` checks repeated retention and owner drop.
- `crates/daemon/src/lib.rs:22886-23004` tests native-prefix independence
  and ready-snapshot fallback after cache eviction.

The owning retention and expansion paths are default-production. The new
representation still needs fresh checks; old tests do not exercise it.

## Failure scenario

A borrowed field ties a retained snapshot to the inbound byte buffer. An
unnecessary reconstructed prefix instead breaks the accepted sharing
contract while preserving values. A missing SharedWireBlock owner could make
projection lifetime depend on a request handle that has already dropped.

These failures require different observations. Equal values do not prove Arc
sharing, and an Arc count does not prove equality of retained values.

## Timing windows and dependencies

Drop input bytes after decode while keeping decoded owners. Then drop the
original request or projection while another owner remains. Exercise cache
eviction separately from dropping a local variable.
Compare synthetic-status equality and inequality, because the latter must
create a pass-local shell instead of mutating a cached one.

HEAD Handler::handle holds RequestCtx through settlement
(`crates/daemon/src/lib.rs:12153-12170`). The output ownership property does
not claim early production release of the private body or transport lease.

## What a test must construct

Decode escaped strings and nonempty Value payloads from an owned Vec.
Keep a ready snapshot, projection, and separately cloned block owner.
Drop the Vec and verify typed payload access and serialization still work.
Reattach a nonempty prefix twice, append a suffix, and compare pointers and
values with an independently rebuilt request under the same synthetic view.

Evict projection and native cache entries while keeping a ready request.
Verify expansion uses that snapshot and retains its prefix pointers.
Drop projection and request owners in both orders; the block owner remains
usable until it is dropped. No test executes in this discovery pass.

## Investigation log

### Q: Does existing mobility evidence imply prompt input-buffer release?

- Sources examined: wire.rs:1796-1800 and daemon lib.rs:12153-12170.
- Findings: the type checks express mobility; the handler still owns ctx.
- Missing evidence: issue 524's implemented release boundary and execution.
- Conclusion: resolved with answer: no; preserve only the owned-output
  prerequisite here and hand production release to its owner.

### Q: Does the fallback test distinguish both prefix sources?

- Sources examined: lib.rs:4351-4400 and :22984-23004.
- Findings: the test explicitly expects no projection cache and checks
  snapshot-prefix pointer identity.
- Missing evidence: that sequence plus input-buffer drop on final serde.
- Conclusion: unresolved, needs the final owner-retention witness.

## Named handoff

`/testing:test-strategy` owns mobility and owner-drop checks. The issue 524
owner owns private-input release; the issue 438 owner owns joined work.
`/testing:invariant-test-review` audits B1's existing prefix assertions.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`, `cargo test -p daemon --locked
--features test-support` (1,489 tests pass; `lifecycle_cli` is platform-unsupported
on the aarch64 host). Projection shares the ingress shell whenever
the effective synthetic flag matches (`project_messages_from_state`); the
`wire.rs` sharing tests and `decode_and_projection_fit_the_declared_pool`
(every projection block points into the request's shells) pass. No input-drop
sequence was added.
