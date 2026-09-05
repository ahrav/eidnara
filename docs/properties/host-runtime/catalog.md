# Host runtime property catalog

Records for `crates/host-runtime`, merged from six source catalogs (host lifecycle, ring datapath, setup identity,
client peer, request path, runtime config) at `host@39e823037`. `index.json` is generated from this file; the
record contract is [`../METHOD.md`](../METHOD.md). Each area keeps its existing-check inventory, fault map, and
portfolio evaluation under its own directory; every per-record evidence file lives in [`evidence/`](evidence/).

## Provenance and scope

- Source catalogs were written against `crates/host-runtime` before the mandatory-ring refactor that removed the
  TCP frame channel, transport negotiation, provider recovery, and the second transport backend. Records whose
  mechanism that refactor deleted carry `Status: invalidated` with the removal named in `Invalidated:`; records
  whose mechanism moved keep their status and cite the file that owns the mechanism now.
- Every line citation inside the `Check:` field of an active record is verified against this tree, because
  those are the lines a campaign instruments; a construct the tree no longer has is named as removed rather than
  given a line. Citations in the other fields are the source catalogs' and name lines of the source files at the
  time the catalog was written; test names are the stable anchors there, and where such a citation has been
  re-verified against this tree the record says so. This tree has no `migration/waves/U3/` ledger: each record's
  `Status` and `Reachability` fields are the coverage authority for this catalog, and the wave-level
  `core`/`carried-forward`/`invalidated` classification is recorded when the U3 `property-impact.json` lands.
- `default-production` in this catalog means "on the default path of `host_runtime::run`
  with no composition-dependent or configuration-dependent state". In this tree `run` itself is
  reached only from `crates/host-runtime/examples/` and a bench; the daemon that will call it in
  production is scheduled for U4 (`../README.md`). Records whose reachability depends on a
  composed component (Broca, Synapse, the reserved route class), on the host's own `Client`, or
  on the daemon's probe and CLI paths are `test-only` here and say which caller reclassifies
  them. Whether the `run`-default records should also move until the daemon lands is bias B1 in
  [discovered-at-u3/portfolio-evaluation.md](discovered-at-u3/portfolio-evaluation.md).
- Discovery at U3 for code the source catalogs did not cover (`broca/`, `synapse/`, `harness_closure.rs`, `wire.rs`)
  is in the trailing "Discovered at U3" section; those records carry the status observed at discovery.

## Part 2a catalog: host lifecycle, generations, connections

Scope: `crates/host-runtime/src/lifecycle.rs`, `generation.rs`, `connection.rs`,
`frame_read.rs`, `panic_boundary.rs` (~6.5k lines). Boundary context from
`dispatch.rs`, `runtime.rs`, `routing.rs`, `transport_provider.rs`,
`transport_negotiation.rs`, `frame_channel.rs`, `tcp_frame_channel.rs`,
`instance.rs`, `wire.rs`, `auth.rs`, `control.rs`.

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). System
`the `host` source checkout at `d90e7811`, 2026-08-29. The five
scope files are byte-identical from `753b1c38` through `d90e7811`.

Not re-mined here: `shm_provider.rs` and `provider_recovery.rs` custody and
recovery, already cataloged as Part 1 boundary context.

## One naming collision to read past first

"Generation" means three unrelated things in this scope, and conflating them
makes every record ambiguous:

- **Payload generation** - a content-addressed on-disk directory named by the
  SHA-256 of its canonical manifest bytes. This is what `generation.rs` is about.
  It has no async code and no task.
- **Connection generation** - an in-process `GenerationCore` with a `u64` id
  minted per served frame channel. This lives in `connection.rs` and is the
  lifecycle state machine.
- **Catalog generation** - a `u64` catalog-state version, out of scope here.

Groups A through D, G, I, and J concern connection generations, as do the two
liveness records in Group K. Group F and the manifest and platform records in
Group K concern payload generations. Group E concerns the daemon incarnation,
which is a fourth, separately-fenced lifetime.

## Product context, corrected after portfolio evaluation

An earlier revision of this section claimed that because this is production code
on the default path, "every record here is reachable in a shipped configuration
unless marked otherwise". **That is false**, and the correction matters enough to
state before the records.

Verified reachability classes:

- **Default production.** The shutdown latch, the daemon incarnation and probe,
  the payload generation store, the panic boundary, connection admission, and the
  correlation watermark. These run in every shipped configuration.
- **Explicit configuration only.** Everything on the liveness probe path - the
  ping and pong records, including
  [pong-preanswer-rejected-in-every-mutex-order](#pong-preanswer-rejected-in-every-mutex-order)
  - is gated on `liveness` being configured. The default is `None`
  (`config.rs:296`), and the only `liveness: Some(..)` in the crate is inside the
  `#[cfg(test)]` module at `config.rs:664`. So **the liveness loop does not run in
  any shipped configuration in this tree**; those records are live only for an
  embedder that opts in.
- **Test-only.** The candidate grant, activation, commit, and promotion path.
  Non-TCP providers are explicitly test-only and the default config installs none
  (`transport_provider.rs:1-13`, `:157-163`). Records about two generations per
  socket and the promoted watermark are latent in the same sense Part 1's records
  were.

The honest summary is that this part mixes three reachability classes, and the
label belongs per record rather than in a blanket preamble. Deriving these labels
mechanically rather than by hand is an open bias, the same one Part 1 raised about
"reaches production".

One coverage fact from the source repository shaped the whole catalog: of
`host-runtime`'s 26 integration test binaries, that repository's CI named four,
and `tests/lifecycle.rs`, `tests/activation.rs`, and `tests/host_roundtrip.rs`
were named in no workflow. In this tree that gap does not exist: `ci.yml:118` and
`:126` run `cargo test --workspace --all-targets`, which executes every binary. See
[the-largest-lifecycle-proof-runs-in-ci](#the-largest-lifecycle-proof-runs-in-ci),
which is `Exercised: yes` here and keeps the source history as provenance.

### Repair provenance

This file survived a working-tree clean holding 38 of its 55 records. The missing
17 are the gap-closure set and were reappended from `_lenses/` verbatim, five from
`gap-g1-setup-states.md`, six from `gap-g2-frame-read.md` and six from
`gap-g3-g4-g5.md`, as Groups I, J and K below. Three mechanical adjustments were
made and no record was re-derived: evidence links were rewritten from the
lens-relative `../evidence/` to the catalog-relative `evidence/`, one
cross-reference written as `../catalog.md#...` in a lens file became an intra-file
anchor, and field paragraphs were rewrapped to about 80 columns, with content
equality checked token by token afterwards. The six Group J records additionally
carry `Status: invalidated` in place of `active`, because
`ed487e11` removed `frame_read.rs` from the module tree; the group preamble states
what that leaves outstanding. The index table already held all 55 rows in this
order, so no row was added, and none of the 38 surviving records was touched.

## Index

| Slug | Type | Confidence |
| --- | --- | --- |
| [generation-id-strictly-increases-and-is-never-reused](#generation-id-strictly-increases-and-is-never-reused) | safety | high |
| [at-most-one-registered-generation-per-connection](#at-most-one-registered-generation-per-connection) | safety | high |
| [close-disposition-is-a-total-function-of-the-read-exit-cause](#close-disposition-is-a-total-function-of-the-read-exit-cause) | safety | high |
| [retirement-discards-only-through-the-discard-token](#retirement-discards-only-through-the-discard-token) | safety | high |
| [a-retired-generation-emits-nothing-and-mutates-nothing](#a-retired-generation-emits-nothing-and-mutates-nothing) | safety | high |
| [generation-registry-entry-released-on-every-connection-exit](#generation-registry-entry-released-on-every-connection-exit) | safety | high |
| [disconnect-releases-every-resource-keyed-to-the-connection](#disconnect-releases-every-resource-keyed-to-the-connection) | safety | medium |
| [request-correlation-strictly-increases-per-generation](#request-correlation-strictly-increases-per-generation) | safety | high |
| [promoted-generation-refuses-the-setup-correlations](#promoted-generation-refuses-the-setup-correlations) | safety | high |
| [ping-and-consumer-correlations-cannot-cross-settle](#ping-and-consumer-correlations-cannot-cross-settle) | safety | high |
| [pong-preanswer-rejected-in-every-mutex-order](#pong-preanswer-rejected-in-every-mutex-order) | safety | high |
| [host-ping-correlation-exhaustion-retires-the-generation](#host-ping-correlation-exhaustion-retires-the-generation) | safety | high |
| [no-task-outlives-the-generation-it-serves](#no-task-outlives-the-generation-it-serves) | safety | high |
| [the-writer-task-is-abortable-through-a-stated-owner](#the-writer-task-is-abortable-through-a-stated-owner) | safety | high |
| [draining-rendezvous-is-released-or-the-loss-is-declared](#draining-rendezvous-is-released-or-the-loss-is-declared) | liveness | high |
| [no-generation-registers-after-the-drain-snapshot](#no-generation-registers-after-the-drain-snapshot) | safety | high |
| [read-task-quiescence-implies-no-further-registration](#read-task-quiescence-implies-no-further-registration) | safety | high |
| [a-cancelled-emission-releases-every-permit-it-held](#a-cancelled-emission-releases-every-permit-it-held) | safety | high |
| [no-writer-hook-panic-poisons-a-generation-lock](#no-writer-hook-panic-poisons-a-generation-lock) | safety | high |
| [shutdown-commits-exactly-once-on-write-ack](#shutdown-commits-exactly-once-on-write-ack) | safety | high |
| [admission-freeze-precedes-the-shutdown-commit](#admission-freeze-precedes-the-shutdown-commit) | safety | high |
| [shutdown-commit-effects-are-all-or-nothing](#shutdown-commit-effects-are-all-or-nothing) | safety | medium |
| [latch-wake-cannot-be-lost](#latch-wake-cannot-be-lost) | liveness | high |
| [probe-never-reports-stopped-while-either-fence-is-held](#probe-never-reports-stopped-while-either-fence-is-held) | safety | high |
| [stopping-precedes-unpublication-on-every-path](#stopping-precedes-unpublication-on-every-path) | safety | high |
| [phase-evidence-outlives-a-long-phase](#phase-evidence-outlives-a-long-phase) | safety | high |
| [clock-anomalies-do-not-invalidate-live-evidence](#clock-anomalies-do-not-invalidate-live-evidence) | safety | high |
| [legacy-incumbent-classification-needs-an-unforgeable-witness](#legacy-incumbent-classification-needs-an-unforgeable-witness) | safety | high |
| [an-observed-wedge-cause-reaches-the-operator](#an-observed-wedge-cause-reaches-the-operator) | safety | high |
| [current-profile-never-names-an-unvalidatable-generation](#current-profile-never-names-an-unvalidatable-generation) | safety | high |
| [validation-and-enumeration-address-one-directory-object](#validation-and-enumeration-address-one-directory-object) | safety | high |
| [an-undecidable-quarantine-witness-fails-closed](#an-undecidable-quarantine-witness-fails-closed) | safety | high |
| [persisted-state-quarantine-caps-agree](#persisted-state-quarantine-caps-agree) | safety | high |
| [every-declared-cli-reason-id-has-a-producer](#every-declared-cli-reason-id-has-a-producer) | safety | high |
| [every-callback-invocation-is-inside-the-redaction-guard](#every-callback-invocation-is-inside-the-redaction-guard) | safety | high |
| [the-panic-hook-cannot-itself-fail](#the-panic-hook-cannot-itself-fail) | safety | medium |
| [authentication-and-capacity-rejections-are-observable](#authentication-and-capacity-rejections-are-observable) | safety | high |
| [the-largest-lifecycle-proof-runs-in-ci](#the-largest-lifecycle-proof-runs-in-ci) | reachability | high |
| [negotiation-precedes-every-gated-frame-kind](#negotiation-precedes-every-gated-frame-kind) | safety | high |
| [setup-selection-is-sticky-for-the-generation](#setup-selection-is-sticky-for-the-generation) | safety | high |
| [setup-readiness-is-decided-by-one-predicate](#setup-readiness-is-decided-by-one-predicate) | safety | high |
| [a-setup-pong-is-required-and-forbidden-in-the-same-window](#a-setup-pong-is-required-and-forbidden-in-the-same-window) | reachability | high |
| [fallback-reason-precedence-survives-a-silent-preflight](#fallback-reason-precedence-survives-a-silent-preflight) | safety | high |
| [cancellation-preempts-every-bounded-frame-read](#cancellation-preempts-every-bounded-frame-read) | safety | high |
| [a-body-read-consumes-exactly-the-declared-frame-boundary](#a-body-read-consumes-exactly-the-declared-frame-boundary) | safety | high |
| [a-zero-length-read-ends-the-read-instead-of-looping](#a-zero-length-read-ends-the-read-instead-of-looping) | safety | high |
| [no-framed-read-resumes-after-a-read-stop](#no-framed-read-resumes-after-a-read-stop) | safety | high |
| [oversize-control-drain-work-is-bounded-without-ingress-budget](#oversize-control-drain-work-is-bounded-without-ingress-budget) | safety | high |
| [the-client-body-budget-refusal-drain-is-never-entered](#the-client-body-budget-refusal-drain-is-never-entered) | reachability | high |
| [a-timely-pong-sustains-the-generation-within-a-bounded-round](#a-timely-pong-sustains-the-generation-within-a-bounded-round) | liveness | high |
| [slow-egress-alone-does-not-retire-a-probed-generation](#slow-egress-alone-does-not-retire-a-probed-generation) | safety | high |
| [manifest-canonical-bytes-and-digest-are-pinned-by-a-full-golden-vector](#manifest-canonical-bytes-and-digest-are-pinned-by-a-full-golden-vector) | safety | high |
| [a-declaration-order-change-cannot-orphan-a-retained-generation](#a-declaration-order-change-cannot-orphan-a-retained-generation) | safety | high |
| [the-atomic-directory-exchange-is-atomic-on-every-supported-platform](#the-atomic-directory-exchange-is-atomic-on-every-supported-platform) | safety | high |
| [an-occupied-rename-target-is-never-replaced-on-the-portable-path](#an-occupied-rename-target-is-never-replaced-on-the-portable-path) | safety | high |

---

## Group A: connection generation lifecycle and retirement

### generation-id-strictly-increases-and-is-never-reused

Type: safety
Reachability: default-production - the generation counter is seeded once per
incarnation (`gen_counter: AtomicU64::new(1)` at `crates/host-runtime/src/runtime.rs:788`, re-verified) and every accepted
connection mints from it (`shared.gen_counter.fetch_add(1, ..)` in `new_generation`,
`crates/host-runtime/src/connection.rs:219`, re-verified).
Status: active
Exercised: not yet - every test hand-builds `id: 1`.
Guarantee: Within one host incarnation every minted connection generation has a
strictly larger id than all previous ones, and no two live `GenerationCore`
values share an id.
Check: `always` - instrument minting; the sequence is strictly increasing, and
`shared.connections` never contains an id equal to a previously-removed one. Id
uniqueness is what makes the route registry's ownership test and the connection
registry's keying sound, so it must hold at every evaluation.
Fault/timing angle: refined after portfolio evaluation. Concurrent minting is
**not** the meaningful fault: the allocator is a single sequentially-consistent
fetch-and-add, so interleaving cannot produce a duplicate. The only way uniqueness
fails is **wraparound** at the counter's maximum, which is unchecked. Seed the
counter near its maximum rather than running a concurrency campaign.
Required faults and enabling state: a counter seeded near its maximum. The
two-generations-per-socket case needs a candidate promotion, which is a test-only
path.
Confidence: high - [evidence](evidence/generation-id-strictly-increases-and-is-never-reused.md). verified: the counter is initialized to 1 at `runtime.rs:898`
and `gen_counter` has exactly two references.
Existing check: none. `routing.rs:535`
`concurrent_generations_never_share_a_live_channel` asserts channel exclusivity
between two hand-built ids, not id minting.
Impact: a duplicate id would let one generation's close finalize another's route,
or let a stale generation's frames settle a live correlation.
Open questions: None.

### at-most-one-registered-generation-per-connection

Type: safety
Reachability: default-production - every accepted setup socket runs `run_connection` unconditionally (`crates/host-runtime/src/runtime.rs:893`, re-verified); no configuration gates this path.
Status: active
Exercised: not yet - no test lands a drain between setup completion and the registration at `connection.rs:260` and asserts that no insert occurs.
Guarantee: One accepted socket never has two generations registered in `shared.connections` simultaneously, and a generation minted while draining or after the shutdown token is cancelled is never registered.
Check: `always` - for each connection task, at most one of its minted ids is in the registry at any observation point; and when `draining` is true or `shutdown.is_cancelled()` at the registration check (`connection.rs:256`), no insert occurs at `:260`.
Fault/timing angle: `run_connection` constructs one `GenerationCore` and the registry has a single insertion at `connection.rs:260`, taken under the `connections` mutex that the shutdown snapshot also holds; that shared lock is what makes the drain-time interleaving safe. The bootstrap-to-promoted transfer window the source catalog described, with its neither-registered gap, was removed with transport negotiation (`ed487e11`) and is recorded in the evidence file as history.
Required faults and enabling state: a shutdown or signal drain landing between setup completion and the registration check at `connection.rs:256`.
Confidence: high - [evidence](evidence/at-most-one-registered-generation-per-connection.md). The single insertion and its lock are read directly at HEAD; the exclusion is structural now that only one generation exists per connection.
Existing check: none at HEAD. The source catalog's `shutdown_during_candidate_setup_reaps_both_channels` was removed with the candidate-handoff path (`ed487e11`); `tests/lifecycle.rs` shutdown cases execute `run_connection` under drain incidentally and assert nothing about registration.
Impact: shutdown, route ownership, and Goodbye delivery all enumerate the registry assuming one live owner per socket; a generation registered after the drain snapshot is never told to stop.
Open questions:
- Should a generation discarded by the `:256-258` early return receive a connection Goodbye? Today it is cancelled and its writer discarded without one. (needs human input)

### close-disposition-is-a-total-function-of-the-read-exit-cause

Type: safety
Reachability: default-production - the `ReadExit` match runs on every
connection teardown (`crates/host-runtime/src/connection.rs:293-319`), reached
unconditionally from `run_connection` (`crates/host-runtime/src/runtime.rs:1043`).
Status: active
Exercised: partial - the current three-arm disposition is proven only by
`tests/lifecycle.rs` and `tests/transport_negotiation.rs`; the first runs in CI in
this tree (`ci.yml:118`, `:126`, `cargo test --workspace --all-targets`), the second was removed by the mandatory-ring refactor.
Guarantee: For every read-exit cause, the frames emitted after the close decision
are exactly the declared set for that cause.
Check: `always` - for each cause, assert the emitted sequence: nothing for a peer
exit, the drain for a host-cancelled exit, exactly one authoritative terminal for
an oversize-control drain failure. Adding a cause without declaring its
disposition should fail to compile.
Fault/timing angle: a peer-driven close racing queued off-reader emissions. The
`ReadExit::HostCancelled if !gen.token.is_cancelled()` guard means a *new*
cancellation source silently falls into the silent-close arm.
Required faults and enabling state: each of the eleven read-exit sites, with
queued emissions in flight.
Confidence: high - [evidence](evidence/close-disposition-is-a-total-function-of-the-read-exit-cause.md). this property is derived from an incident chain, not a
hypothesis. Five successive commits corrected one decision: cancel without
discard still flushed queued frames; keying on the host-wide `draining` flag gave
terminals to a peer that sent a corrupt frame during shutdown; an inherited
cancellation is a retirement rather than a drain; and a bare keep-queue marker let
the whole queue flush instead of the one promised terminal.
Existing check: partial, per above. Status unaudited.
Impact: this is the silent-close rule the wire protocol requires. Each of the five
iterations shipped a wrong disposition.
Open questions:
- Should the disposition be encoded so a new cause cannot compile without a
  declared disposition? That is a design change, not a test. (needs human input)

### retirement-discards-only-through-the-discard-token

Type: safety
Reachability: default-production - `discard` and the `retired` token are the
shipped writer's own gates (`crates/host-runtime/src/frame_channel.rs:701`, `:727`),
and retirement runs on every connection exit
(`crates/host-runtime/src/connection.rs:307`, `:317`).
Status: active
Exercised: not yet - nothing exercises admission after cancel.
Guarantee: After a generation is retired with both `token.cancel()` and
`writer.discard()`, no byte of any frame admitted after the cancel reaches the
socket.
Check: `always` - cancel the token and call `writer.discard()`, then have a producer that already passed its `is_cancelled` precheck call send; assert the bytes never appear on the peer socket. Both halves of the precondition are required: `send_ticket_before` gates on the writer's own `retired` token (`frame_channel.rs:640-653`), and the endpoint select services a ready queue before `root.cancelled()` (`ring_transport.rs:438-470`), so cancelling the generation token alone may still publish the newly queued frame. Separately assert `token.cancel()` alone does *not* stop queued frames, because the drain paths depend on that.
Fault/timing angle: `send_ticket_before` gates on `retired` only, not on the
generation token or `discard` (`frame_channel.rs:812-825`). So the guarantee is
enforced downstream by the writer's biased discard arm, not by admission: a
producer can be admitted after cancel, and it is the writer that must drop it.
Required faults and enabling state: a producer suspended between its
`is_cancelled` precheck and its send, with the cancel landing in between.
Confidence: high - [evidence](evidence/retirement-discards-only-through-the-discard-token.md). every gate read directly; `discard` being a separate token
from `retired` is the load-bearing detail.
Existing check: partial - `tcp_frame_channel.rs:1130` and `:1062` cover
writer-initiated retirement only.
Impact: a frame emitted after the close decision violates the silent-close rule.
Open questions: None.

### a-retired-generation-emits-nothing-and-mutates-nothing

Type: safety
Reachability: default-production - cancel-then-discard is the default
retirement path (`crates/host-runtime/src/connection.rs:315-318`, and `:350-354` for
the unregistered case).
Status: active
Exercised: partial - one shape covered.
Guarantee: Once a generation's token is cancelled, no *new* frame is admitted or
charged on its behalf, and once its writer is additionally discarded, no already
queued frame reaches the socket.
Check: `always` - after cancel, every charge attempt returns none and every emit
returns without enqueueing; after discard, the writer breaks without publishing
any remaining queued frame. The guarantee was corrected after portfolio
evaluation: an earlier revision said cancellation alone stopped queued frames,
which contradicts
[retirement-discards-only-through-the-discard-token](#retirement-discards-only-through-the-discard-token)
and is false. Cancellation stops *admission*; discard stops *queued bytes*. The
drain paths depend on exactly that split.
Fault/timing angle: the interesting interleaving is a frame already queued but
not yet begun; the biased discard arm decides it. Peer-driven exits cancel and
discard together, which is what makes a corrupt-frame close silent even during
shutdown.
Required faults and enabling state: an in-flight off-reader emission concurrent
with a peer-driven close.
Confidence: high - [evidence](evidence/a-retired-generation-emits-nothing-and-mutates-nothing.md). every emit path routes through a charge helper or an explicit
`is_cancelled` pair.
Existing check: `tests/transport_negotiation.rs:907` covers one shape;
`connection.rs:1598-1606` pins the positive fence, not the negative case.
Impact: the fail-closed property the whole retirement design rests on.
Open questions: None.

### generation-registry-entry-released-on-every-connection-exit

Type: safety
Reachability: default-production - every accepted setup socket runs
`run_connection` unconditionally (`crates/host-runtime/src/runtime.rs:1043`); no
configuration gates this path.
Status: active
Exercised: not yet - needs an induced panic or abort between insert and removal.
Guarantee: A generation inserted into the registry is removed before its
connection task can finish or die.
Check: `always` - for every path out of `serve_generation` after the insert, the registry no longer contains that id and no `Arc<GenerationCore>` for it is retained: the ordinary return, the panic path, and the abort path, where the connection task's future is dropped by the forced shutdown's `abort_all` between insert and removal and the removal must run from a drop guard rather than from a return.
Fault/timing angle: `close_generation` is the only remover and runs after
`read_tasks.wait()` and after the `shutdown_complete` rendezvous. Any unwind
before that line leaks the entry; the leaked `Arc<GenerationCore>` then keeps the
writer sender and pending map alive for host lifetime, and the shutdown sequence
iterates a generation whose task is gone.
Required faults and enabling state: a panic in the read loop, control handling,
grant, or close-route decision; or an abort while between insert and removal.
Confidence: high - [evidence](evidence/generation-registry-entry-released-on-every-connection-exit.md). the single-remover structure is directly readable and nothing
guards the interval.
Existing check: none.
Impact: a permanently leaked registry entry that shutdown will wait on.
Open questions: None.

### disconnect-releases-every-resource-keyed-to-the-connection

Type: safety
Reachability: default-production - the draining early return is on the default connection path (`crates/host-runtime/src/connection.rs:256-258`, re-verified): under the connections lock, if `draining` is set or the shutdown token is cancelled, `discard_unregistered_generation` (`:319-323`) cancels `read_cancel`, cancels the root `token`, and discards the writer before returning. The promoted-versus-unpromoted asymmetry the source catalog described was removed with transport negotiation; one arm remains.
Status: active
Exercised: not yet - no test lands shutdown in the post-commit,
pre-registration window.
Guarantee: When a connection ends, every permit, charge, map entry, task, and
cancellation root created for it is released, including on the early-return path
taken while the host is draining.
Check: `always` - after the connection task returns, both permits are released, every setup-phase byte and ring charge has returned the accounting snapshot to its pre-connection baseline, the registry holds neither generation id, every owned route is finalized, no connection-owned task set retains a task and no reference to the `GenerationCore` survives, and the generation's root token is cancelled, on the normal path and on the draining early return at `connection.rs:256-258`. Permits and registry entries alone are not enough, because the early return can leave a setup-phase charge or a detached task live while both permits are back.
Fault/timing angle: the window is a committed shutdown landing after setup completes and before the generation is registered under the connections lock. The early return at `connection.rs:256-258` handles it with one arm that cancels the root directly, so release no longer depends on the abort-on-drop handle. What is unverified is the rest of the guarantee on that arm: whether the setup-phase permits and charges taken before `:256` are released by the discard or only by task exit.
Required faults and enabling state: a committed shutdown landing between setup completion and the generation's registration at `connection.rs:260`.
Confidence: medium - [evidence](evidence/disconnect-releases-every-resource-keyed-to-the-connection.md). The early return and the discard are read directly at HEAD; the source catalog's candidate-specific fault is recorded in the evidence file as history. What is not established is which permits the early return releases.
Existing check: `tests/lifecycle.rs` covers shutdown before a connection registers; nothing lands shutdown in the post-setup, pre-registration window. The source catalog's `tests/transport_negotiation.rs` reference was removed with that file.
Impact: a connection retired during shutdown that leaks a permit or charge until the host exits.
Open questions:
- Which of the setup-phase permits and charges does the `:256-258` early return release directly, and which only through task exit? (needs human input)

---

## Group B: correlation and probe discipline

### request-correlation-strictly-increases-per-generation

Type: safety
Reachability: default-production - the consumer watermark is evaluated in the
read loop of every connection (`crates/host-runtime/src/connection.rs:381`), reached
unconditionally from `run_connection` (`crates/host-runtime/src/runtime.rs:1043`).
Status: active
Exercised: partial - the watermark is covered; the pending insert is not.
Guarantee: Within one generation no consumer request correlation is accepted
twice, so a pending-request key can never collide with a live entry.
Check: `always` - for every accepted request frame and every rejection carrying a
header correlation, the correlation exceeded the watermark before the watermark
advanced; and the pending-map insert never returns an existing entry.
Fault/timing angle: none for the read-loop half, which is one task. The insert
silently overwrites, so correctness rests entirely on the watermark one function
away, with nothing asserting the insert returned empty.
Required faults and enabling state: a repeated or lower correlation; and for the
second clause, a mutation weakening the watermark while leaving the insert alone.
Confidence: high - [evidence](evidence/request-correlation-strictly-increases-per-generation.md). both sites read; the check precedes the channel-0 split, so it
covers control and routed requests alike.
Existing check: `tests/dispatch.rs:211`
`a_non_increasing_correlation_closes_the_generation_before_dispatch` covers the
watermark. Nothing pins the insert return value. Status unaudited.
Impact: correlation reuse would cross-settle two requests.
Open questions: None.

### promoted-generation-refuses-the-setup-correlations

Type: safety
Reachability: default-production when live - at `ed487e11^`
`HostConfig::default` injected the candidate provider itself
(`crates/host-runtime/src/config.rs:297-303`), so reaching a promoted generation
needed no explicit host config, only a client that drove the candidate exchange;
the read-loop watermark it seeded is on the unconditional path
(`crates/host-runtime/src/connection.rs:381`, reached from
`crates/host-runtime/src/runtime.rs:1043`). Superseded rather than live: the
ring-transport refactor removed the promotion and candidate-handoff path, so no
promoted generation exists at HEAD and the subject of this record is unreachable
by any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - the pre-commit case is covered; the post-promotion case is
not.
Guarantee: On a promoted candidate, correlations 1 and 2 are permanently spent, so
a client cannot re-drive setup or collide with the activation and commit
correlations from application traffic.
Check: `always` - a promoted generation's initial watermark equals the commit
correlation, and any request at or below it closes the generation before dispatch.
Fault/timing angle: the frames that make this matter are pipelined ahead of the
commit response. The setup driver deliberately stops polling the receiver while
awaiting write completion, so those frames are first observed by the promoted read
loop, which is exactly where this watermark applies.
Required faults and enabling state: a client that pipelines a correlation-1 or
correlation-2 request behind its commit request.
Confidence: high - [evidence](evidence/promoted-generation-refuses-the-setup-correlations.md). the seed value and the comparison were both read.
Existing check: none. The cited pre-promotion test,
`application_frame_before_promotion_fails_setup_instead_of_dispatching` at
`crates/host-runtime/tests/transport_negotiation.rs:1268`, lived in a file `ed487e11`
deleted, so that citation resolves only at the commit the lens pass read.
Impact: a single wrong constant silently permits replay of the activation and
commit correlations. `ed487e11 refactor(host): make ring transport mandatory`
removed the promotion path and the `ConnectionSetup::initial_watermark` field it
seeded, so `crates/host-runtime/src/connection.rs:381` now seeds the watermark to 0
unconditionally, correlations 1 and 2 are not spent, and the guarantee is vacuous
rather than violated.
Open questions:
- `crates/host-runtime/src/connection.rs:379-380` still tells a reader that "A
  promoted candidate starts at 2 so application correlations begin at 3
  (§7.7.4)" immediately above the unconditional `= 0` seed, and two further
  comments describe the removed mechanism as present: `:250-251` says
  `serve_generation` "Returns the candidate handoff", though it returns unit, and
  `:333-335` describes taking a handoff from "the promotion slot". All three are
  live misleading comments in shipped code. Whether §7.7.4 still reserves
  correlations 1 and 2 for a setup exchange the ring transport no longer
  performs, and therefore whether the comments or the constant are the defect, is
  a protocol-ownership question (needs human input). Note also that
  `provider_active()` already seeded `initial_watermark: 0` at `ed487e11^`
  (`crates/host-runtime/src/connection.rs:804`) while its own doc comment claimed 3,
  so the comment-versus-constant disagreement predates the refactor.

### ping-and-consumer-correlations-cannot-cross-settle

Type: safety
Reachability: explicit-config-only - the liveness loop is spawned only when a
policy is configured (`crates/host-runtime/src/connection.rs:279`),
`HostConfig::default` leaves `liveness: None`
(`crates/host-runtime/src/config.rs:236`, re-verified), and no in-tree caller of `run`, in
`crates/host-runtime/examples/` or the bench, sets `liveness`; the daemon that will build the
production config is scheduled for U4 (`docs/properties/README.md:52`).
Status: active
Exercised: yes - `tests/lifecycle.rs:468`
`ping_and_consumer_correlations_do_not_cross_settle` constructs a numerically
equal consumer correlation. That file runs in CI in this tree (`ci.yml:118`, `:126`, `cargo test --workspace --all-targets`).
Guarantee: Host-originated ping correlations and consumer-originated correlations
never settle each other even when numerically equal.
Check: `always` - pong handling reads only the pings map; consumer terminals key
only the pending map by channel, epoch, and correlation.
Fault/timing angle: none; the separation is structural, two maps.
Required faults and enabling state: a consumer correlation numerically equal to a
live ping correlation.
Confidence: high - [evidence](evidence/ping-and-consumer-correlations-cannot-cross-settle.md). the two maps and both lookup sites read directly.
Existing check: as above. Status unaudited.
Impact: a cross-settle would let a client's request terminal clear a liveness
probe, defeating read-liveness detection.
Open questions: None.

### pong-preanswer-rejected-in-every-mutex-order

Type: safety
Reachability: explicit-config-only - the liveness loop is spawned only when a
policy is configured (`crates/host-runtime/src/connection.rs:279`),
`HostConfig::default` leaves `liveness: None`
(`crates/host-runtime/src/config.rs:236`, re-verified), and no in-tree caller of `run`, in
`crates/host-runtime/examples/` or the bench, sets `liveness`; the daemon that will build the
production config is scheduled for U4 (`docs/properties/README.md:52`).
Status: active
Exercised: not yet - no test drives the two mutex orderings.
Guarantee: A pong observed strictly before its ping's bytes were written is never
accepted as an answer, regardless of which party wins the pings mutex.
Check: `always` - for every probe removed by the read loop, the read-loop
observation instant is at or after the probe's recorded write-completion instant.
The type's own doc states this unconditionally, so an accepted pre-answer is a
violation rather than a tolerated case.
Fault/timing angle: verified by direct read. The read loop samples
`now = Instant::now()` at `connection.rs:504` **before** acquiring the pings lock
at `:505`. If the writer's completion hook wins the lock first, it sets the
probe's `sent` to the completion instant; the read loop then takes the
completion-recorded arm and evaluates only
`now.duration_since(probe.sent) < pong_deadline` at `:519-521`. With
`completed_at > now`, tokio's `Instant::duration_since` saturates to zero rather
than panicking, so the comparison passes and the probe is removed. The
`answered_at >= completed_at` guard exists only in the hook's branch, not here.
Required faults and enabling state: a peer emitting a pong for a correlation
before the ping bytes complete (sequential correlations make this cheap), plus
writer-task preemption so the hook lands after the peer's pong is read, plus a
configured liveness policy.
Confidence: high - [evidence](evidence/pong-preanswer-rejected-in-every-mutex-order.md). both branches read directly at HEAD, and the saturating
subtraction is documented tokio behaviour.
Existing check: none. `tests/lifecycle.rs:468` covers an *unmatched* pong, not a
matched pre-answer.
Impact: defeats the pre-answer defence the probe design exists to provide. A peer
that never reads its socket can keep a generation alive by answering pings it
never received, which is precisely what read-liveness is supposed to detect.
Open questions:
- Is the absence of the guard on this side an oversight, or is the design comment
  intended to cover it? The comment argues a peer that received bytes but answered
  without reading is indistinguishable from a real answer; that does not cover
  this case, where the pong is accepted before the bytes existed. (needs human
  input)

### host-ping-correlation-exhaustion-retires-the-generation

Type: safety
Reachability: explicit-config-only - the egress half that carries the gap, the
host's ping allocator, runs only under a configured policy
(`crates/host-runtime/src/connection.rs:279`), and `HostConfig::default` leaves
`liveness: None` (`crates/host-runtime/src/config.rs:236`, re-verified), which no in-tree caller of
`run` overrides; the daemon that will build the production config is scheduled for U4
(`docs/properties/README.md:52`). The enforced ingress
watermark half is default-production (`crates/host-runtime/src/connection.rs:381`).
Status: active
Exercised: not yet - practically unreachable by exhaustion.
Guarantee: A correlation is never reused or wrapped; at exhaustion the sender
retires the generation instead.
Check: `always` - ingress holds by the strict watermark. Egress: seed `next_ping_corr` (`connection.rs:75`, allocated by `fetch_add` at `:780`) at `u64::MAX` before the next tick and assert that the generation retires without writing a Ping carrying a reused or wrapped correlation; an allocator that stops incrementing, returns an error, or saturates without retiring fails the check, because the guarantee requires both halves, no reuse and retirement at exhaustion. Predicted to fail at HEAD: `fetch_add` wraps and no retirement path exists.
Fault/timing angle: none. The ping counter uses an unbounded `fetch_add`, so the
2^64-th ping wraps to correlation 0, and a ping with correlation 0 violates the
frame-shape rule the host's own client-side matching enforces.
Required faults and enabling state: none constructible; the record exists because
this is a documented MUST with no implementing code, which the wire protocol
explicitly calls out as a defect to be replaced with checked exhaustion.
Confidence: high - [evidence](evidence/host-ping-correlation-exhaustion-retires-the-generation.md). the counter and the absence of a bound were read directly.
Existing check: none.
Impact: negligible operationally, material contractually: the ingress half of
this rule is enforced and the egress half is not.
Open questions: None.

---

## Group C: task ownership, cancellation, shutdown

### no-task-outlives-the-generation-it-serves

Type: safety
Reachability: default-production - the spawn sites enumerated are on the
default connection and dispatch path, reached from `run_connection`
(`crates/host-runtime/src/runtime.rs:1043`).
Status: active
Exercised: not yet - no test enumerates the spawn sites reachable from a generation and asserts each is bound to the generation's token or tracker; the spawn inventory in the record is a manual read.
Guarantee: Every task holding a generation reference is a member of a set that
some shutdown path closes and waits on.
Check: `always` - enumerate spawn sites reachable from a generation; each is in
the generation's read-task set, or in the host tracker with a retained abort
handle, or owned by an abort-on-drop handle whose owner is itself tracked. Assert
the enumeration is exhaustive.
Fault/timing angle: `dispatch.rs:747` is the one bare `tokio::spawn` in the
connection path, verified. It is absent from the read-task set, so neither the
per-generation wait nor the shutdown wait covers it, and absent from the abort
handles, so the forced sweep cannot reach it. It self-bounds on one admission
deadline while holding a generation reference, so it can cancel a token for a
generation already removed from the registry.
Required faults and enabling state: an authenticated shutdown whose response is
admitted to the writer queue; the interesting case is a second shutdown on a
generation the first watchdog still holds.
Confidence: high - [evidence](evidence/no-task-outlives-the-generation-it-serves.md). verified by enumerating every spawn in the three files; this is
the only untracked one.
Existing check: none.
Impact: harmless as written, which is exactly why it should be pinned: the
shutdown sequence's completeness argument rests on the enumeration being total.
Open questions:
- Should the watchdog be tracked? That makes its lifetime a stated part of the
  generation's at the cost of one abort handle.

### the-writer-task-is-abortable-through-a-stated-owner

Type: safety
Reachability: default-production - every connection builds its writer through
the shipped frame channel (`crates/host-runtime/src/connection.rs:147-148`) and
spawns the endpoint task into the host tracker (`:190`); the forced sweep is
part of the ordinary shutdown path.
Status: active
Exercised: not yet for the forced path.
Guarantee: Forced shutdown terminates every connection writer task.
Check: `always` - park a writer on a stalled peer, run the forced path, and assert the host tracker's wait completes, that the endpoint's own completion signal fires (`done_tx` at `ring_transport.rs:229`, awaited by the tracked proxy at `:284`) and the endpoint thread has exited, and that the ring's permits and charges are released; tracker quiescence alone proves only that tracked handles are gone, which an untracked writer or an aborted proxy also satisfies.
Fault/timing angle: the writer is spawned with the tracker's own `spawn`, not the
tracked helper, so no abort handle is registered and the forced sweep cannot reach
it directly. It *is* tracked, so the wait does cover it. Termination therefore
depends on a chain: the sweep aborts the connection task, which drops the
abort-on-drop handle, which aborts the writer. Break either link and forced
shutdown waits forever on a stalled writer while holding the instance lock.
Required faults and enabling state: a peer that authenticates then stops reading,
queued frames, and a drain that misses its deadline so the forced branch runs.
Confidence: high - [evidence](evidence/the-writer-task-is-abortable-through-a-stated-owner.md). the spawn-helper difference at this one site is unambiguous and
the abort chain is the only thing closing the gap.
Existing check: none for the forced path.
Impact: the instance lock is held until the tracker wait completes, so a surviving
writer blocks a successor incarnation.
Open questions:
- Is the omission deliberate, so the writer survives the sweep long enough to
  flush terminals and Goodbye? If so the compensating chain belongs in a comment.

### draining-rendezvous-is-released-or-the-loss-is-declared

Type: liveness
Reachability: default-production - the rendezvous await is on the default
teardown path whenever draining is set
(`crates/host-runtime/src/connection.rs:328-330`).
Status: active
Exercised: not yet - no test drives a generation into the rendezvous with draining set, exhausts the drain window during route-settle, and asserts that the sequence returns inside the forced-exit bound with a non-graceful result.
Guarantee: A generation that observes draining while tearing down proceeds past the shutdown rendezvous, or the host declares that it did not, within the forced-exit bound `run` already carries.
Check: `always-or-unreached` - within `shutdown_deadline + 3 * lifecycle_callback_deadline`
plus up to `2 * lifecycle_callback_deadline` for each of the two `force_close_all_routes` calls (the tracker wait at `dispatch.rs:1299` and then `run_route_gone` at `:1162-1166`, both under the configured deadline), so `shutdown_deadline + 7 * lifecycle_callback_deadline` in total, after the shutdown token cancels, the shutdown sequence has
returned, no task is parked at the rendezvous, and if the drain timed out the
return value is non-graceful and names it. The bound is the forced-exit figure in
[rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline](#rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline);
a sequence that has not returned inside it is a lost rendezvous and the check
fails rather than waiting. `always-or-unreached` because the rendezvous is
reached only when draining was true at that instant; when the branch is skipped
the obligation does not exist and the check must not fail.
Fault/timing angle: the rendezvous await has no timeout and no competing arm. If
the drain times out during the route-settle loop, the cancelling line is never
reached and the parked task survives only because the forced sweep happens to hold
its abort handle. That handle exists because the connection task uses the tracked
spawn helper; had it used the lifecycle helper, the host would hang holding the
instance lock.
Required faults and enabling state: draining true, a generation whose read loop
exits inside the drain window, and a route-settle phase slow enough to consume the
shutdown deadline.
Confidence: high - [evidence](evidence/draining-rendezvous-is-released-or-the-loss-is-declared.md). On the mechanism; medium on severity, since abort does rescue it,
but by abort, and no signal distinguishes drained from aborted mid-rendezvous.
Existing check: none.
Impact: the graceful-close guarantee degrades to task abort, two timeouts deep.
Open questions:
- Should the rendezvous carry its own timeout, or is "escaped only by the forced
  sweep" the intended contract? If the latter, the connection task's choice of
  spawn helper is a correctness requirement rather than a style choice.

### no-generation-registers-after-the-drain-snapshot

Type: safety
Reachability: default-production - the draining check and the registry insert
share the connections lock on every accepted connection
(`crates/host-runtime/src/connection.rs:267-278`).
Status: active
Exercised: not yet - no test races a registration against the drain snapshot and asserts the late generation is either in the snapshot or cancelled by the read-loop wait.
Guarantee: The shutdown sequence's one-shot registry snapshot contains every
generation that will ever wait on the shutdown rendezvous.
Check: `always` - every inserted generation either appears in the snapshot or
completed its close before the snapshot was taken.
Fault/timing angle: the argument rests on two orderings. The draining flag is
stored with sequential consistency strictly before the snapshot, with an await
between, so any insert winning the connections lock afterwards reads true and
bails. And the check and insert share the snapshot's lock scope. Both hold as
written; neither is asserted. That the second draining writer is a *writer task*
rather than the shutdown path is what makes this non-obvious.
Required faults and enabling state: a socket accepted and authenticated between
the draining store and the snapshot; requires a multi-thread runtime to be
interesting.
Confidence: high - [evidence](evidence/no-generation-registers-after-the-drain-snapshot.md). That it holds, high that it is unchecked.
Existing check: none. The in-code comment documents only the token half of the
window, not the snapshot half.
Impact: one violation is a permanent hang.
Open questions: None.

### read-task-quiescence-implies-no-further-registration

Type: safety
Reachability: default-production - the read-task tracker is closed and awaited
on every connection teardown (`crates/host-runtime/src/connection.rs:326-327`).
Status: active
Exercised: not yet - the existing fence tests hand-roll the producer.
Guarantee: Once a generation's read-task set is closed and empty, nothing can
register another future in it.
Check: `always` - after the wait returns, the read loop has returned and no
registration site on that tracker is reachable.
Fault/timing angle: the tracker's wait completes when closed and empty, and
closing does not forbid later registration. Everything registering is spawned from
the read loop or before it starts, and the safety net is that the read loop is
itself tracked in the same set, so the count cannot reach zero while a producer
exists. The shutdown sequence closes the tracker while read loops are still live,
which is exactly the case the argument must cover.
Required faults and enabling state: a read cancellation fired while an emission
task is mid-flight.
Confidence: high - [evidence](evidence/read-task-quiescence-implies-no-further-registration.md). all ten registration sites enumerated; all are inside the read
loop's dynamic extent or precede it.
Existing check: `connection.rs:1598-1607` proves an already-started producer is
waited for, but hand-rolls the producer with a bare spawn instead of driving the
real read loop, so it does not cover who else can register. Both are
current-thread.
Impact: a refactor that spawns into the set from outside the read loop, or stops
tracking the read loop itself, makes shutdown silently stop waiting for producers.
Open questions: None.

### a-cancelled-emission-releases-every-permit-it-held

Type: safety
Reachability: default-production - the pending, reject, and egress pools are
constructed for every host incarnation
(`crates/host-runtime/src/runtime.rs:903-914`) and the per-generation reject permits
per connection (`crates/host-runtime/src/connection.rs:244`).
Status: active
Exercised: partial - connection permits on the candidate path only.
Guarantee: Aborting or dropping any off-reader emission task returns its pending
permit, its per-generation reject permit, and its egress byte charge.
Check: `always` - saturate the pools, abort the emission tasks, then assert both
semaphores return to full and the egress budget to zero.
Fault/timing angle: the pattern is a permit acquired before spawn and rebound
*inside* the future, which is what makes abort release it. Moving any binding
outside the async block leaks the permit on abort while leaking nothing on
success, so the bug would be invisible to a happy-path test.
Required faults and enabling state: pools at or near saturation, plus a forced
sweep or a read cancellation while emissions are parked on contended egress.
Without saturation the check cannot distinguish a leak from headroom.
Confidence: high - [evidence](evidence/a-cancelled-emission-releases-every-permit-it-held.md). the binding is inside the future at all seven sites, verified.
Existing check: `tests/transport_negotiation.rs:1522` covers connection permits;
`tcp_frame_channel.rs:944` and `:1062` cover charges. Nothing covers pending or
reject permits under abort.
Impact: a stranded permit is unrecoverable without a restart.
Open questions: None.

### no-writer-hook-panic-poisons-a-generation-lock

Type: safety
Reachability: explicit-config-only - the liveness loop is spawned only when a
policy is configured (`crates/host-runtime/src/connection.rs:279`),
`HostConfig::default` leaves `liveness: None`
(`crates/host-runtime/src/config.rs:236`, re-verified), and no in-tree caller of `run`, in
`crates/host-runtime/examples/` or the bench, sets `liveness`; the daemon that will build the
production config is scheduled for U4 (`docs/properties/README.md:52`).
Status: active
Exercised: not yet - requires an injected panic.
Guarantee: A panicking write-completion hook cannot leave any generation mutex
poisoned, and cannot convert one connection's fault into a panic on another task.
Check: `always` - install a hook that panics while holding the pings lock, then
assert the read loop's pong path and the liveness loop still make progress rather
than panicking on the lock.
Fault/timing angle: the completion hook is called synchronously inside the writer
task with no unwind guard, verified. The liveness hook takes the pings lock and
does instant arithmetic; a panic there poisons the lock, and the read loop, the
wake computation, the expiry scan, and the insert all expect a healthy lock. The
unwind also skips the writer's retirement signal, so the writer dies without
setting `retired` and senders learn only through the closed channel.
Required faults and enabling state: a configured liveness policy and an injected
panic in a completion hook. Unreachable today, which makes this a hardening
property rather than a live bug.
Confidence: high - [evidence](evidence/no-writer-hook-panic-poisons-a-generation-lock.md). For the mechanism; medium that a hook can panic today, since no
current hook has arithmetic that must overflow.
Existing check: none. The comparable boundaries elsewhere *are* guarded - the
provider preflight, the prepare worker, and the writer's owned-conversion all
catch unwind. This call is the gap in an otherwise consistent policy.
Impact: every later connection becomes a panicking task, and because the panic
originates outside a handler callback it prints unredacted.
Open questions: None.

---

## Group D: the shutdown commit latch

### shutdown-commits-exactly-once-on-write-ack

Type: safety
Reachability: default-production - the shutdown latch is constructed for every
host incarnation (`crates/host-runtime/src/runtime.rs:919`) and driven by the
ordinary shutdown path; nothing gates it on configuration.
Status: active
Exercised: yes - four in-crate latch tests plus three integration tests, and
the integration file runs in CI in this tree (`ci.yml:118`, `:126`, `cargo test --workspace --all-targets`).
Guarantee: Across any number of concurrent and repeated shutdown requests, the
latch commits and the shutdown token is cancelled at most once per incarnation.
Check: `always` - drive concurrent and pipelined requests, some on generations that retire mid-flight; assert the commit executes exactly once when at least one committing response's write callback is acknowledged, and zero times when every owner generation retires before its response is written (each dropped `CommitOnAck` reopens the latch); assert each requester receives exactly one correlated response or none, and none receives two.
Fault/timing angle: the exclusion rests on the commit hook being moved into the
frame's written callback, so it either fires or is dropped, never both. The subtle
part is that commit is unconditional while reopen is guarded, so a late reopen
after a commit is a no-op but a late commit after a reopen would not be.
Required faults and enabling state: at least two requesters, plus a
pre-acknowledgement failure on the first owner.
Confidence: high - [evidence](evidence/shutdown-commits-exactly-once-on-write-ack.md). all transitions read and the mutual exclusion traced.
Existing check: strong. Four in-crate tests including one that directly pins the
enable-before-check rule against a lost wakeup, plus three integration tests.
Status unaudited.
Impact: this is the stop linearization point.
Open questions: None.

### admission-freeze-precedes-the-shutdown-commit

Type: safety
Reachability: default-production - the shutdown latch is constructed for every
host incarnation (`crates/host-runtime/src/runtime.rs:919`) and driven by the
ordinary shutdown path; nothing gates it on configuration.
Status: active
Exercised: not yet - all four latch tests construct the hook with no registry.
Guarantee: At the instant the commit cancels the shutdown token, registry
admission is already frozen; no path can commit without freezing first.
Check: `always` - assert the freeze happens-before the cancellation on every path
that reaches the commit.
Fault/timing angle: this is a repaired defect. Generation registration once tested
only the draining flag, which the shutdown *sequence* stores, while a committed
shutdown cancels the token first, so a socket accepted in between registered a new
generation after the advertised admission-cancellation point and handler work
started after the commit. The registration gate now reads both, and dispatch
stores draining and freezes before acknowledging. But the commit only commits and
cancels; the freeze is entirely the caller's duty, unenforced by the type.
Required faults and enabling state: a socket accepted and authenticated between
the token cancellation and the freeze.
Confidence: high - [evidence](evidence/admission-freeze-precedes-the-shutdown-commit.md). the ordering and the unenforced duty were both read.
Existing check: the latch tests cannot see it, because they have no registry.
Impact: handler work admitted after the host promised it had stopped admitting.
Open questions: None.

### shutdown-commit-effects-are-all-or-nothing

Type: safety
Reachability: default-production - the shutdown latch is constructed for every
host incarnation (`crates/host-runtime/src/runtime.rs:919`) and driven by the
ordinary shutdown path; nothing gates it on configuration.
Status: active
Exercised: not yet - one test covers the hook never running; nothing runs the hook and fails it partway to assert that draining and the latch either both moved or neither did.
Guarantee: The commit point either applies all three effects - draining, frozen
route admission, latch commit plus token cancellation - or none.
Check: `always` - for every prefix of the hook body at which a panic is injected, after the hook has unwound either all three effects are applied (draining set, admission frozen, latch committed with the token cancelled) or none is observable or retained (draining false, admission open, latch reopened and not acknowledged). A partial state, in particular draining set with the latch reopened, or the acknowledged flag set without a commit, fails the check; recovery by a later successor does not make a partial state pass.
Fault/timing angle: the hook runs three effects in sequence inside the writer
task. Corrected after portfolio evaluation: a tokio **abort cannot** split it,
because the hook body is a synchronous closure with no await point, and tokio
cancels only at await points. Only a panic can, and there are two distinct
prefixes with different severities. A panic in the freeze leaves draining true
while the dropped hook reopens the latch, which is recoverable by a successor
requester. A panic inside the acknowledgement is worse: the acknowledged flag is
set *before* the commit, so the drop declines to reopen and the latch is stuck in
the in-flight phase with no possible successor. That second prefix is the wedge.
Under the check as stated, the freeze-prefix panic is a predicted violation, not a recoverable pass: it leaves draining set with the latch reopened. The record keeps the guarantee unconditional because the code's own contract is all-or-nothing; a campaign that constructs the prefix should expect the check to fail until the hook restores draining on unwind.
Required faults and enabling state: an authenticated shutdown that reaches write
completion, plus a panic at one of the two prefixes.
Confidence: medium - [evidence](evidence/shutdown-commit-effects-are-all-or-nothing.md). the hazard and both prefixes are read directly; a panic in
the freeze could not be constructed by inspection, so reachability rests on the
general no-unwind-guard argument rather than a specific panicking operation.
Existing check: one test covers the hook never running. Nothing covers it running
partially.
Impact: a wedged host holding the instance lock.
Open questions: None.

### latch-wake-cannot-be-lost

Type: liveness
Reachability: default-production - the shutdown latch is constructed for every
host incarnation (`crates/host-runtime/src/runtime.rs:919`) and driven by the
ordinary shutdown path; nothing gates it on configuration.
Status: active
Exercised: partial - `an_enabled_change_future_survives_a_pre_poll_notification` (`crates/host-runtime/src/lifecycle.rs:1771`) pins, under a one-second bound, that a notification landing between `try_own` and the first poll is observed, but only for `reopen` (`:1778-1781`); the test that reaches the `commit` notification (`:1763`) awaits the waiter without a bound at `:1765`, so a lost commit wake hangs the job rather than failing this record's one-poll oracle. Note that the bounded test constructs `changed()` before `try_own()`
Guarantee: A shutdown requester that observes the wait state is always woken by
the next phase change.
Check: `always` - for every interleaving of ownership attempt, reopen, commit, and a waiter's change-future lifecycle, the waiter returns owner or committed within a bounded window after the phase change: the existing test bounds it at 1 s of wall clock (`lifecycle.rs:1779`), and under `start_paused` the waiter must be ready at the first poll after the notify. The invariant a test must protect is that the `Notified` future is created (`changed()`, `lifecycle.rs:1091`) before the state is re-checked (`try_own()`, `:1065`).
Fault/timing angle: the latch notifies with `notify_waiters` (`lifecycle.rs:1083`,
`:1088`), which stores no permit. For the pinned Tokio 1.53.1, a `Notified` future
is guaranteed to observe `notify_waiters` from the moment it is created, polled or
not (`tokio/src/sync/notify.rs:529-531`); `enable()` is load-bearing for
`notify_one`, not for this primitive. The lost-wake fence is therefore creating
`changed()` before calling `try_own()`; a mutation that moves the creation below
the state check reintroduces the lost wake even if `enable()` is still called.
Both `reopen` and `commit` release the phase lock before notifying: in `commit`
the guard from `*self.phase.lock().expect(..) = ..` is a temporary dropped at the
semicolon (`lifecycle.rs:1087`), so `notify_waiters` on the next line runs
unlocked. The source catalog described a notify-under-lock asymmetry here; it does
not exist, and the evidence file records the same correction.
Required faults and enabling state: at least two concurrent requests on distinct
generations, plus a pre-acknowledgement failure so reopen fires rather than
commit. Needs a multi-thread runtime for the notify-between-check-and-poll
interleaving to be reachable, and every existing test is current-thread.
Confidence: high - [evidence](evidence/latch-wake-cannot-be-lost.md). the protocol is correct as written and the reasoning is
spelled out in comments.
Existing check: the strongest existing check in this scope. Status unaudited.
Impact: a lost wakeup is a permanently stuck requester holding a pending permit.
Open questions:
- The source comment at `lifecycle.rs:1769` and the `enable()` calls at `:1750` and `:1776` attribute the guarantee to enabling rather than to creation order; the calls are harmless but the stated rationale disagrees with the Tokio contract. Contract-versus-code note, not resolved here. (needs human input)

---

## Group E: daemon incarnation and the probe

### probe-never-reports-stopped-while-either-fence-is-held

Type: safety
Reachability: test-only - the lifecycle record this probe reads is written on
every host start (`crates/host-runtime/src/runtime.rs:587` for `Starting`, `:715`
for `Running`, re-verified) and on teardown (`lifecycle.rs:384`), but the reader
this record is about, `probe_lifecycle` (`lifecycle.rs:805`), has no caller outside
`crates/host-runtime` tests in this tree. Its production consumer, the daemon CLI
(`crates/daemon`), is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: yes - five in-crate tests in `lifecycle.rs`, none gated on `target_os`; the only Linux-gated items in that module are the FIFO cases, which do not bear on this record (re-verified).
Guarantee: The probe returns stopped only when both the lifetime fence and the
runtime-directory instance lock are observed free.
Check: `always` - for every evidence shape, hold each fence in turn and assert the
verdict is never stopped; in particular, replace the subtree under a live daemon
and assert wedged.
Fault/timing angle: the two fences are acquired in opposite orders at start and
teardown, so a probe can legitimately land where exactly one is held; the code
absorbs that with a bounded disagreement grace and then classifies wedged. The one
coherent single-fence shape, a held runtime lock with a free lifetime fence and a
legacy record, is a pre-coordination incumbent and classifies by its record.
Required faults and enabling state: a live daemon plus namespace replacement, or a
probe sampling inside the few-syscall window between the two acquisitions.
Confidence: high - [evidence](evidence/probe-never-reports-stopped-while-either-fence-is-held.md). stopped is returned from exactly two places and both require
the lifetime fence free.
Existing check: strong, five tests, portable across Unix. Status unaudited.
Impact: a false stopped authorizes a launcher to start a second incarnation over a
live one.
Open questions: None.

### stopping-precedes-unpublication-on-every-path

Type: safety
Reachability: test-only - the stopping record is written on the shipped teardown
path (`crates/host-runtime/src/lifecycle.rs:384`, re-verified), but the reader that
this record's guarantee is stated against, `probe_lifecycle` (`lifecycle.rs:805`),
has no caller outside `crates/host-runtime` tests in this tree. Its production
consumer, the daemon CLI (`crates/daemon`), is scheduled for U4
(`docs/properties/README.md:52`); reclassify to `default-production` in the wave
that lands it.
Status: active
Exercised: partial - the success path only.
Guarantee: When an incarnation removes its publication, the on-disk record already reads stopping, so an orderly stop never classifies wedged; if the stopping write fails, the publication is not removed until the failure has been surfaced.
Check: `always` - fault-inject the phase write, run each teardown path, and assert that the publication survives until the phase is demoted, or that the write failure is surfaced (returned or logged) before the removal; a removal after a silently failed demotion fails the check. This is a predicted violation at HEAD: `begin_stopping` (`crates/host-runtime/src/lifecycle.rs:383-386`, re-verified) discards the write result with `let _ =` and removes the publication unconditionally, so a failed demotion is neither ordered nor surfaced.
Fault/timing angle: the ordering inside the demotion function is correct and all
five teardown paths route through it. The gap is that the phase write's error is
discarded and teardown proceeds regardless. The in-code justification, that a
stale phase ages to wedged honestly, covers a *successful* write followed by a
hang, not a *failed* write, which produces an immediate wedged for a clean stop.
Required faults and enabling state: a storage or permission failure on the runtime
directory at teardown, with a publication still present.
Confidence: high - [evidence](evidence/stopping-precedes-unpublication-on-every-path.md). On the ordering and the discarded error; medium on reachability.
Existing check: two tests cover the success path. Nothing covers the failed write.
Impact: an orderly stop reported to the operator as a fault.
Open questions:
- Is a failed demotion meant to abort or delay publication removal? The contract
  says MUST demote first without saying what a failed demotion means. (needs human
  input)

### phase-evidence-outlives-a-long-phase

Type: safety
Reachability: test-only - the lifecycle record this probe reads is written on
every host start (`crates/host-runtime/src/runtime.rs:587` for `Starting`, `:715`
for `Running`, re-verified) and on teardown (`lifecycle.rs:384`), but the reader
this record is about, `probe_lifecycle` (`lifecycle.rs:805`), has no caller outside
`crates/host-runtime` tests in this tree. Its production consumer, the daemon CLI
(`crates/daemon`), is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active - **reframed after portfolio evaluation**
Exercised: not yet - the two existing tests assert that expiry produces wedged; nothing holds both fences through a phase longer than the evidence window and asserts the probe never reports wedged.
Guarantee: The documented freshness window is wide enough for every phase the
implementation can legitimately take, or the phase budget is coupled to the window
so the two cannot disagree.
Check: `always` - for a live coherent incarnation holding both fences, no phase
that the configuration permits can exceed the freshness window. The framing was
corrected: an earlier revision asserted that a healthy long phase must never be
classified wedged, which **contradicts the documented contract**. The protocol's
classification table states that an expired record in any phase is wedged, and its
prose says the freshness windows "still age a hung start or stop to wedged". So
ageing out is specified behaviour, not a violation. The real defect is a
*coupling* gap, which is what this record now states.
Fault/timing angle: the record is written once per phase transition and never
refreshed, and freshness compares it against a fixed 60 second wall-clock window
(`lifecycle.rs:770-776`, value at `:773`). That window is **not configurable**: the
sole production construction is the default, with no flag, field, or environment
override. Meanwhile the frame deadline and the lifecycle callback deadline are
operator-settable up to 365 days. So an operator can legally configure a phase
budget three orders of magnitude larger than the window that judges it, and
nothing couples them.
Required faults and enabling state: a configuration whose callback or drain budget
exceeds 60 seconds, or a slow filesystem making a phase exceed it. No adversary
needed.
Confidence: high - [evidence](evidence/phase-evidence-outlives-a-long-phase.md). the window value, its non-configurability, and the settable
budgets were all verified. One candidate cause the earlier revision named,
per-file hashing during payload validation, was **refuted**: it runs before the
phase record exists.
Existing check: two tests assert that expiry produces wedged, which is the
documented behaviour. Nothing asserts the window bounds what the configuration
permits.
Impact: the freshness window is an undocumented hard cap on startup and shutdown
duration, and it is the one value in the pair that cannot be tuned.
Open questions:
- Should the window scale with the configured budgets, or should the budgets be
  clamped to it? Either couples them; the protocol specifies neither. (needs human
  input)

### clock-anomalies-do-not-invalidate-live-evidence

Type: safety
Reachability: test-only - the lifecycle record this probe reads is written on
every host start (`crates/host-runtime/src/runtime.rs:587` for `Starting`, `:715`
for `Running`, re-verified) and on teardown (`lifecycle.rs:384`), but the reader
this record is about, `probe_lifecycle` (`lifecycle.rs:805`), has no caller outside
`crates/host-runtime` tests in this tree. Its production consumer, the daemon CLI
(`crates/daemon`), is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: not yet - both freshness tests manipulate the record, not the clock.
Guarantee: A wall-clock step or an unrepresentable clock value does not reclassify
a live incarnation.
Check: `always` for a live coherent incarnation - the probe is invariant to
wall-clock adjustments larger than the freshness window.
Fault/timing angle: the millisecond helper collapses a pre-epoch clock to zero and
an unrepresentable count to the maximum. Zero fails the freshness test in one
direction, the maximum fails it in the other, so both directions of a real NTP
step or a suspend and resume longer than the window produce wedged. Neither the
daemon nor the probe uses a monotonic source for this comparison, and there is no
skew allowance beyond the same 60 second value used for expiry.
Required faults and enabling state: a clock step exceeding the window, or a clock
set before the epoch, concurrent with an incarnation in starting or stopping.
Confidence: high - [evidence](evidence/clock-anomalies-do-not-invalidate-live-evidence.md). the saturating collapses and the wall-clock comparison are
literal.
Existing check: one test confirms the future-side behaviour is intended for a
*forged* record; it does not distinguish forgery from a clock step.
Impact: a routine time correction reclassifies a healthy host as incoherent.
Open questions: None.

### legacy-incumbent-classification-needs-an-unforgeable-witness

Type: safety
Reachability: test-only - the lifecycle record this probe reads is written on
every host start (`crates/host-runtime/src/runtime.rs:587` for `Starting`, `:715`
for `Running`, re-verified) and on teardown (`lifecycle.rs:384`), but the reader
this record is about, `probe_lifecycle` (`lifecycle.rs:805`), has no caller outside
`crates/host-runtime` tests in this tree. Its production consumer, the daemon CLI
(`crates/daemon`), is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: partial - the regression test plants exactly the forgery by hand.
Guarantee: A running verdict derived from a legacy record is accompanied by
evidence the record's author cannot forge, or the verdict is wedged.
Check: `always` - for a legacy-shaped record beside a matching publication, the
running verdict requires a witness not writable by whoever wrote the record.
Fault/timing angle: this is a repaired defect whose fix widened the classification
to an unauthenticated shape. A canonical-digest requirement once made every
pre-coordination record decode as malformed, so routine upgrades saw an alarm
instead of a stoppable incumbent. The fix accepts *any* empty-digest record beside
a matching publication as running. Both files are attacker-writable under the
same-user model, and nothing distinguishes a genuine pre-coordination daemon from
a squatter holding only the runtime-directory flock.
Required faults and enabling state: a planted empty-digest record plus a matching
publication, with the runtime lock held.
Confidence: high - [evidence](evidence/legacy-incumbent-classification-needs-an-unforgeable-witness.md). the widened predicate was read directly, and the regression
test constructs the forgery itself.
Existing check: one test pins the classification, using the forgeable shape.
Status unaudited.
Impact: a squatter is classified as a live incumbent, which suppresses a
successor.
Open questions:
- Are pre-coordination releases trusted by definition? If so, state it; if not,
  the rule needs an unforgeable witness. (needs human input)

### an-observed-wedge-cause-reaches-the-operator

Type: safety
Reachability: test-only - the lifecycle record this probe reads is written on
every host start (`crates/host-runtime/src/runtime.rs:587` for `Starting`, `:715`
for `Running`, re-verified) and on teardown (`lifecycle.rs:384`), but the reader
this record is about, `probe_lifecycle` (`lifecycle.rs:805`), has no caller outside
`crates/host-runtime` tests in this tree. Its production consumer, the daemon CLI
(`crates/daemon`), is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: not yet - in-crate tests assert the reason field directly; nothing asserts that each distinguished wedge reason reaches an operator-visible surface, and the CLI that would render it is not in this tree.
Guarantee: When the host distinguishes a wedge cause, that distinction is
observable outside the process.
Check: `always` - for every one of the thirteen reasons `classify` (`crates/host-runtime/src/lifecycle.rs:917`) distinguishes, the operator-visible output produced from that reason differs from the output produced from every other reason. This is a value-mapping assertion over the whole reason set, not location coverage: reaching the renderer once proves nothing when twelve reasons collapse to the same `wedged` output, which is the predicted violation at HEAD.
Fault/timing angle: none. The classifier computes thirteen distinct reasons; the
sole production consumer forwards one and collapses the rest to a bare "wedged".
A probe *error* also becomes "wedged", erasing the distinction between fence
incoherence and an I/O failure. Verified: the crate has no tracing or log
dependency, so there is no second channel.
Required faults and enabling state: any wedge other than the forwarded one; two
are already fixtured in the existing tests.
Confidence: high - [evidence](evidence/an-observed-wedge-cause-reaches-the-operator.md). the forwarding is a single conditional and the reason table is
complete in one function.
Existing check: none. In-crate tests assert the reason field directly, so the
crate proves the reasons are computed while nothing proves they are conveyed.
Impact: twelve of thirteen diagnosable causes are indistinguishable to an
operator, and remediation advice is uniform where the causes are not.
Open questions:
- Is only the forwarded reason a contract, with the other twelve as pure
  diagnostics? If so they are diagnostics nobody can see. (needs human input)

---

## Group F: the payload generation store

### current-profile-never-names-an-unvalidatable-generation

Type: safety
Reachability: test-only - `stage_and_promote` has no caller outside
`crates/host-runtime` tests in this tree (re-verified by a workspace-wide search).
The daemon CLI that reads the current profile and promotes on every start
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: partial - success and post-hoc tampering only; no fault injection and
no crash test.
Guarantee: After any outcome of staging and promotion, including a crash at any
point, the current profile is absent, quarantined, or names a digest that
validates.
Check: `always` - for every fallible step, inject a failure and a simulated crash,
then assert the invariant on the store.
Fault/timing angle: durability ordering carries this. Files and every created
directory are synced deepest-first inside the temp, the promoting rename is
followed by a directory sync, and only then is the profile rewritten and the root
synced. A crash between promote and profile replacement leaves an orphan that a
later prune removes as unprotected.
Required faults and enabling state: storage exhaustion at each write and sync
point, a delayed-allocation filesystem so exhaustion first surfaces at sync, and
power-loss simulation between the two renames, all under the transaction lock.
Confidence: high - [evidence](evidence/current-profile-never-names-an-unvalidatable-generation.md). On the ordering; medium on completeness, since the
exchange-then-revalidate window has a state where the digest name holds a
candidate and the temp name holds the corrupt orphan.
Existing check: four tests cover the success path, same-digest convergence,
post-hoc tampering, and the quarantine abort. Status unaudited.
Impact: the profile is the selector that decides which payload the daemon
executes.
Open questions:
- Should the exchange-then-revalidate window be crash-tested? Whether any reader
  can observe the intermediate was not established.

### validation-and-enumeration-address-one-directory-object

Type: safety
Reachability: test-only - `stage_and_promote` has no caller outside
`crates/host-runtime` tests in this tree (re-verified by a workspace-wide search).
The daemon CLI that reads the current profile and promotes on every start
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: partial - the two fixed instances each have a regression test: the
validation walk that once re-resolved by pathname, and the prune enumeration that
did the same for eight further review rounds. No test sweeps the remaining store
operations (read, walk, and removal in `stage_and_promote`, quarantine, and
current-profile handling) for a third pathname-based call, which is the open
question below.
Guarantee: Every read, walk, and removal in a store operation resolves through the
descriptor that operation pinned, never through a re-resolved pathname.
Check: `always` - for every store operation, a replacement directory planted at
the operation's name cannot redirect a read, a walk, or a removal.
Fault/timing angle: this is a defect class that recurred. Validation once verified
manifest-listed files through a retained descriptor while walking for unlisted
entries by pathname, so a replacement directory holding only the expected names
satisfied the walk while the result still pinned the original. That was fixed. The
identical split then survived in prune for eight more review rounds, where
enumeration by pathname drove deletions inside the pinned store.
Required faults and enabling state: a directory replacement between the pin and the
walk, under the transaction lock.
Confidence: high - [evidence](evidence/validation-and-enumeration-address-one-directory-object.md). both instances read as diffs.
Existing check: partial; the fixed instances have regression tests. Nothing
prevents a third instance.
Impact: two separate shipped defects from one class, and the class was never swept.
Open questions:
- How many more instances exist? A sweep of every pathname-based call in the store
  would settle it.

### an-undecidable-quarantine-witness-fails-closed

Type: safety
Reachability: test-only - `stage_and_promote` has no caller outside
`crates/host-runtime` tests in this tree (re-verified by a workspace-wide search).
The daemon CLI that reads the current profile and promotes on every start
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: partial - the oversize case only.
Guarantee: For every *read-failure* mode of the lifecycle record and the generation
manifest, the quarantine gate refuses the mutation rather than admitting an
overwrite or a delete.
Check: `always` - for an oversize read, an I/O error, and a permission error, the
gate reports quarantined. Scope narrowed after portfolio evaluation: an earlier
revision also included non-regular shapes. That clause is **wrong** for the
lifecycle record, where replacing a planted symlink or FIFO at the record name
without following it is deliberate, documented, and covered by a passing test. The
property is about *undecidable reads*, not about hostile shapes, which have their
own separate and correct handling.
Fault/timing angle: this is a repaired defect with an unswept sibling. The
lifecycle gate once failed open on open, stat, and read failures, which admits the
start, and startup then overwrites the record by atomic rename. That was fixed.
The manifest-side gate still returns removable on four distinct failure modes, and
the most reachable of them is one the earlier revision did not name: the child
directory open rejects **any** group or other mode bit, so a 0o755 generation
directory from a future release or a restored backup is classified removable and
deleted. The other three are a missing or symlinked manifest, a stat error on an
open descriptor, and a read error.
Required faults and enabling state: a generation directory with a wider mode, or an
oversize manifest, or an I/O error on either object.
Confidence: high - [evidence](evidence/an-undecidable-quarantine-witness-fails-closed.md). the four early returns were enumerated and their reachability
ranked; the mode-bit cause is the practical one.
Existing check: one test covers the oversize manifest case.
Impact: a retained generation written by a newer release, or restored with wider
modes, is deleted by prune. That is the forward-compatibility break quarantine
exists to prevent.
Open questions: None.

### persisted-state-quarantine-caps-agree

Type: safety
Reachability: test-only - `stage_and_promote` has no caller outside
`crates/host-runtime` tests in this tree (re-verified by a workspace-wide search).
The daemon CLI that reads the current profile and promotes on every start
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
to `default-production` in the wave that lands it.
Status: active
Exercised: not yet - statically checkable, and currently false.
Guarantee: The size above which persisted state is unreadable and therefore
quarantined is one value across the lifecycle record and the generation manifest.
Check: `always` - the lifecycle-record cap and the generation-manifest cap (`MAX_MANIFEST_BYTES`, `generation.rs:46`, 1 MiB) are the same value. This is a predicted violation at HEAD, where the two thresholds differ (64 KiB against 1 MiB) while the source comment claims they match; accepting distinct caps requires revising or invalidating this guarantee, not passing the check.
Fault/timing angle: forward compatibility. Verified: the evidence cap is 65,536
bytes and the manifest cap is 1,048,576 bytes, sixteen times apart, while the
manifest constant is documented as "matching the lifecycle evidence cap". A future
release writing a 100 KiB record is quarantined by this release; a 100 KiB manifest
is not. A maintainer adjusting one cap on the comment's authority moves only one
threshold.
Required faults and enabling state: none.
Confidence: high - [evidence](evidence/persisted-state-quarantine-caps-agree.md). both constants and the claim read directly.
Existing check: none.
Impact: the two forward-compatibility thresholds that must agree do not, and the
code says they do.
Open questions:
- Should the two caps be unified, or should the guarantee be invalidated and each threshold documented on its own? Until decided, the check fails. (needs human input)

### every-declared-cli-reason-id-has-a-producer

Type: safety
Reachability: test-only - neither the CLI binary that emits the reason ids
(`crates/daemon/src/bin/eidnara-host.rs`) nor the plugin surface that consumes
them (`packages/plugin/src/shared/host-lifecycle/paths.ts`) is in this tree; the
`packages/` directory holds only `shm-native`. The daemon is scheduled for U4
(`docs/properties/README.md:52`); reclassify in the wave that lands it. The
TypeScript producer and mis-mapping findings below are source-repository evidence.
Status: active - **premise corrected after portfolio evaluation**
Exercised: not yet - the TypeScript producer tests are in the source repository; nothing in this tree enumerates the declared ids against Rust producers, and the CLI and plugin surfaces are absent.
Guarantee: Each reason id the release contract declares is emitted by the layer
the remediation implies, and a condition that maps to one id is not reported under
another.
Check: `always` - for every classified failure condition in the Rust layer, the reason id it produces is the one the release contract declares for that condition, and each declared id has at least one producer in the layer the remediation implies. The first clause is a predicted violation at HEAD for the filesystem conditions named below: an atomic exchange unsupported on the volume, a filesystem without the rename flags, and a cross-device rename all map to the payload-invalid result rather than `unsupported_filesystem`. `always` because the mapping must hold for every condition, not once per vocabulary entry.
Fault/timing angle: no timing angle. An earlier revision of this record claimed
`unsupported_filesystem` has **no** producer anywhere in the workspace. That is
false and is corrected here: it is produced in TypeScript, by the managed-policy
path preflight (`packages/plugin/src/shared/host-lifecycle/paths.ts:157`), with
its own passing tests. What remains true, and is the real finding, is narrower:
the **Rust** conditions that ought to yield it - an atomic exchange unsupported on
the volume, a filesystem without the rename flags, a cross-device rename - all map
to the payload-invalid error instead, so a user whose filesystem cannot support the
operation is told to reinstall the payload. The declared id exists and is
reachable; the native classification does not use it.
Required faults and enabling state: a data root on a filesystem lacking atomic
same-filesystem exchange, with a corrupt unprotected occupant at the digest name so
promotion reaches the exchange.
Confidence: high - [evidence](evidence/every-declared-cli-reason-id-has-a-producer.md). the TypeScript producer and the Rust mis-mapping were both
verified, and 17 of 31 declared ids have Rust producers.
Existing check: the TypeScript side has targeted tests. Nothing checks that the
native error classification uses the declared vocabulary.
Impact: an operator-facing diagnosis exists but the native layer cannot emit it, so
the same root cause produces different advice depending on which layer noticed.
Open questions:
- For the 13 declared ids with no Rust producer, is the intent that the
  TypeScript policy layer owns them entirely? A partial survey found producers for
  some; no count is asserted. (needs human input)

---

## Group G: the panic boundary

### every-callback-invocation-is-inside-the-redaction-guard

Type: safety
Reachability: default-production - every handler callback on the default
dispatch path goes through the guard (`crates/host-runtime/src/dispatch.rs:994-995`,
`:1148-1150`, `:1263-1264`) as does the shutdown callback
(`crates/host-runtime/src/runtime.rs:303-304`).
Status: active
Exercised: partial - one test pins the not-over-broad direction.
Guarantee: Every call into untrusted handler or provider code runs with the
redaction guard active, for both the synchronous prologue and each individual
future poll.
Check: `always` - every invocation is wrapped; a panic in any callback emits only
the redacted string; and a panic on the same worker from an unrelated task is not
redacted.
Fault/timing angle: the guard is a thread-local depth counter incremented per poll
rather than per await, so a yielded callback cannot suppress another task's panic
on the same worker. Installation is once-only, so the first caller decides which
prior hook is preserved, and any hook installed by a test harness afterwards is
replaced.
Required faults and enabling state: a panicking callback, plus a concurrently
panicking unrelated task on the same worker to prove the guard is not over-broad.
Confidence: high - [evidence](evidence/every-callback-invocation-is-inside-the-redaction-guard.md). On the inventory, which is an exhaustive grep of roughly twenty
call sites; medium on the guarantee, because the promise is enforced by convention
at each site with nothing in the type system requiring a new site to wrap.
Existing check: `tests/dispatch.rs:661` pins the not-over-broad direction. Verified:
`panic_boundary.rs` has **zero** test modules of its own, so nothing asserts what
is printed, that the prior hook is preserved, or that the depth counter unwinds
correctly through a panic.
Impact: one unwrapped call site leaks handler panic payloads and backtraces.
Open questions: None.

### the-panic-hook-cannot-itself-fail

Type: safety
Reachability: default-production - the redaction guard and its reporting run on
the default dispatch path (`crates/host-runtime/src/dispatch.rs:994-995`).
Status: active
Exercised: not yet - no test installs the hook and drives it with a panic payload that fails to format, a poisoned lock, or a blocked writer to assert it neither panics nor blocks.
Guarantee: Reporting a redacted callback panic never escalates into process abort
or an indefinite stall.
Check: `always` - the hook completes without panicking and without blocking, for
every state of the standard error stream.
Fault/timing angle: the hook's only output is a single `eprintln!`, verified, which
panics on a write error. A panic inside the hook is a nested panic and therefore an
abort, which bypasses the stopping demotion entirely, so the publication and a
running record survive and the launcher never observes stopping. The daemon's
standard error is a log file, so the live trigger is a full or failing disk -
precisely the condition the storage-exhaustion error exists to name. If the stream
is a pipe whose reader has stalled, the write blocks and the panicking thread parks
inside the hook.
Required faults and enabling state: a callback panic concurrent with a write
failure on the standard error stream, or a non-draining consumer.
Confidence: medium - [evidence](evidence/the-panic-hook-cannot-itself-fail.md). the panic-on-write-error and nested-panic-abort behaviours are
stable standard-library semantics; the disk-full coincidence is plausible rather
than demonstrated.
Existing check: none.
Impact: a callback panic plus a full disk converts a redacted diagnostic into an
abort that skips the entire teardown ordering.
Open questions: None.

---

## Group H: observability and coverage

### authentication-and-capacity-rejections-are-observable

Type: safety
Reachability: default-production - both rejection arms are on the default
connection path: the authentication return and the connection-permit
`try_acquire_owned` failure (`crates/host-runtime/src/connection.rs:130-138`).
Status: active
Exercised: not yet - no test triggers each rejection class and asserts an operator-visible record; the record itself finds there is no channel to carry one.
Guarantee: Every rejected connection produces some record an operator can see.
Check: `always` - for every rejection event (authentication failure, connection capacity exhaustion, post-authentication drain refusal), some counter, log line, or frame differs from the accepted case; three constant coverage markers, one per class, show that each class was constructed at least once, and are instrumentation rather than the assertion. This is a predicted violation at HEAD: the record finds no channel that carries any of the three. `always` because the guarantee is over every rejection, not over one example per class.
Fault/timing angle: none. Verified: authentication failure returns with no counter,
no log, and no rate signal, and the peer address is already dropped at accept, so
a credential-probing client is indistinguishable from silence. Capacity exhaustion
and drain refusal both drop an authenticated client with no frame. The crate has
no tracing or log dependency, so there is no channel to carry any of it.
Required faults and enabling state: an authentication failure, a capacity
exhaustion, and a drain refusal.
Confidence: high - [evidence](evidence/authentication-and-capacity-rejections-are-observable.md). all three discard sites verified at their line numbers.
Existing check: none.
Impact: the single most alarm-worthy event in the connection path produces nothing,
and capacity exhaustion looks like a network reset to both sides.
Open questions: None.

### the-largest-lifecycle-proof-runs-in-ci

Type: reachability
Reachability: test-only - the subject is the integration-test binaries and the
CI workflow that runs them. Re-verified against this tree: `.github/workflows/ci.yml:118`
and `:126` run `cargo test --workspace --all-targets --all-features --locked` on
the 1.98 and stable toolchains, on `ubuntu-latest` only. This is build
configuration, not a runtime path.
Status: active
Exercised: yes - `cargo test --workspace --all-targets` builds and runs every
integration binary in the workspace, including `tests/lifecycle.rs` (35 tests),
`tests/activation.rs` (4), and `tests/host_roundtrip.rs` (4), on both toolchains
(`ci.yml:118`, `:126`). No binary is named individually; none needs to be.
Guarantee: The executed proof of shutdown ordering, lock-release ordering, latch
commit, fence overlap refusal, and probe-across-an-incarnation runs in continuous
integration.
Check: `reachable` - each named proof, not merely its containing binary, executes in a workflow on at least one platform: the shutdown-ordering, lock-release-ordering, latch-commit, fence-overlap-refusal, and probe-across-an-incarnation tests in `tests/lifecycle.rs`, `tests/activation.rs`, and `tests/host_roundtrip.rs` each carry a reachability marker or are enumerated by name against the executed test list, so a test that is removed, renamed, filtered, or marked `#[ignore]` while its binary still runs fails the check.
Fault/timing angle: none; a configuration fact.
Required faults and enabling state: none.
Confidence: high - [evidence](evidence/the-largest-lifecycle-proof-runs-in-ci.md). Verified in this tree by reading
`ci.yml:118` and `:126`, and by `cargo test --help`: `--workspace` tests every
workspace package and `--all-targets` includes every integration test target. The
source-tree history the evidence file records (four named binaries out of 26 at
authoring time, the `ed487e11` refactor adding `--test lifecycle`, PR #131 removing
the macOS jobs) describes the source repository's workflow, not this one.
Existing check: none - this record *is* the check.
Impact: 43 tests across 2543 lines, including the regression tests for ten repaired
lifecycle defects, would execute only when a developer runs them locally. In this
tree the gap is closed by construction: the workspace-wide test step runs every
binary, so the residual gaps the source catalog recorded (`activation.rs` and
`host_roundtrip.rs` unnamed; a `daemon` step's unrelated `lifecycle_cli`) do not
apply here. The record is retained because the gap was real in the source
repository and its closure is the evidence.
Open questions: None.

## Group I: setup-state transitions

Five records closing the gap the portfolio evaluation queued at
`portfolio-evaluation.md:96`: the setup state machine in `connection.rs` has four
in-code states against the eight the wire protocol's section 7.7 documents, and no
record covered the transitions. The first three are the gate itself, that
negotiation precedes every gated frame kind, that the selection is sticky for the
generation, and that readiness is decided by one predicate rather than by two
copies of it. The last two are the sharp edges: a `Pong` that the documentation
retires and the code requires inside the same window, and a fallback-reason
precedence rule that is test-only because no shipped configuration installs a
provider that can set a reason.

### negotiation-precedes-every-gated-frame-kind

Type: safety
Reachability: default-production when live - at `ed487e11^` every accepted
connection began in `BootstrapTcp` and the gate ran on every inbound frame.
Superseded rather than live: the mandatory-ring refactor removed transport
negotiation and the setup state machine, so no generation is ever in a
not-ready setup state at HEAD and the subject of this record is unreachable by
any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`, `ed487e11`) removed transport negotiation, the `TransportState` setup machine (`BootstrapTcp`, `CandidateSetup`, `transport_ready`, `handle_negotiate`), and `tests/transport_negotiation.rs`; no file under `crates/host-runtime` holds this mechanism at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - `tests/transport_negotiation.rs:906-949` covers four
channel-0 control operations, one routed request, and one oversize control
declaration before negotiation. `Cancel`, routed `Goodbye`, and `Pong` before
negotiation are uncovered, and no case asserts the absence of an emitted
terminal.
Guarantee: while the setup state is `BootstrapTcp` or `CandidateSetup`, no
routed request is dispatched, no cancel is applied, no routed goodbye begins a
close, no non-negotiate channel-0 control action is admitted, and no
oversize-rejection terminal is emitted; the generation retires instead.
Check: `always` - for every inbound frame, assert that
`transport_ready(setup) == false` implies the read loop returned
`ReadExit::Peer` without reaching `dispatch_request` (`connection.rs:486`),
`handle_cancel` (`:498`), `begin_close_owned` (`:566`), the control admission
block (`:659` onward), or `emit_authoritative_rejection` (`:454`). `always` and
not `unreachable`, because the forbidden thing is a *state pairing* (not ready,
yet dispatched) rather than a code location that must never execute: all five
sites are legitimate once ready.
Fault/timing angle: pipelining. The client may send the gated frame in the same
TCP segment as, or immediately after, its negotiate request. The state commits
at `:960` before the response is queued at `:961`, so the interesting window is
frames that arrive after `:960` but before the client could have read the
selection; those are legitimately accepted, and a test must not mistake them
for a gate failure.
Required faults and enabling state: a raw client that authenticates and then
sends one frame of each gated kind without negotiating. No provider, no
liveness, and no fault injection: `TestHost::start` plus `setup_client`
(`tests/support/mod.rs:688`) is sufficient. Reaching the `CandidateSetup` half
of the state predicate additionally needs an injected provider, which is
test-only; the `BootstrapTcp` half is the default path.
Confidence: high - [evidence](evidence/negotiation-precedes-every-gated-frame-kind.md).
All five
gate sites and all fourteen frame-kind arms enumerated against
`connection.rs:417-598` and `:626-647`.
Existing check: `tests/transport_negotiation.rs:906` covers six of the eight
gated shapes and asserts no bind occurred; it does not cover `Cancel`, routed
`Goodbye`, or the absence of a terminal on the oversize path.
Impact: any hole admits application-visible work on a connection whose
transport is not yet chosen, which is the one thing section 7.7 exists to
forbid. A routed dispatch there would run handler code the client can then be
told to reach over a different transport.
Open questions:
- Should a premature oversize control declaration receive the section 7.1
  authoritative terminal before retiring, or does the setup gate correctly
  outrank it? The code chooses the gate (`:430` refuses before any emission),
  while the malformed-negotiate path chooses the terminal (`:849-875`). The
  document specifies both rules and does not order them. (needs human input)

### setup-selection-is-sticky-for-the-generation

Type: safety
Reachability: default-production when live - at `ed487e11^` the `TcpCommitted`
arc needed no provider and no configuration. Superseded rather than live: the
mandatory-ring refactor removed negotiation and the `setup.state` and
`setup.handoff` writes it guarded, so no selection is ever made at HEAD and the
subject of this record is unreachable by any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`, `ed487e11`) removed transport negotiation, the `TransportState` setup machine (`BootstrapTcp`, `CandidateSetup`, `transport_ready`, `handle_negotiate`), and `tests/transport_negotiation.rs`; no file under `crates/host-runtime` holds this mechanism at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - `tests/transport_negotiation.rs:962-980` covers a repeated
negotiation after a negotiated TCP selection. Negotiation during
`CandidateSetup`, negotiation on a promoted `ProviderActive` generation, and
the at-most-one-candidate claim are uncovered.
Guarantee: at most one negotiation commits per generation. Once the state
leaves `BootstrapTcp` it never returns, at most one candidate handoff is ever
recorded, and any later or repeated negotiation retires the generation.
Check: `always` - instrument the two `setup.state` writes (`connection.rs:960`,
`:1103`) and the `setup.handoff` write (`:1104`); assert at most one of each
per `ConnectionSetup`, assert no write whose prior state is not `BootstrapTcp`,
and assert every `handle_negotiate` entry with a non-`BootstrapTcp` state
returns `ControlFlow::Close` (`:846-848`). `always` because stickiness is
evaluated on every negotiate frame, and a single violation hands one connection
two transports.
Fault/timing angle: none in the single-reader design. All three writes happen
on the sole read loop, so no interleaving can produce two selections. The
timing question is the reverse one: the state closes at `:960` and `:1103`
before the corresponding response is emitted, so retirement of a late
negotiation can be observed by the client before it observes the first
selection.
Required faults and enabling state: a raw client that negotiates twice. The
`TcpCommitted` arc needs no provider and no configuration. The `CandidateSetup`
and `ProviderActive` arcs need an injected provider, which is test-only; that
is why this record is labelled by its default-reachable arc and the provider
arcs are named explicitly.
Confidence: high - [evidence](evidence/setup-selection-is-sticky-for-the-generation.md).
`setup.state` has exactly two write sites and `setup.handoff` exactly one, all
enumerated by grep over `connection.rs`.
Existing check: `tests/transport_negotiation.rs:962`
`repeated_negotiation_after_negotiated_tcp_selection_retires` covers the
`TcpCommitted` arc only. Status `unaudited`.
Impact: a second accepted negotiation would prepare a second candidate,
overwrite `setup.handoff` at `:1104`, and leak the first candidate's sender,
cancellation root, and I/O task, because the setup owner reaps only what the
slot still holds (`:201-234`).
Open questions: None.

### setup-readiness-is-decided-by-one-predicate

Type: safety
Reachability: default-production when live - at `ed487e11^` both predicate
copies ran on every frame. Superseded rather than live: the mandatory-ring
refactor removed `TransportState` and both readiness predicates, so there is no
readiness definition to agree on at HEAD and the subject of this record is
unreachable by any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`, `ed487e11`) removed transport negotiation, the `TransportState` setup machine (`BootstrapTcp`, `CandidateSetup`, `transport_ready`, `handle_negotiate`), and `tests/transport_negotiation.rs`; no file under `crates/host-runtime` holds this mechanism at the pinned commit, so the subject is unreachable by any configuration.
Exercised: not yet - no test reads the setup state directly, and no lint or
test asserts that the two predicate copies agree.
Guarantee: exactly one definition decides whether a generation may carry
non-setup traffic, and every frame-kind gate consults that one definition, so a
new setup state cannot be ready for one frame class and not another.
Check: `always` - assert that the set of `TransportState` variants accepted by
`transport_ready` (`connection.rs:832-837`) equals the set accepted by the
inline `matches!` in `handle_control` (`:642-645`), and that no third readiness
test exists. Enforceable as a test over an extracted predicate, or mechanically
by deleting the inline copy and calling `transport_ready`; the property is the
agreement, not the shape. `always` because the two copies are evaluated
independently on every frame.
Fault/timing angle: none. This is a structural property of the source, and the
fault is a future edit rather than a runtime interleaving.
Required faults and enabling state: none for the current tree, which satisfies
the property. To make the check meaningful, add a fifth `TransportState`
variant in a test fixture, or assert both predicates over all variants; the
`matches!` shape means a new variant is refused by both copies unless a author
adds it, which is the fail-closed direction.
Confidence: high - [evidence](evidence/setup-readiness-is-decided-by-one-predicate.md).
Both
predicate bodies read at HEAD and confirmed textually identical over the same
field; the four `transport_ready` call sites and the one inline site
enumerated.
Existing check: none.
Impact: divergence makes the negotiation-first gate non-uniform for exactly one
frame class, which is the shape the `Pong` hole already has. This record is the
structural reason that hole is easy to create.
Open questions:
- Is the inline copy at `:642-645` deliberate, for example to keep
  `handle_control` independent of a `connection.rs`-private helper, or is it
  incidental duplication? (needs human input)

### a-setup-pong-is-required-and-forbidden-in-the-same-window

Type: reachability
Reachability: explicit-config-only when live - at `ed487e11^` the window needed
`liveness: Some(..)` in `HostConfig`. Superseded rather than live: the
mandatory-ring refactor removed the `BootstrapTcp` and `CandidateSetup` setup
states that defined the window, so the window cannot open at HEAD and the
subject of this record is unreachable by any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`, `ed487e11`) removed transport negotiation, the `TransportState` setup machine (`BootstrapTcp`, `CandidateSetup`, `transport_ready`, `handle_negotiate`), and `tests/transport_negotiation.rs`; no file under `crates/host-runtime` holds this mechanism at the pinned commit, so the subject is unreachable by any configuration.
Exercised: not yet - no test sends a `Pong` before negotiation, and no test
runs a configured liveness policy against a generation that has not yet
negotiated.
Guarantee: contested. `docs/host-wire-protocol.md:562` states that a `Pong`
retires the setup generation. `connection.rs:500-540` implements no readiness
gate in the `Pong` arm, and `connection.rs:291-302` starts liveness probing
before the read loop is awaited at `:304`, so with liveness configured the host
demands a `Pong` in exactly the window the document says a `Pong` must retire.
This record catalogues the contradiction with both sides cited and does not
choose between them.
Check: `sometimes` - assert that a campaign produces at least one interval in
which the setup state is `BootstrapTcp` or `CandidateSetup`, a host Ping is
outstanding in `gen.pings`, and a matching `Pong` is delivered to the read
loop. Marker `SETUP_PONG_WINDOW_OBSERVED`. `sometimes` and not `always`,
because asserting either resolution would resolve a normative question this
record exists to record: the honest check is that the window occurs, so a human
can decide against a real trace. Situation coverage, not location coverage: the
`Pong` arm's lines are already executed by post-negotiation tests while the
setup window is never produced.
Fault/timing angle: the whole record is a window. It opens when `liveness_loop`
is spawned at `:296` and closes when the state commits at `:960` or `:1103`.
Its width is `ping_interval` versus the client's time to negotiate, so a fast
client never sees a Ping and a slow or paused client always does. The grant
path narrows, but does not close, the window: it stops and joins bootstrap
liveness at `:1032-1036`, which is after any bootstrap Ping may already have
been sent.
Required faults and enabling state: `liveness: Some(..)` in `HostConfig`, which
is not a shipped configuration in this tree; the default is `None`
(`config.rs:296`) and the only `Some` is inside the test module
(`config.rs:664`). Plus a client that authenticates, delays negotiation past
`ping_interval`, and then answers the Ping. The weaker half of the
contradiction, that an *unsolicited* `Pong` before negotiation is silently
ignored rather than retiring the generation, needs no liveness at all and is
default-production; it is a strictly smaller claim and is recorded in the
evidence file.
Confidence: high - [evidence](evidence/a-setup-pong-is-required-and-forbidden-in-the-same-window.md).
Both sides read at HEAD: the document sentence, the ungated `Pong` arm, the
liveness start point, and the grant path's own comment explaining that
bootstrap probing is live during setup.
Existing check: none. `pong-preanswer-rejected-in-every-mutex-order` in this
catalog covers the `Pong` arm's acceptance rule, not its absent readiness gate.
Impact: unresolved, and that is the finding. Under the document's reading the
host kills healthy generations' probes by answering them; under the code's
reading the document forbids a frame the host itself solicits. A test author
picking either side without a decision would encode a guess as a regression
test.
Open questions:
- Should the document drop `Pong` from the retirement list at `:562`, or should
  liveness probing be deferred until a selection commits? (needs human input)
- If probing is deferred, does the setup deadline
  (`shared.timing.transport_setup_deadline`, used at `:1022`) fully replace
  liveness for detecting a peer that authenticates and then goes silent without
  negotiating? The bootstrap has no such deadline before a grant.

### fallback-reason-precedence-survives-a-silent-preflight

Type: safety
Reachability: test-only when live - at `ed487e11^` reaching the precedence block
needed injected providers. Superseded rather than live: the mandatory-ring
refactor removed transport negotiation, TCP fallback, and `TransportProviders`,
so no fallback reason is ever computed at HEAD and the subject of this record is
unreachable by any configuration.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`, `ed487e11`) removed transport negotiation, TCP fallback reasons, `TransportProviders`, `transport_provider.rs`, and `tests/transport_negotiation.rs`; no file under `crates/host-runtime` holds this mechanism at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - `tests/transport_negotiation.rs:876-905` covers capability
mismatch falling back and version mismatch retiring; `:851-874` covers an
unprovided non-TCP offer selecting reasonless TCP. No test produces
`DynamicallyUnavailable`, and no test pairs it with a mismatched sibling to
exercise the precedence.
Guarantee: when both conditions are present across the evaluated offers,
`unavailable` outranks `capability_version_mismatch`; a permanently absent
provider and a panicking preflight both contribute no reason and select
reasonless TCP.
Check: `always-or-unreached` - whenever a reason is computed
(`connection.rs:953-959`), assert `dynamically_unavailable` implies the emitted
reason is `Unavailable` regardless of `capability_mismatch`, and that a
`StaticallyOmitted` or panicking preflight contributes no reason. These
semantics and not `always`, because the path may never run in a shipped
configuration: with an empty provider registry both flags are unconditionally
false, so it must be correct when reached rather than reached at all.
Fault/timing angle: a panicking preflight is the fault of interest. It is
caught at `:907-912` and mapped to `StaticallyOmitted`, so a broken provider is
indistinguishable from one that was never installed, and the client is denied
the re-upgrade probe that `docs/host-wire-protocol.md:621` reserves for
`unavailable`. Ordering matters too: the loop breaks on the first serveable TCP
offer (`:892-896`), so offers after the TCP entry are never evaluated and
cannot contribute a reason.
Required faults and enabling state: at least two injected providers, one
returning `DynamicallyUnavailable` from `preflight` and one installed at a
capability version the client does not offer, with the client offering the
unavailable transport at lower preference than the mismatched one. Injected
providers are test-only: `TransportProviders::default()` is empty
(`transport_provider.rs:157-163`), the module documents providers as
test-injected (`:1-13`), and `HostConfig::default` installs the empty registry
(`config.rs:297`). Verified consequence: in every shipped configuration the
reason is always `None`.
Confidence: high - [evidence](evidence/fallback-reason-precedence-survives-a-silent-preflight.md).
Precedence block, preflight default, panic mapping, and the `serves_transport`
gate all read at HEAD; the empty-registry conclusion traced from
`HostConfig::default` to `TransportProviders::default`.
Existing check: `tests/transport_negotiation.rs:136`
`version_mismatches_encode_the_documented_tcp_fallback_reasons` covers the
encoders, and `:876` covers one live mismatch fallback. Neither covers
precedence between the two reasons. Status `unaudited`.
Impact: reporting a static mismatch where a transient condition exists
permanently suppresses the client's re-upgrade probe, so a transport that would
recover in seconds stays unused for the life of the connection. The panic
mapping has the same effect for a provider whose preflight is merely buggy.
Open questions:
- Should a panicking preflight be observably different from permanent absence,
  for example through a host-side event, given that the wire reason must stay
  reasonless per the KTD6 comment at `:906`? (needs human input)

## Group J: shared frame-read mechanics

Six records on `crates/host-runtime/src/frame_read.rs`, 125 lines of shared
host-and-client code whose own module doc named cancellation precedence, EOF
handling and frame-boundary capping as load-bearing while the file had no tests at
all. **The ring-transport refactor has since removed that file from the module
tree**: `ed487e11 refactor(host): make ring transport mandatory` deleted
`frame_read.rs`, so every line reference in this group resolves only at the
commit the lens pass read and the records carry `Status:
invalidated` rather than `active`. They are retained because the three
obligations did not disappear with the file. **Each must be re-derived against
the new read path** before it is treated as closed, and the two that were findings
rather than guarantees, the budget-free oversize drain and the unreachable
client-side refusal drain, are the two most likely to have moved rather than
gone.

### cancellation-preempts-every-bounded-frame-read

Type: safety
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: not yet - no in-crate test cancels a token while any `frame_read`
helper is running, so the precedence branch has no oracle anywhere. Integration
shutdown tests execute it incidentally and assert nothing about it.
Guarantee: When a bounded frame read's cancellation token is cancelled, the
read returns `ReadStop::Cancelled` and performs no further read, even when
input bytes are simultaneously available.
Check: `always` - with the token cancelled and a full read's worth of bytes
already buffered on the reader, each of `read_exact`, `read_body`, and `drain`
returns `Err(ReadStop::Cancelled)`, and the reader's unconsumed byte count is
unchanged. `always`, not `always-or-unreached`: cancellation fires on every
production shutdown and every generation retirement, so this is evaluated on
ordinary paths rather than on an optional one. Assert the byte count, not just
the return value, because a `Cancelled` return after a completed read is the
defect and the return value alone cannot see it.
Fault/timing angle: the window is exactly one `select!` poll in which both the
token and the read are ready. Under `biased` that is deterministic; without it,
tokio picks a random ready branch, so the defect appears about half the time
per iteration and a multi-iteration `read_exact` almost surely completes and
returns `Ok(())` on a cancelled generation.
Required faults and enabling state: a reader with bytes buffered and ready,
plus a token cancelled before the poll. Both are constructible with
`tokio::io::duplex`: write the bytes, cancel the token, then call the helper.
No scheduler control is needed because the ordering is established before the
call.
Confidence: high - [evidence](evidence/cancellation-preempts-every-bounded-frame-read.md).
Verified the `biased;` keyword and branch order at `frame_read.rs:47-49`,
`:81-83`, `:111-113`; the production cancellation sources at
`connection.rs:305`, `runtime.rs:1160`, and `runtime.rs:431`; and by exhaustive
`awk` over both test modules that no in-crate test cancels a read token.
Existing check: none. The 24 tests in `tcp_frame_channel.rs:512-1155` and the
two `read_active_frame` tests at `client.rs:3650` and `:3683` all construct a
fresh `CancellationToken::new()` and never cancel it.
Impact: a retiring generation keeps consuming frames. Each completed read
admits a frame and charges the ingress budget on a generation the host has
already decided to stop, which contradicts
[a-retired-generation-emits-nothing-and-mutates-nothing](#a-retired-generation-emits-nothing-and-mutates-nothing)
on the read side, and delays shutdown by up to one frame deadline per admitted
frame.
Open questions: None.

### a-body-read-consumes-exactly-the-declared-frame-boundary

Type: safety
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: not yet - no test passes a non-empty buffer, and no test asserts the
consumed-byte count rather than the buffer contents.
Guarantee: A successful `read_body(reader, buf, len, ..)` consumes exactly
`len` bytes from the reader: never more, so a pipelined next header cannot be
read as this frame's body, and never fewer, so the stream stays aligned.
Check: `always-or-unreached` - for an empty `buf`, assert both that
`buf.len() == len` and that exactly `len` bytes were consumed from the reader,
with a following pipelined header still intact and readable. For a non-empty
`buf` holding `k` bytes, either the call consumes `len` bytes or it must not
return `Ok`. `always-or-unreached` because both current call sites allocate a
fresh empty vector immediately before calling (`tcp_frame_channel.rs:217`,
`client.rs:2003`), so the non-empty case is unreached today while remaining
callable through the host's forwarding wrapper at
`tcp_frame_channel.rs:264-277`.
Fault/timing angle: none for the over-read direction; the `take` at
`frame_read.rs:79` makes it physically impossible. The under-read direction has
no timing window either. It is a pure precondition: the loop at
`frame_read.rs:80` tests `buf.len() < len`, an absolute buffer length, while
the cap at `:79` counts bytes read, so any non-empty incoming buffer makes the
two disagree and the call under-reads by exactly `k`, returning `Ok(())`.
Required faults and enabling state: no fault. Call `read_body` directly with a
buffer pre-filled with `k` bytes, `0 < k < len`, and a reader holding `len`
body bytes followed by a valid header. The observable is that the header no
longer parses on the next read.
Confidence: high - [evidence](evidence/a-body-read-consumes-exactly-the-declared-frame-boundary.md).
The cap and the loop condition were read at `frame_read.rs:79-80`; the
freshness of both callers' buffers was verified at `tcp_frame_channel.rs:217`
and `client.rs:2003`; the `take` semantics were confirmed against tokio 1.53.1.
Existing check: partial and indirect. `tcp_frame_channel.rs:836-896`,
`fragmented_and_coalesced_frames_preserve_alignment`, proves alignment survives
fragmentation and coalescing for empty buffers, which covers the over-read
direction end to end. Nothing covers the non-empty case. Status unaudited.
Impact: silent stream desynchronization with an `Ok` return. `k` body bytes are
parsed as the next header, so a peer's body content chooses the host's next
header, and the frame handed up contains `k` bytes that are not its body.
Open questions:
- Should `read_body` take `&mut Vec<u8>` at all? The client wrapper already
  owns its allocation (`client.rs:2003`), so only the host's forwarding wrapper
  needs the out-parameter. Either clearing `buf` on entry, asserting
  `buf.is_empty()`, or returning an owned `Vec` would make the disagreement
  unrepresentable rather than merely unreached. (needs human input)

### a-zero-length-read-ends-the-read-instead-of-looping

Type: safety
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - `read_body`'s EOF is proven by an exact-string assertion,
`read_exact`'s only by a wildcard that cannot discriminate the defect, and
`drain`'s only through the `RejectedDrainFailed` mapping.
Guarantee: A read returning zero bytes with the target unfilled terminates the
helper as `ReadStop::Eof` rather than continuing the loop.
Check: `always` - for each of the three helpers, close the peer mid-target and
assert the helper returns `Err(ReadStop::Eof)` promptly, well inside the
deadline. Assert the deadline is *not* consumed, because that is what separates
this property from a looping implementation: both return an error, and only the
timing and the error class distinguish them. `always`, since every orderly
mid-frame peer close reaches it.
Fault/timing angle: the whole property is about timing. A looping
implementation does not hang: a closed stream returns `Ok(0)` immediately and
forever, so it would spin hot until `timeout_at` fired and then report
`DeadlineExpired`, mapped by the host to `Corrupt("frame deadline expired")`
instead of `Corrupt("EOF inside frame")` (`tcp_frame_channel.rs:284-287`). So a
correct diagnosis costs a full frame deadline of spun CPU when it is wrong.
Required faults and enabling state: a mid-target peer close, three times: after
one header byte (`read_exact`), after a partial declared body (`read_body`),
and after a partial drained body (`drain`). All three are one `drop(client)` on
a `tokio::io::duplex` pair.
Confidence: high - [evidence](evidence/a-zero-length-read-ends-the-read-instead-of-looping.md).
Read the three `if read == 0` sites at `frame_read.rs:55-57`, `:89-91`,
`:119-121`, and verified the non-empty-target guards at `:46`, `:109-110`. The
`read_body` case needed dependency evidence, since `read_buf` has two other
`Ok(0)` causes: both were ruled out against
`tokio-1.53.1/src/io/util/read_buf.rs:46-48` and
`bytes-1.12.1/src/buf/buf_mut.rs:1599-1636` at the versions pinned in
`Cargo.lock`.
Existing check: three tests, of unequal strength.
`tcp_frame_channel.rs:599-618` asserts the exact string
`Corrupt("EOF inside frame")` and does discriminate a looping `read_body`.
`:580-597` asserts only `Corrupt(_)`, which a looping `read_exact` would also
satisfy. `:810-834` covers `drain` through `RejectedDrainFailed`, which
collapses every stop class into one variant (`connection.rs:401-410`) and so
cannot discriminate either. Status unaudited.
Impact: a hot spin for the whole frame deadline on every orderly mid-frame
close, plus a close reason that blames a slow peer for a clean disconnect.
Open questions:
- `read_body`'s EOF detection depends on `bytes`' `BufMut for Vec<u8>` growing
  on demand. Should that dependency be pinned by a comment or an assertion at
  `frame_read.rs:89`? A buffer type with fixed remaining capacity would
  silently reclassify "buffer full" as "peer closed". (needs human input)

### no-framed-read-resumes-after-a-read-stop

Type: safety
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - the host's obligation is structurally enforced by an
exhaustive match, which is stronger than a test; the client's is enforced by
code inspection only, and no test drives a stop and then attempts a second
read.
Guarantee: After any `ReadStop`, no caller performs another framed read on the
same reader. All three helpers abandon partially consumed frames and keep no
offset, so a resumed read would begin mid-frame.
Check: `always` - for every `Err` exit of the host's `recv` and the client's
`read_active_frame`, the enclosing loop terminates without calling the reader
again. The compile-time form is stronger and already present on the host side:
the `ReadClose` match at `connection.rs:398-415` is exhaustive with every arm
returning, so a new variant cannot be handled by falling through. `always`
rather than `unreachable`, because the forbidden thing is a *state* (a second
read after a stop) reached from many call sites, not one code location.
Fault/timing angle: the sharpest case is the transport layer rather than a
caller. `recv` takes `pending_drain` at `tcp_frame_channel.rs:97` before
running the drain, so a failed drain both leaves the stream misaligned and
clears the state that would realign it. A retry of `recv` after
`Err(RejectedDrainFailed)` would read body bytes as a header with no pending
drain left. `connection.rs:401-410` is the only thing preventing that.
Required faults and enabling state: any stop class, then an attempted second
read. Cheapest construction: a truncated declared body for EOF, a paused clock
for the deadline, a cancelled token for cancellation.
Confidence: high - [evidence](evidence/no-framed-read-resumes-after-a-read-stop.md).
Verified that
no helper retains an offset (`frame_read.rs:45`, `:79`, `:108`), and that both
callers stop: the host's four exhaustive `Err` arms at `connection.rs:400`,
`:401-410`, `:411-414`, and the client's `break` at `client.rs:1897-1900` plus
its clean-EOF `break` at `:1894-1897`.
Existing check: `connection.rs:398-415` is a production structural guard rather
than a test, and covers the host completely. The client has neither a test nor
a guard, only `reader_loop`'s shape. Status unaudited.
Impact: a resumed read parses body bytes as a header. Every downstream identity
decision - correlation, channel, epoch, frame type - is then made from
attacker-chosen bytes on a stream the host believes is aligned.
Open questions:
- The obligation is not written down. `frame_read.rs:5-8` assigns short-read
  *policy* to the callers but never states that no resume is permitted. Should
  the module doc state it, given that the type system cannot? (needs human
  input)

### oversize-control-drain-work-is-bounded-without-ingress-budget

Type: safety
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: partial - the zero-budget property is asserted, its cost bound is
not. `tcp_frame_channel.rs:803-807` proves an oversize declaration holds no
ingress budget; nothing bounds how much read-and-discard work one peer can
provoke.
Guarantee: An early-rejected oversize control frame is drained without charging
the ingress byte budget, and the work it costs is bounded by the rejected
frame's own absolute frame deadline, by the connection permit semaphore, and by
nothing else.
Check: `always` - across a sustained reject-then-drain cycle, the ingress
budget's available permits return to their starting value after every cycle
(the intended property, already asserted once), and each drain completes or
fails within the deadline armed at the rejected frame's first header byte.
State the bound in the units the code bounds: the absolute deadline from
`tcp_frame_channel.rs:169`, carried into `PendingDrain` at `:126`, default 30 s
(`config.rs:224`); and `max_connections` concurrent loops, default 64
(`config.rs:129`, `runtime.rs:890`). Do not assert a byte-rate ceiling, because
there is none.
Fault/timing angle: the deadline is armed at the *first header byte*, not at
the drain's start, and is spent by the header read and by the engine's
rejection emission before the drain begins (`connection.rs:433-462` spawns the
emission between the two `recv` calls). So a host slow to emit shortens its own
drain window and converts the drain into `RejectedDrainFailed`, which closes
the generation. That makes the loop self-limiting under host slowness but not
under host health.
Required faults and enabling state: a peer sending channel-0 `Request` headers
with strictly increasing correlations (`connection.rs:426-429`) declaring
`len > MAX_CONTROL_BODY_LEN`, each followed by the declared bytes, repeated. To
observe the aggregate, run `max_connections` of them.
Confidence: high on the mechanism, medium on whether the cost is a defect - [evidence](evidence/oversize-control-drain-work-is-bounded-without-ingress-budget.md).
Verified that the oversize branch at `tcp_frame_channel.rs:198-202` precedes
the budget charge at `:204-215`; that the ceiling is `MAX_BODY_LEN` and not the
control cap, because `validate_inbound_header` (`frame_channel.rs:58-61`) runs
first at `:196` (`wire.rs:35`, `:371`, `:374`); and that the only permits in
the path are `connection_permits` (`connection.rs:165-169`) and `busy_rejects`
(`:443-447`, cap 32 at `:53`), the latter bounding the emissions rather than
the drain, exactly as its comment at `:437-442` says.
Existing check: `tcp_frame_channel.rs:772-808` covers one reject-then-drain
cycle and asserts the zero-budget property at `:803-807` with a deliberately
tiny budget. `:730-770` covers realignment. Neither repeats the cycle, and no
check bounds the cost. Status unaudited.
Impact: a peer can force up to 64 MiB of read-and-discard per frame per
connection, and up to `max_connections` of those concurrently, while holding no
resident-byte budget. Those bytes are also unobserved: `drain` never touches
the `CopyCounter`, whose only producers are `frame_channel.rs:376` and
`shm_provider.rs:611`, so no counter in the crate sees them.
Open questions:
- Is the unbudgeted cost acceptable, or should the drain hold a nominal charge
  or a dedicated permit? The zero-budget property is deliberate and correct as
  a *resident-memory* property (never buffer the body); the open question is
  whether the same reasoning should extend to the bandwidth and CPU the discard
  costs. (needs human input)
- Should the reject-then-drain cycle be counted? The gap here is a missing
  bound, and today no signal would show the loop running at all. (needs human
  input)

### the-client-body-budget-refusal-drain-is-never-entered

Type: reachability
Reachability: default-production when live - at `ed487e11^` `frame_read.rs` sat on the
unconditional host and client read paths. Superseded rather than live: the mandatory-ring
refactor deleted `frame_read.rs`, so the subject of this record is unreachable at HEAD.
Status: invalidated
Invalidated: the mandatory-ring refactor (`host@907746f7b`) removed this mechanism; no file under `crates/host-runtime/src` holds it at the pinned commit, so the subject is unreachable by any configuration.
Exercised: not yet - nothing observes the branch, so it could start firing
without notice.
Guarantee: The client's inbound read reservation is exclusively the reader's
and sized to the framing maximum, so the budget-refusal branch at
`client.rs:1970-1973` never executes.
Check: `unreachable` - the `else` arm at `client.rs:1970` must not be entered.
`unreachable` and not `always(!X)` because this is one specific code location
with a natural detection point, exactly the case the semantics table reserves
`unreachable` for. Entering it means the reservation is no longer
reader-exclusive: either a `ByteCharge` outlived its `dispatch` call, or the
cap and `MAX_BODY_LEN` diverged. A `debug_assert!(false)` or a counter on that
arm is the whole check.
Fault/timing angle: none today. The reservation is released synchronously
inside `dispatch` before the next read (`client.rs:1393` called from `:1903`),
so no interleaving can leave `used > 0` at a read. The branch becomes reachable
the moment a `dispatch` arm retains a read charge across an await, or a stream
item borrows the read charge instead of `retained_budget` (`:1523`).
Required faults and enabling state: to *prove* unreachability, none: it follows
from the four verified steps in the evidence. To detect a regression,
instrument the arm and run the ordinary inbound suite.
Confidence: high - [evidence](evidence/the-client-body-budget-refusal-drain-is-never-entered.md).
Verified all four steps the claim rests on: the cap equals the framing maximum
(`client.rs:88`, `:403`); `validate_inbound` rejects a larger `len` first
(`:2040`, called at `:1957`); `ByteCounter::charge` refuses only when
`used > 0` or on overflow (`:1770-1781`); and `used` is zero at every read
because every `dispatch` arm releases the charge (`:1433`, `:1530`, and
ownership drop at `:1431`, `:1479`, and function exit) while retained stream
bytes go to `retained_budget` (`:1523`, documented at `:956-961`).
Existing check: none. The comment at `client.rs:1965-1969` states the invariant
in prose and correctly labels the branch as a structural guard, but nothing
enforces or observes it.
Impact: low today and that is the finding. Two dead claims hang off it:
`drain_until`'s doc at `client.rs:2010-2011` promises a realignment that no
code consumes, and even if the branch fired, `read_active_frame` returns
`Err(())` immediately after, which retires the connection at `:1897-1899` and
discards the realignment. If the branch ever does fire it signals a real
regression in the reader-exclusive reservation, and nothing would report it.
Open questions:
- Should the branch keep its drain, or return the error directly? The drain's
  only stated purpose is a realignment the sole caller discards. Removing it
  would delete the client's only `frame_read::drain` call site and make the
  guard a bare error return. (needs human input)

## Group K: configured liveness, manifest evolution, and platform

Six records closing the remaining three queued gaps. The first two are normal
configured liveness, where the loop's bound is three code-stated quantities rather
than an unbounded "eventually", and where slow egress alone must not retire a
probed generation; both are `explicit-config-only`, because `LivenessPolicy`
defaults to `None` (`config.rs:296`) and nothing in this crate opts in. The next
two are canonical manifest evolution, the golden vector that pins the bytes and
the digest, and the declaration-order change that must not orphan a retained
generation. The last two are the platform pair: an atomic directory exchange
whose macOS arm is dead text with no CI executor now that `ci.yml` is
Linux-only after PR #131 (merge `5d638e3e8`), and a portable rename fallback
that cannot deliver
the guarantee its caller documents. The lens pass proposed folding the first two
into Group B and the last four into Group F; they are kept together here because
the index and the deferred-candidate list already treat the gap-closure set as
three groups, and splitting them would renumber records that other artifacts
reference.

### a-timely-pong-sustains-the-generation-within-a-bounded-round

Type: liveness
Reachability: explicit-config-only
Status: active
Exercised: partial - `tests/client.rs:97-145` keeps a generation alive across
roughly seven ping intervals with a real answering client, but asserts only
that an unrelated unary request later succeeds. It observes no Ping, no Pong,
and no retirement, so it passes unchanged if the probe never runs at all. No
test covers the retirement direction.
Guarantee: For a configured policy, a peer that answers each Ping within
`pong_deadline` of that Ping's write completion keeps its generation
uncancelled indefinitely; and when `invalidate_on_missed` is set, a peer that
does not answer has its generation cancelled within one `pong_deadline` of
write completion.
Check: `always` - under paused virtual time, for a policy with a chosen
`ping_interval` and `pong_deadline`: (a) answer every Ping strictly inside
`pong_deadline` measured from the write-completion instant recorded at
`connection.rs:816`, advance the clock by `k * ping_interval` for a fixed small `k` and assert exactly
`k` Pings were written (the loop rearms `next_ping_at` after every send at
`connection.rs:779`, so the cadence count is taken over the requested intervals
only), then advance a further `pong_deadline` and assert `gen.token` is not
cancelled without asserting the count, since `pong_deadline >= ping_interval` is
a valid configuration (`config.rs:304-317`) under which that tail produces more
Pings, and additionally assert the per-round inductive invariant that
makes the finite run stand for the unbounded guarantee: after each timely Pong the
liveness state carries no counter, deadline, or flag that depends on how many
rounds have passed (the `expired` predicate at `connection.rs:755-760` reads only
each outstanding probe's `written_at` and `sent`, and an answered probe is removed
from `pings`), so round `k + 1` is decided by the
same inputs as round 1; (b) with `invalidate_on_missed: true`, answer nothing,
advance to `write_completion + pong_deadline`, and assert `gen.token` is
cancelled; and assert it is *not* cancelled at `write_completion +
pong_deadline
- 1ns`. `always` because the two directions are the dual outcomes of one
  predicate, `expired` at `connection.rs:755-760`, and both must hold at
  every evaluation of the loop.
Fault/timing angle: the bound is stated in the units the code bounds, so this
is a finite check rather than an unbounded "eventually". The wake is the
minimum of the next tick and the earliest `probe.sent + pong_deadline`
(`connection.rs:1355-1364`); expiry is `>= pong_deadline` from `probe.sent`
(`:1370-1373`); the tick re-arms at `now + ping_interval` (`:1399`).
`config.rs:370-382` rejects a zero value for either, so both bounds are
strictly positive in any accepted configuration. The subtle part is which
instant `probe.sent` holds: the insert at `:1403-1411` records the enqueue
instant with `written_at: None`, and the write-completion hook at `:1421-1447`
overwrites it with `completed_at`. Probes with `written_at: None` are excluded
from both the deadline wake (`:1358`) and the expiry scan (`:1372`), so
queueing delay neither expires a probe nor arms one. Both halves need paused
time; wall-clock sleeps cannot distinguish the boundary from scheduler noise.
Required faults and enabling state: a configured `LivenessPolicy`, which no
shipped configuration supplies. For (a) a cooperative peer, which the in-crate
duplex harness at `connection.rs:1480` onward already provides. For (b) a peer
that reads but never sends a Pong, plus `invalidate_on_missed: true`. Paused
tokio time for both. No adversary and no concurrency campaign.
Confidence: high - [evidence](evidence/a-timely-pong-sustains-the-generation-within-a-bounded-round.md).
Every bound was read at HEAD and the two `sent` anchors were traced through
both writers of the field.
Existing check: partial. `tests/client.rs:97-145` covers direction (a) with an
indirect oracle, and is the only place in the crate where a full client answers
a host Ping. `tests/lifecycle.rs:468` covers an *unmatched* Pong, not a missed
one. Nothing covers direction (b).
Impact: the probe exists to detect a peer that has stopped reading. If (a)
fails, healthy long-running work is killed, which is the exact outcome
`config.rs:236-238` cites as the reason `invalidate_on_missed` defaults to
`false`. If (b) fails, a dead peer holds a generation, its route registrations,
and its egress budget for the life of the process.
Open questions:
- Should the retirement bound be stated from write completion or from Ping
  issuance? The code anchors on completion (`:1443`), so a Ping stuck in the
  queue extends the wall-clock time to retirement without bound while keeping
  the `pong_deadline` bound intact. That is the intended anchor per `:528-534`,
  but it means no bound exists on *total* time to detect a dead peer.

### slow-egress-alone-does-not-retire-a-probed-generation

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - no test fills the writer queue while liveness is
configured.
Guarantee: Application egress backpressure, on its own, never retires a
generation whose peer is answering Pings, and in particular never retires one
when `invalidate_on_missed` is `false`.
Check: `always` - in every window in which the coverage preconditions below hold, the generation is not retired and `probe.expired` stays false until the peer's Pong arrives, with `invalidate_on_missed` false; the catalog's own trace says `FrameSender::send` retires the generation when the writer queue stays saturated through the Ping admission deadline, so this is a predicted violation at HEAD rather than a pass. The two markers below are coverage instrumentation that shows the window was constructed, not the assertion. A constant marker `probe_queued_behind_saturated_egress`
fires when all of these independent, legal preconditions hold at one instant: a
`LivenessPolicy` is configured; `invalidate_on_missed` is `false`; the writer
queue holds `writer_queue_frames` admitted frames; and a Ping tick is due. A
second constant marker `pong_parked_pending_write_completion` fires when a
matching Pong is observed while `probe.written_at.is_none()`, which is the park
branch at `connection.rs:464`. The markers use situation coverage rather than line coverage because a
campaign can execute those lines while never producing the operational state:
line coverage of `:464` proves the branch compiled and ran, whereas the
property needs a full queue coinciding with a due tick, which is situation
coverage. Both markers assert only legal preconditions, so they still fire
against a correct implementation; neither asserts a retirement, an expiry, or
any violation.
Fault/timing angle: the window exists because the host writer has no control
lane. `frame_channel.rs:761` holds a single
`mpsc::Sender<QueuedOutboundFrame>`, created at `:862` with capacity
`queue_frames`, default 64 (`config.rs:141`, passed at `connection.rs:181`).
There is no reserved control slot, unlike the client, which reserves one
(`client.rs:954`). Two distinct consequences follow, and only the first is
handled. First, a Ping queued behind application frames is deadline-anchored at
completion (`connection.rs:1443`) and its Pong is parked rather than judged
(`:528-535`), so queueing delay is not charged to the peer. That is correct and
deliberate. Second, and unhandled: the Ping's *admission* is bounded.
`gen.writer.send(...)` at `:1449` reaches `FrameSender::send`
(`frame_channel.rs:779-781`), which passes `admission_deadline()`, that is
`now + admission_timeout` (`:783-785`), and `connection.rs:178-186` supplies
`shared.timing.frame_deadline` there, default 30 seconds (`config.rs:224`). The
timeout arm at `frame_channel.rs:819-823` calls `self.retired.cancel()` and
`self.generation.cancel()`. So a Ping that cannot be admitted within
`frame_deadline` retires the generation from inside the sender, before
`liveness_loop` observes `sent.is_err()` at `:1457`. This bypasses
`invalidate_on_missed` entirely: the missed-Pong retirement at `:1376` is gated
on that flag and the admission retirement is not.
Required faults and enabling state: a configured `LivenessPolicy` with
`invalidate_on_missed: false`; a handler producing frames faster than the peer
drains them so all 64 slots stay occupied; and a peer that keeps reading fast
enough that no single dequeued write exceeds `frame_deadline`, since a peer
that stops reading is retired by the write deadline on its own, which
`config.rs:204-206` documents as intended. Paused time makes the 30 second
window cheap. The second marker needs only a Ping enqueued behind at least one
unwritten frame plus a prompt Pong.
Confidence: high - [evidence](evidence/slow-egress-alone-does-not-retire-a-probed-generation.md).
The admission path was traced from the Ping send through the cancel calls, and
the absence of a host-side control lane was confirmed by reading the whole
`FrameSender` and its constructor.
Existing check: none. `tests/client.rs:97-145` is named
`stream_order_and_slow_consumer_do_not_block_ping_or_unary`, but its
`stream_then_hang` handler (`tests/support/mod.rs:492-501`) streams two items
that the client consumes before the observed window, so the queue is empty
throughout and there is no slow consumer. The hang is in the handler.
Impact: the documented safety valve does not hold. `config.rs:236-238` states
that `invalidate_on_missed` stays `false` because enabling it "would kill
healthy long-running awaits (protocol §9.3)". An embedder that configures a
policy with the flag off, believing invalidation is disabled, still loses
generations to egress backpressure. The failure looks like a transport reset to
both sides, and per `authentication-and-capacity-rejections-are-observable`
there is no channel to report it.
Open questions:
- Is retiring on Ping admission timeout intended? The admission timeout is a
  general frame-channel policy and the Ping is an ordinary caller of it, so
  this reads as an unnoticed interaction rather than a decision. If it is
  intended, `invalidate_on_missed`'s doc comment overstates what the flag
  disables. (needs human input)
- Should the host reserve a control slot as the client does? That would remove
  the interaction rather than document it, but it changes the queue accounting
  the ingress budget depends on.

### manifest-canonical-bytes-and-digest-are-pinned-by-a-full-golden-vector

Type: safety
Reachability: test-only - `GenerationStore::stage_and_promote` has no caller
outside `crates/host-runtime` tests in this tree (workspace-wide search); the
daemon CLI that promotes on every start (`crates/daemon`) is scheduled for U4
(`docs/properties/README.md:52`); reclassify to `default-production` in the wave
that lands it.
Status: active
Exercised: partial - one byte-exact golden vector exists
(`generation.rs:1395-1412`) but its fixture omits the optional field and
carries an empty `files`, so it pins neither the optional field's position nor
`ManifestFile`'s field order, and it asserts no digest.
Guarantee: The canonical manifest encoding is fixed by a golden vector that
covers every field in the schema, so no change to declaration order can alter a
generation's bytes or its digest without a test failing.
Check: `always` - a golden test asserts a hardcoded byte string and a hardcoded
lowercase-hex SHA-256 for a fully populated `GenerationManifest`: all four
leading scalars, `source_payload_manifest_sha256` present as `Some`, and
`files` holding at least two entries so sortedness is also pinned. Assert both
`manifest.canonical_bytes() == GOLDEN_BYTES` and
`manifest.digest() == GOLDEN_DIGEST`, and assert the fixture round-trips by
decoding `GOLDEN_BYTES` and re-encoding. `always` because the encoding is
evaluated on every stage and every validate; there is no configuration in which
it is unreached.
Fault/timing angle: none. This is a static encoding contract. The mechanism is
that `canonical_bytes` is `serde_json::to_vec(self)` (`generation.rs:172-174`),
so the byte order is literally the declaration order at `:153-168` for the
outer struct and `:141-144` for each file entry, and `digest` is the SHA-256 of
exactly those bytes (`:176-178`). The optional field's
`skip_serializing_if = "Option::is_none"` at `:166` is what makes the existing
vector blind to its position: with the field absent, moving it changes nothing
about the fixture's bytes.
Required faults and enabling state: none. The check is a pure unit test with no
store, no filesystem, and no fault injection. It is the cheapest record in this
pass.
Confidence: high - [evidence](evidence/manifest-canonical-bytes-and-digest-are-pinned-by-a-full-golden-vector.md).
Both structs, both encoding functions, and the existing fixture were read at
HEAD, and the three blind spots were each confirmed by reasoning from the
fixture's literal contents.
Existing check: `generation.rs:1395-1412`
`a_generation_staged_before_the_source_digest_field_still_decodes` asserts
`decoded.canonical_bytes() == predecessor` against the literal at `:1401`. It
is the only byte-exact manifest vector in the crate and it does pin the
relative order of the four leading scalars and `files`, and it does catch the
addition of a required field, because `deny_unknown_fields` plus a missing
field fails the decode. Status unaudited.
Impact: declaration order is a wire-compatibility contract that is currently
enforced by a fixture that cannot see two thirds of it. A maintainer who
inserts `source_payload_manifest_sha256` beside the other hashes for
readability, or who alphabetizes `ManifestFile`, changes the digest of every
generation with files and sees a green suite.
Open questions: None.

### a-declaration-order-change-cannot-orphan-a-retained-generation

Type: safety
Reachability: test-only - `GenerationStore::stage_and_promote` has no caller
outside `crates/host-runtime` tests in this tree (workspace-wide search); the
daemon CLI that promotes on every start (`crates/daemon`) is scheduled for U4
(`docs/properties/README.md:52`); reclassify to `default-production` in the wave
that lands it.
Status: active
Exercised: not yet - no test validates a manifest written under one declaration
order against a binary using another.
Guarantee: A retained generation staged by an earlier release keeps validating,
and keeps its directory name, under any later release of the same schema
number.
Check: `always` - for a manifest fixture whose bytes were produced under the
previous declaration order, `store.validate(old_digest)` succeeds under the
current binary. Equivalently, and cheaper: for every field of
`GenerationManifest` and `ManifestFile`, a permutation of the declaration order
that leaves the existing golden test green must be detectable by some check.
`always` because validation runs on the default start path and the schema
number does not change when a field moves, so there is no version in which the
obligation lapses.
Fault/timing angle: none, but the failure shape needed correcting. The break is
**fail-closed, not silent**. `validate_in_dir` enforces the binding twice
(`generation.rs:636-648`): first `hex(sha256(bytes)) != digest`, then
`manifest.canonical_bytes() != bytes`. Under a reordered struct the on-disk
bytes still hash to the directory name, so the first check passes, and the
re-encode of the decoded manifest differs, so the second fails with
`invalid("manifest is not canonically encoded")`. The comment at `:640-647`
explains why that second check exists: without it one logical manifest would
have two identities. So reordering produces a refusal of intact payloads, which
is precisely the outcome the `source_payload_manifest_sha256` doc comment at
`:157-165` was written to avoid, reached by a different route. The second,
independent consequence is that staging the same logical content under the
reordered struct computes a different digest, so it lands in a new directory
and content-addressed deduplication stops deduplicating without any error at
all.
Required faults and enabling state: none at runtime. Constructing the check
needs a fixture manifest whose bytes encode an older field order, which is a
string literal, plus a staged directory whose files match it. No fault
injection.
Confidence: high - [evidence](evidence/a-declaration-order-change-cannot-orphan-a-retained-generation.md).
Both equality checks were read at HEAD, and the fail-closed conclusion was
derived from them rather than assumed; the prompt's "silently change every
retained generation's digest" is corrected in the evidence file.
Existing check: partial and narrow. The predecessor fixture at
`generation.rs:1395-1412` is the only cross-version manifest test, and it
covers one specific evolution, a field added at the end and omitted when
absent. Nothing covers a field that moves.
Impact: the first start after an upgrade reports `native_payload_invalid` for
every retained generation carrying the moved field, refusing payloads that are
byte-for-byte intact. That is the forward-compatibility break the `Option` on
`source_payload_manifest_sha256` was introduced to prevent.
Open questions:
- Should the canonical encoding be decoupled from declaration order, for
  example by an explicit field-order list or a canonical-JSON serializer, so
  the contract is stated once rather than implied by the struct? The current
  design makes an ordinary refactor a compatibility break. (needs human input)

### the-atomic-directory-exchange-is-atomic-on-every-supported-platform

Type: safety
Reachability: test-only - `GenerationStore::stage_and_promote` has no caller
outside `crates/host-runtime` tests in this tree (workspace-wide search); the
daemon CLI that promotes on every start (`crates/daemon`) is scheduled for U4
(`docs/properties/README.md:52`); reclassify to `default-production` in the wave
that lands it.
Status: active
Exercised: partial - the Linux arm is covered by one test through `promote_temp`
(`same_digest_corrupt_target_is_repaired_only_by_validated_exchange`,
`generation.rs:1526`, re-located at HEAD); the macOS arm and the non-Linux
non-macOS stub have never executed under observation; after PR #131 (merge `5d638e3e8`) `ci.yml` has no macOS job,
so no CI executor for that arm exists.
Guarantee: Wherever the digest-target exchange runs, the two names are swapped
atomically inside one directory, or an error is returned and neither name is
left unoccupied; on a platform without the primitive it fails closed.
Check: `always-or-unreached` - for each supported platform, drive
`promote_temp` into the exchange branch and branch on the result: on `Ok`,
assert the digest name holds the validated candidate and the temp name holds
the displaced corrupt orphan; on `Err`, assert both names are still occupied by
the bytes they held before the call, so the fail-closed outcome the guarantee
permits passes and only a lost or deleted name fails. In neither case is there an
observable state in which either name is absent. Also assert the non-Linux
non-macOS stub returns an error rather than succeeding.
`always-or-unreached` because the branch is optional: it is entered only when
the digest target is occupied by a corrupt unprotected generation
(`generation.rs:751-770`), so a run that never meets that condition owes
nothing and the check must not fail.
Fault/timing angle: the risk is platform divergence behind one expression.
`generation.rs:1191-1198` gives Linux and macOS a shared arm calling
`rustix::fs::renameat_with(.., RenameFlags::EXCHANGE)`, and its own doc comment
at `:1191-1193` records that this is `renameat2(RENAME_EXCHANGE)` on Linux and
`renameatx_np(RENAME_SWAP)` on macOS. Those are two different kernel interfaces
with independent semantics, error sets, and filesystem support, reached through
one line of Rust. The call site at `:905` sits between an exchange and a
revalidate-then-delete (`:907-910`), so a non-atomic or partially-applied
exchange deletes the wrong directory. `promote_temp`'s contract at `:865-876`
depends on the exchange being all-or-nothing.
Required faults and enabling state: a corrupt, unprotected generation already
occupying the digest target, plus a restage of the same digest. The existing
test `same_digest_corrupt_target_is_repaired_only_by_validated_exchange`
(`generation.rs:1689`) already builds that fixture, so the fault is available;
the missing element is executing it on macOS. On the stub platforms the check
is a compile-and-call assertion.
Confidence: high - [evidence](evidence/the-atomic-directory-exchange-is-atomic-on-every-supported-platform.md).
Both cfg arms and the call site were read at the authoring pass's HEAD, along
with the whole macOS CI job as it then existed. PR #131 (merge `5d638e3e8`)
since deleted that job with every other macOS job, so the claim that no macOS
lifecycle or generation test executes now holds trivially: `ci.yml` at HEAD
contains only `ubuntu-latest` jobs.
Existing check: partial, Linux only. `generation.rs:1526` (re-located at HEAD)
`same_digest_corrupt_target_is_repaired_only_by_validated_exchange` drives the
branch. The macOS CI job this entry previously described was removed by
PR #131 (merge `5d638e3e8`); no `generation` or `lifecycle` test body runs on
macOS because nothing runs on macOS. Status unaudited.
Impact: the exchange is the store's only repair primitive for a corrupt
occupant, and it is immediately followed by a deletion. If macOS `RENAME_SWAP`
behaves differently on APFS than `RENAME_EXCHANGE` on ext4, the failure mode is
deleting a retained generation, on a platform whose lifecycle code the suite
never runs.
Open questions:
- Is macOS a supported deployment target for the lifecycle store, or only a
  development platform? The cfg arm still carries the macOS path in source,
  but after PR #131 (merge `5d638e3e8`) no CI job builds or runs it. That
  decision sets whether this record is a real gap or a documentation fix.
  (needs human input)
- Is Darwin still a supported release surface? (needs human input)

### an-occupied-rename-target-is-never-replaced-on-the-portable-path

Type: safety
Reachability: test-only - `GenerationStore::stage_and_promote` has no caller
outside `crates/host-runtime` tests in this tree (workspace-wide search); the
daemon CLI that promotes on every start (`crates/daemon`) is scheduled for U4
(`docs/properties/README.md:52`); reclassify to `default-production` in the wave
that lands it.
Status: active
Exercised: not yet - the branch is unreachable on Linux without a filesystem
that rejects `renameat2` flags, and it is the only path on macOS, where no test
in this scope runs.
Guarantee: `rename_no_replace` never replaces an occupied target, on any
platform and for any occupant, including an empty directory.
Check: `always` - plant a directory at `to`, call `rename_no_replace`, and
assert it returns `Ok(false)` and leaves both names as they were. Run the
assertion for an empty occupant and a nonempty occupant, on both the flagged
Linux path and the portable path, forcing the portable path either by a
filesystem that rejects `renameat2` flags or by extracting the fallback so it
can be called directly. On the portable path also close the check-then-act
window: with a failpoint between the `statat` that reports `NOENT`
(`generation.rs:1062-1064`) and the plain `renameat` (`:1067`), create the target,
as an empty directory and as a file, while the call is parked, and assert the
rename does not replace it; pre-planting the target alone tests occupancy before
the window and cannot fail on the race the guarantee forbids. `always` because the caller's contract at
`generation.rs:744` is unconditional, so an occupied target that gets
replaced is a violation rather than a tolerated case.
Fault/timing angle: this is a check-then-act window that is dead on one
platform and load-bearing on another. `generation.rs:1216-1242`: the Linux
block at `:1217-1230` returns on `Ok`, `EXIST`, `NOTEMPTY`, and `NOSPC`,
falling through only on `INVAL`, `NOSYS`, or `OPNOTSUPP`, so on a current
kernel with ext4, tmpfs, or overlayfs the fallback never runs. On macOS that
block is absent by cfg, so `statat` at `:1231-1235` followed by `renameat` at
`:1236-1241` is the only path. POSIX `rename` replaces an existing *empty*
directory, which `promote_temp`'s comment at `:868-871` calls out by name as
the thing that must not happen, so the flagged path and the fallback do not
implement the same guarantee. The fallback's justification at `:1211-1215` is a
prose claim that `transaction.lock` (`lifecycle.rs:44`, `:495-496`) excludes
concurrent creation. That is a claim about actors inside the trust model only,
and the same file defends against out-of-model directory replacement elsewhere:
the walk comment at `:669-678` describes exactly that attack, and the catalog's
`validation-and-enumeration-address-one-directory-object` records two shipped
defects from the class.
Required faults and enabling state: a directory appearing at the target between
the `statat` and the `renameat`. Deterministically: a failpoint between the two
calls, or an extracted fallback called with the target pre-planted. Reaching
the fallback on Linux at all needs a filesystem that rejects `renameat2` flags;
running it as the default needs macOS.
Confidence: high - [evidence](evidence/an-occupied-rename-target-is-never-replaced-on-the-portable-path.md). On the mechanism, medium on severity, since the transaction
lock does exclude the in-model actors and no out-of-model writer is
demonstrated.
Both branches, the caller's contract, and the lock references were read at
HEAD.
Existing check: none. No test drives the portable fallback on any platform, and
no test plants an empty directory at a digest target.
Impact: the guarantee `promote_temp` documents as mandatory, that a protected
occupant corrupted into an empty directory is never silently destroyed, is
enforced by the kernel on Linux and by a prose argument on macOS. A defect here
destroys a retained generation, which is the outcome the protection check
exists to prevent.
Open questions:
- Does anything outside the trust model have write access to the generations
  directory in a real deployment? The lock argument is sound if and only if the
  answer is no, and the store's validation path assumes the answer is yes.
  (needs human input)
- Should the fallback be removed rather than justified? If macOS is supported,
  it needs a real no-replace primitive; if it is not, the fallback is dead code
  on every supported platform and the stub at `:1200-1205` is the honest shape.
  (needs human input)
---

## Deferred candidates

The lens passes produced roughly 90 candidates; the 55 above are the strongest.
Groups I, J, and K closed the five gaps the portfolio evaluation queued: the
mandatory setup-state transitions, the shared frame-read mechanics and the
budget-free oversize drain, normal configured liveness, canonical manifest
evolution, and the Darwin store behaviour (whose macOS arms now have no CI
executor: `ci.yml` is Linux-only after PR #131, merge `5d638e3e8`). The
candidates still deferred to a
follow-up pass, with their lens evidence retained:

- Grant records: activation replay across generations, the grant binding compared
  against itself so two rejection branches are dead.
- Trust-boundary records: the eager capacity reservation before body arrival, the
  single-slot authoritative terminal, the unvalidated consumer launch nonce,
  channel epoch headroom.
- Store records: the untested source verification branches, the umask hazard
  class.
- Platform and drift records: the directory-enumeration backend divergence, the
  relative data-root CWD anchor, the two-fence coupling at creation, the
  probe's undocumented blocking budget, the exported-type stability question.


## Sub-part 2b catalog: the ring datapath in the host

Scope: the host-side ring datapath, 3,447 lines across four files.
`crates/host-runtime/src/ring_transport.rs` (966 lines) owns the process-level
transport, the per-connection `DuplexRing`, the endpoint thread, and both
publication and receive loops. `wire.rs` (973) is the frame codec and the byte
budget. `frame_channel.rs` (807) is the transport-neutral channel boundary the
connection engine sees. `frame_channel/contract_tests.rs` (701) is the semantic
contract suite. All four counts were re-derived with `wc -l` at `HEAD`.

Boundary context, read but not cataloged: `connection.rs` for the prepare and
close call sites, `runtime.rs` for construction and the ingress budget,
`client.rs` for the in-process peer, and `crates/shm-transport` for the ring
itself. Part 1 covers the transport crate and Part 2a the connection engine;
both are cited rather than re-derived.

**This is a post-refactor surface, and nothing had ever been cataloged against
it before.** `ring_transport.rs` is what the ring-transport refactor produced by
renaming `shm_provider.rs`, and the refactor also deleted five files. Four
commits carry it, all dated 2026-08-30 and all verified by
`git log -1` at authoring time:

| Commit | Subject |
| --- | --- |
| `0f336d3c` | `refactor(shm): collapse to fixed ring transport` |
| `d8bde128` | `feat(host): add authenticated ring setup socket` |
| `793a973e` | `build(shm): require packaged native transport` |
| `ed487e11` | `refactor(host): make ring transport mandatory` |

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). System
`the `host` source checkout, branch
`feat/shared-memory-release-gate-audit`, `HEAD` = `e447c927`
("refactor(shm): trim final review leftovers"). Both lens agents read and
verified their line references at that commit, and this synthesis re-verified
every citation it repeats. Scope and CI findings come from
`part-2-rescope/scope-map-and-risk-ranking.md` (a source-tree artifact that was not migrated into this repository).

Two corrections to the lens files are carried in this catalog rather than left
in the working material, per METHOD.md rule 1.

- `Admission::quarantine` is `crates/shm-transport/src/profile.rs:568`, not
  `:566`. Lens B has the correct line and lens A is off by two. The private
  `AdmissionController::quarantine` is `:522`. The finding is unaffected: a
  repository-wide search for `quarantine()` under `crates/host-runtime/src` returns
  zero call sites.
- The `#[doc(hidden)]` attribute on the module sits at `lib.rs:20` (post-#131: `:17`) and
  `pub mod ring_transport` at `:21`. Both lenses noted this refinement to the
  re-scope and both are right.

## Eventfd reconciliation pass, 2026-08-31

This catalog, `fault-map.md`, and `portfolio-evaluation.md` were reconciled
against HEAD `ec0f1bbe1` after PR #131 (merge `5d638e3e8`) replaced polling
with sparse eventfd delivery. `crates/host-runtime/src/ring_transport.rs` was
rewritten (966 to 1,045 lines): `POLL_INTERVAL` no longer exists in the file
(an inline test, `shared_memory_workers_have_no_periodic_polling` at
`ring_transport.rs:798-806`, now asserts its absence), the endpoint loop parks
on an eventfd doorbell (`arm_data_wait` at `:429`, readiness at `:459-471`),
and the ingress-budget wait awaits an async semaphore charge
(`ByteBudget::charge`, `wire.rs:397-407`) instead of polling `try_charge`.
`crates/shm-transport/src/backend/ring.rs` was rewritten too (2,374 lines),
and `wire.rs` shrank from 973 to 937 lines. Two records were re-derived
against the new mechanics,
`ring-a-cancellation-close-requires-an-empty-inbound-observation` and
`ring-a-ingress-wait-holds-a-lease-while-servicing-egress`, and every
`ring_transport.rs:`, `ring.rs:`, and `lib.rs:` citation in this catalog and
in `evidence/` was re-verified at that HEAD, with stale numbers corrected in
place and removed constructs marked as removed. Citations to other files
(`connection.rs`, `profile.rs`, `frame_channel.rs`, `wire.rs`, `runtime.rs`,
`client.rs`, `lease.rs`, `raw_client.rs`) were not swept in this pass; PR #131
rewrote several of them, so their pre-merge line numbers are suspect and need
their own pass. The scope paragraph and refactor table above are the original
authoring-time provenance and are kept as history; the line counts they state
are pre-#131.

## What this part is about

Four facts frame every record below, and one of them is the reason this sub-part
was cataloged at all.

**Admission-quarantine accounting is owned by nothing.** This is the central
finding, and it is narrower than the claim this catalog first made. The earlier
wording, "recovery is owned by nothing", is withdrawn: it was refuted by an
independent evaluation and the refutation holds. Two recovery duties that the
broad claim swept up do have owners, and both were re-read at `HEAD` for this
correction. **Peer-death teardown** is owned by the sentinel task at
`connection.rs:195-207`: `observe_peer` returns, a non-`Goodbye` close calls
`record_peer_death()` (`:200-202`), and the generation's token and read-cancel
are both cancelled (`:203-204`). **Capacity reclamation** is owned by the
endpoint thread at `ring_transport.rs:264-277`: `admission.release()` (`:276`)
and `done_tx.send(())` (`:277`) sit outside the `catch_unwind` at `:264-275`, so
they run on every exit including a swallowed panic, and `record_reclamation()`
follows at `connection.rs:209`. The charge comes back unconditionally, which is
also this catalog's own guarantee in
[ring-a-admission-charge-releases-on-every-endpoint-thread-exit](#ring-a-admission-charge-releases-on-every-endpoint-thread-exit);
the broad claim contradicted it.

What genuinely has no owner is **admission-quarantine accounting**, and only
that. The refactor deleted `crates/host-runtime/src/provider_recovery.rs` in
`ed487e11`, and `git ls-tree` over `crates/host-runtime/src` at `HEAD` shows no
successor: no file whose name contains `provider` or `recovery` survives. The
transport-side machinery is intact. `Admission::quarantine` (`profile.rs:568`)
still exists and still works, and Part 1 verified its atomicity. What is gone is
anything in the host that calls it. A search for `quarantine` under
`crates/host-runtime/src` returns no call to it; the only mentions are the unrelated
`LeaseTracker` flag (`frame_channel.rs:392`, `:420-433`), two unrelated
`instance.rs` doc comments (`:67`, `:250`), the diagnostics reporter that emits
the field (`ring_transport.rs:171`), and two in-crate assertions that the value
is zero (`:855`, `:880`). So host quarantined accounting is **structurally**
zero, not incidentally zero, and the two assertions that touch it pass
vacuously. Meanwhile `docs/shm-transport.md` presents it as live in
three places: `:21` "Active and quarantined charges are reported separately",
`:65` "active and quarantined accounting", and `:79` "quarantined charges remain
within the configured process bound". The last is satisfied by having no
quarantined charges at all.

The consequence is a policy gap, not an absent owner: because
`admission.release()` at `:276` is unconditional, a connection that exited
because its ring was condemned returns its charge on exactly the same line as a
clean one, so the two cases are accounted identically and nothing distinguishes
them. Part 1 anchored `quarantine-charge-transition-is-atomic` to
`provider_recovery.rs:187`; that anchor has no replacement. Whether a condemned
ring's arena bytes should be released or retained against the process bound is
the sub-part's sharpest open question, it is a release-versus-quarantine policy
decision rather than a missing mechanism, and it needs a human. It must be
settled before the charge records can be made consistent with each other.

**A host that cannot create shared-memory objects refuses every connection while
reporting healthy.** Ring unavailability fails closed, which is the important
half and it holds: `connection.rs:149-164` is a straight-line
`let Ok(Ok(Ok(PreparedRing { .. }))) = timeout_at(..) else { return; }`, and the
`activate_server` call is at `:170`, after it. So a failed `prepare` refuses the
connection before activation and no application frame can flow. The reportability
half is where it fails. `prepare` has five `Err(RingUnavailable)` returns and
only the first touches a counter: admission rejection (`ring_transport.rs:223-226`)
increments `exhaustions`, while runtime-or-ring creation failure (`:249-255`),
descriptor marshalling failure (`:256-259`), thread-spawn failure (`:279-281`),
and initialization-channel loss (`:282`) increment nothing. `diagnostics()`
derives `state` exclusively from `self.accounting()` (`:165-179`), which reads
the admission snapshot and cannot observe a failed `prepare` at all, so it
reports `state: "healthy"` with `error_class: null`. `RingUnavailable`
(`:103-112`) is a unit struct with a fixed `Display` string and no cause field,
and the `else` branch emits no `ServerMessage`, so the peer sees only a closed
setup socket and reports the generic `setup_failed`. A host whose `/dev/shm` is
full therefore presents as a healthy host that refuses every connection for no
stated reason. That is a silent total outage of the only datapath.

**No caller retains a committed release identity.** All nine `commit` call sites
in this surface discard the `ReleaseIdentity` that
`ProducerReservation::commit` returns. The three non-test producers are
`ring_transport.rs:615`, `:628`, and `:696`; the first two are
`reservation.commit(body_len).map_err(|_| ())?`, verified by printing them. The
three inline tests (`:935`, `:985`, `:1022`) and the three integration helpers
(`tests/support/raw_client.rs:698`, `:743`, `:799`) discard it too. This was
verified independently by two passes, lens A by enumerating every `.commit(`
call in the tree and this synthesis by re-printing the six sites in
`ring_transport.rs`. The consequence is a verdict that carries over rather than
one that needs re-deriving: **Part 1's judgement that the producer-side release
hazard is latent survives the refactor.** Part 1 judged `Ring::release`'s
producer-facing form latent precisely because every non-test `commit` caller
discarded the identity. The refactor rewrote all of those callers and they still
discard it, so Part 1's `release-authority-bound-to-lease-ownership` and
`release-exactly-once-per-sequence` keep their reachability labels on the
producer side. Only the line numbers move, from `shm_provider.rs:365` to
`ring_transport.rs:615` and `:628`. See
[ring-a-no-producer-retains-a-committed-release-identity](#ring-a-no-producer-retains-a-committed-release-identity)
for the record and Part 1 for the underlying verdict.

**Reachability is `default-production`, and three signals argued otherwise.**
The re-scope left the class open because three things looked like test markers.
All three are misleading and each was resolved against code.

- The profile name `RING_PROFILE = "host-test-ring-v1"`
  (`ring_transport.rs:30`) has "test" in it, but it is a descriptor field
  compared for equality at attach, and `docs/shm-transport.md:11` names
  that exact literal as the release-fixed profile identity. It gates nothing.
- `RingClientEndpoint` is doc-commented "Thread-confined peer endpoint for
  integration tests" (`ring_transport.rs:650`). That comment is simply wrong.
  `client.rs:1855` constructs it inside `start_ring_bridge`, which
  `Client::connect_info` reaches on the ordinary connect path
  (`client.rs:346-375`), with no `cfg(test)` and no config gate.
- `lib.rs:17-18` exports the module as `#[doc(hidden)] pub mod ring_transport`.
  `#[doc(hidden)]` hides a module from rustdoc and does not restrict linkage.

What decides it is that there is no gate anywhere. `RingTransport` is
constructed unconditionally during host startup at `runtime.rs:876`, and this
synthesis printed the surrounding lines to confirm it: `process_limits` failure
becomes a hard `HostError::InitFailed` (`:872-875`) and the transport is stored
non-optionally as `HostShared.ring` (`:104`). Every authenticated connection
calls `ring.prepare(...)` at `connection.rs:148`. There is no `Option`, no
`if config`, and no alternative branch, which matches
`docs/shm-transport.md:7`: "There is no runtime transport selector,
alternate shared-memory backend, compatibility reader, or degraded data path."

Two named sub-surfaces inside the same file are genuinely test-only, and they
are labelled where they appear rather than in a blanket claim. `PublishHook`
(`ring_transport.rs:36`, `#[doc(hidden)]` at `:35`) and `set_publish_hook`
(`:213`) are reached only through `run_with_publish_hook`
(`runtime.rs:641`, `#[doc(hidden)]` at `:640`), whose only callers are
`tests/support/mod.rs:597`, `:614`, and `:650`. One correction to lens A here:
it announced two non-default sub-surfaces but then resolved the second,
`RingClientEndpoint::try_recv_with` (`:723`), as `default-production`, because
`client.rs:1878` reaches it in production. That resolution is correct, so the
test-only surface is the publish hook and its two entry points, not two
independent surfaces. **No record in this catalog carries a `test-only` label.**
The one record that touches the hook,
[ring-a-endpoint-thread-panic-is-reported-as-orderly-completion](#ring-a-endpoint-thread-panic-is-reported-as-orderly-completion),
is `default-production` because the production `written` completion hook shares
the same unprotected window; the test-only hook is merely the cheapest way to
enter it.

One reachability limit is not resolved and is not guessed.
`RingClientEndpoint::send` and `recv` (`:684`, `:702`) are `pub` on a
`#[doc(hidden)] pub mod`, which hides them from rustdoc but not from linkage.
Only this repository was inspected, so `default-production` on records touching
them covers in-tree use only.

### Coverage: 37 in-crate tests, and all of them run in CI

**37 in-crate tests reach this sub-part, and CI executes every one.** This
reverses the authoring-time headline. `ci.yml:115` runs
`cargo nextest run -p host-runtime -p shm-transport --lib` in the "Mandatory ring
unit and client suites (Linux)" job (`:93`); `--lib` selects the package library
test target, which is where all three units live -
`frame_channel/contract_tests.rs` is reachable as `pub(crate) mod contract_tests`
from `frame_channel.rs:27`, so it is a library module, not an integration
binary. Counts below were re-derived with `cargo nextest list -p host-runtime --lib`
rather than by grepping attributes, which is what corrects the earlier
`ring_transport.rs` figure from 7 to 9 and confirms 14 for
`contract_tests` (9 at the module root plus 5 in its nested
`ownership_contract` module).

| Unit | Tests | Executed in CI |
| --- | --- | --- |
| `wire.rs`, `mod tests` at `:614-937` | **14** | **Yes** (`ci.yml:115`, `--lib`) |
| `frame_channel/contract_tests.rs` | **14** | **Yes** (`ci.yml:115`, `--lib`) |
| `ring_transport.rs`, `mod tests` at `:783-1044` | **9** | **Yes** (`ci.yml:115`, `--lib`) |
| `frame_channel.rs` | **0** | n/a |
| **Total in-crate** | **37** | **Yes** |

The authoring-time claim was that this was structural: that every `-p host-runtime`
invocation in `ci.yml` carries a `--test <name>` filter which selects one
integration binary and never builds the lib target. That is false at HEAD.
Re-derived, the `cargo` invocations naming `host-runtime` are `:115` (`--lib`), `:119`,
`:120`, `:121`, `:128`, `:168`, `:175` (`--doc`), and `:236`; `:115` is neither
filtered by `--test` nor an integration run. Any
`Existing check: ... does not run in CI` conclusion resting on an inline unit
test in these three units is therefore stale and should be re-read as CI-covered.

**A second, smaller correction, now subsumed by the above.** The re-scope's
statement that no in-crate check executes in CI is false of doctests as well:
`cargo test -p host-runtime --doc` runs at `ci.yml:175` under the step name "Rust
lease non-escape" (`:174`), and this sub-part has exactly two doctests, both
`compile_fail`, at `frame_channel.rs:296-301` and `:303-308`. Both were printed
and confirmed: they assert that `ReceiveLease` is neither `Send` nor `'static`.
They were recorded as this sub-part's only CI-executed source-resident checks,
which the `--lib` finding above supersedes - they are two of many.
`wire.rs:4-14` is a ```text``` fence and is not compiled.

**CI in this tree.** `.github/workflows/ci.yml:118` and `:126` run
`cargo test --workspace --all-targets --all-features --locked` on the 1.98 and stable
toolchains, so every integration binary and every inline test this section counts
executes in CI. The named-versus-unnamed distinction and the `ci.yml` line numbers
below describe the source repository's workflow at authoring time and are kept as
provenance; they are not coverage gaps here.

Coverage does arrive from integration tests, indirectly. **Ten of the 24
integration binaries use `support::TestHost`, which starts a real host and
therefore a real ring, and four of the ten are named in CI.** This synthesis
re-derived the membership by testing each of the 24 for `TestHost` use, and the
result is exactly lens B's list: `client.rs` (6 tests), `lifecycle.rs` (35),
`shm_failure_modes.rs` (6), `shm_soak.rs` (2), `protocol_vectors.rs` (15),
`dispatch.rs` (20), `routing.rs` (12), `handler_contract.rs` (12),
`host_roundtrip.rs` (4), and `instance_security.rs` (15). The four named are
`client`, `lifecycle`, `shm_failure_modes`, and one of `shm_soak`'s two tests
under `--exact`. Details are in
[existing-checks.md](ring-datapath/existing-checks.md).

## Index

Fourteen records from this sub-part's own lens passes, in the order lens A
proposed them. Lens B proposed none by design; it built the claim register and
the check inventory. **Four further records were carried into this sub-part in a
later pass**, from the superseded pre-refactor `part-2b-wire-and-channels`; they
are the last four rows and they live in
[Group G](#group-g-the-wire-header-decode-contract). Eighteen records in total.

| Slug | Type | Confidence |
| --- | --- | --- |
| [ring-a-endpoint-thread-solely-owns-both-ring-endpoints](#ring-a-endpoint-thread-solely-owns-both-ring-endpoints) | safety | high |
| [ring-a-no-producer-retains-a-committed-release-identity](#ring-a-no-producer-retains-a-committed-release-identity) | safety | high |
| [ring-a-admission-charge-releases-on-every-endpoint-thread-exit](#ring-a-admission-charge-releases-on-every-endpoint-thread-exit) | safety | high |
| [ring-a-host-never-quarantines-an-admission-charge](#ring-a-host-never-quarantines-an-admission-charge) | safety | high |
| [ring-a-publish-failure-is-reported-as-a-clean-peer-close](#ring-a-publish-failure-is-reported-as-a-clean-peer-close) | safety | high |
| [ring-a-endpoint-thread-panic-is-reported-as-orderly-completion](#ring-a-endpoint-thread-panic-is-reported-as-orderly-completion) | safety | high |
| [ring-a-ring-unavailability-fails-closed-without-a-classified-reason](#ring-a-ring-unavailability-fails-closed-without-a-classified-reason) | safety | high |
| [ring-a-lease-release-failure-is-observable-only-on-the-success-path](#ring-a-lease-release-failure-is-observable-only-on-the-success-path) | safety | high |
| [ring-a-reclamation-count-does-not-witness-charge-release](#ring-a-reclamation-count-does-not-witness-charge-release) | safety | high |
| [ring-a-host-doctor-emits-one-of-five-declared-terminal-classes](#ring-a-host-doctor-emits-one-of-five-declared-terminal-classes) | reachability | high |
| [ring-a-rejected-drain-failure-close-has-no-producer](#ring-a-rejected-drain-failure-close-has-no-producer) | reachability | high |
| [ring-a-segmented-inbound-body-has-no-production-producer](#ring-a-segmented-inbound-body-has-no-production-producer) | reachability | high |
| [ring-a-cancellation-close-requires-an-empty-inbound-observation](#ring-a-cancellation-close-requires-an-empty-inbound-observation) | liveness | medium |
| [ring-a-ingress-wait-holds-a-lease-while-servicing-egress](#ring-a-ingress-wait-holds-a-lease-while-servicing-egress) | reachability | high |
| [decode-header-is-total-over-arbitrary-bytes](#decode-header-is-total-over-arbitrary-bytes) | safety | high |
| [accepted-header-decode-is-a-bijection-on-twenty-one-bytes](#accepted-header-decode-is-a-bijection-on-twenty-one-bytes) | safety | high |
| [reserved-encodings-and-identity-pairings-reject-at-decode](#reserved-encodings-and-identity-pairings-reject-at-decode) | safety | high |
| [encoder-never-emits-a-frame-its-own-decoder-rejects](#encoder-never-emits-a-frame-its-own-decoder-rejects) | safety | high |

The last four rows are the carried records. They keep their original unprefixed
slugs so the carry stays visible against the fourteen `ring-a-` records this
sub-part derived itself.

The six group headings below are this synthesis's own, chosen by shared
mechanism rather than by the order records were proposed. Grouping reorders the
records relative to the index; the index is the record-order artifact.

Distribution after the portfolio disposition in
[portfolio-evaluation.md](ring-datapath/portfolio-evaluation.md): **8 safety, 5 reachability,
1 liveness**, and semantics **8 `always`, 1 `always-or-unreached`, 2
`sometimes`, 2 `reachable`, 1 `unreachable`**. `always(!X)` counts as `always`.
Two records changed under that disposition and both are recorded at the record:
the release-identity record moved from `reachability`/`unreachable` to
`safety`/`always`, and the doctor record moved from `reachable` to `sometimes`.

The four carried records add **4 safety** and semantics **4 `always`**, none of
which passed through that disposition, so the eighteen-record totals are
**12 safety, 5 reachability, 1 liveness** and **12 `always`, 1
`always-or-unreached`, 2 `sometimes`, 2 `reachable`, 1 `unreachable`**.
Reachability: all four carried records are `default-production`, verified per
record at carry time. Confidence: four high.

---

## Group A: thread confinement and the unused release identity

Two records on what the endpoint thread owns and what it throws away. The first
is the premise every other ring property rests on, that exactly one OS thread
ever touches either `Ring`. The second is the observation that the producer half
of the transport's release contract has no host caller, which is what lets
Part 1's producer-side verdict carry over unchanged. They are grouped together
because both are static ownership facts about the same thread, provable without
any fault.

### ring-a-endpoint-thread-solely-owns-both-ring-endpoints

Type: safety
Reachability: default-production - the thread is spawned by `prepare`
(`ring_transport.rs:238-240`), which every authenticated connection calls at
`connection.rs:148`; `RingTransport` is constructed unconditionally at
`runtime.rs:876` and stored non-optionally as `HostShared.ring` (`:104`), so
there is no `Option`, no `cfg`, and no config branch on this path.
Status: active
Exercised: partial - `construction_has_no_ring_side_effects`
(`ring_transport.rs:851-856`) proves the process-level owner holds no ring, and
the `RingFactory` contract suite drives a real `prepare`, but nothing asserts
that no `Ring` value is ever observed from a second thread.
Guarantee: For the whole life of one connection, exactly one OS thread ever
holds either `Ring` of its `DuplexRing`, and no ring handle, mapping pointer, or
arena reference crosses a thread boundary.
Check: `always` - for every prepared connection, the set of thread ids that
touch either `Ring` has cardinality one, and `PreparedRing` contains no field
whose type transitively owns a `Ring`. `always` fits because this is the
premise every other ring property rests on: the transport's single-producer and
single-consumer cursors are unsynchronized between peers of the same direction,
so a second local thread is immediate undefined behaviour, not a degradation.
Fault/timing angle: the window is the whole connection. The specific risk is a
future refactor returning a ring from `prepare` or storing one in
`PreparedRing`; `Ring` is `!Send`, so the compiler catches the direct move, but
a raw pointer, an index into a shared arena, or a `ReceiveLease` smuggled out
through an `unsafe` block would not be caught. `#![deny(unsafe_code)]`
(`lib.rs:5`) currently forecloses that inside `host-runtime`.
Required faults and enabling state: none for the structural check. For a
runtime check, an active connection with both directions carrying traffic, so
that a second thread would actually contend.
Confidence: high - [evidence](evidence/ring-a-endpoint-thread-solely-owns-both-ring-endpoints.md).
Verified by inspection: `DuplexRing::create` at `ring_transport.rs:248` is
inside the thread closure opened at `:240`; `rings` is moved into
`run_endpoint` by value at `:265`; `PreparedRing` (`:93-101`) has seven fields
and none is a `Ring`; the only values crossing the `sync_channel` at `:231` are
a `serde_json::Value` and `[OwnedFd; RING_DESCRIPTOR_COUNT]` - six descriptors
post-#131, up from two, still no ring - sent at `:261`.
Existing check: `ring_transport.rs:851-856`
`construction_has_no_ring_side_effects` - covers the process owner only, and
runs in CI in this tree (`ci.yml:118`, `:126`, `cargo test --workspace --all-targets`). Status unaudited.
Impact: two threads driving one direction's cursors is a data race on the shared
control page, which the transport's `try_receive` would surface as descriptor
validation failure and quarantine at best, and as torn payload delivery at
worst.
Open questions:
- Should `PreparedRing` carry a negative marker, or a compile-fail doctest like
  the two on `frame_channel::ReceiveLease` (`frame_channel.rs:296-308`), so the
  confinement is enforced rather than reviewed?

### ring-a-no-producer-retains-a-committed-release-identity

Type: safety
Reachability: default-production - the three producer call sites are on the
host's publication path, which every activated connection runs:
`ring_transport.rs:615` and `:628` are inside `publish_one`'s helpers
`publish_direct` (`:604`) and `publish_owned` (`:619`), reached from
`run_endpoint` (`:479-484`) and from the charge wait (`:533-540`), and
`:696` is inside `RingClientEndpoint::send` (`:684`), reached in production from
`client.rs:1878` on the ordinary connect path. No `cfg` gate and no config gate
stands on any of the three. The `Ring::release` end of the property is likewise
production: `ring_release_callback` (`ring.rs:1670-1677`) runs on every lease
drop.
Status: active
Exercised: not yet - no test asserts the absence, and the value is dropped at
every call site, so observing the provenance at all needs a
`#[cfg(debug_assertions)]` counter that does not exist.
Guarantee: No host or client producer path retains the `ReleaseIdentity`
returned by `ProducerReservation::commit`, so `Ring::release` is never called
with a producer-derived identity, and the producer-side half of Part 1's
release contract stays unreachable.
Check: `always` - `always(!X)` where X is "`Ring::release` (`ring.rs:1469`) is
entered with an identity that originated from `ProducerReservation::commit`
(`ring.rs:2561`)". Discharged today by enumerating every `.commit(` site and
showing each discards its `Ok` value; optionally backed by a
`#[cfg(debug_assertions)]` counter on the producer-identity path that must stay
at zero. **This record previously claimed `unreachable`, which was wrong and is
corrected here.** `unreachable` is reserved for a code location that must never
execute, and `Ring::release` executes on every lease drop in production, through
`ring_release_callback` (`ring.rs:2458-2465`) carrying a lease-derived identity.
What the property forbids is not the location but the *provenance of an
argument* at a shared function, which is a state with no dedicated detection
point, and METHOD.md's rule for that is `always(!X)`. The type moves from
`reachability` to `safety` for the same reason: the claim is an authority
invariant on who may release a sequence, not coverage of a code point.
Fault/timing angle: none. This is a static call-graph and provenance property;
the interleaving risk it *forecloses* is a producer releasing a sequence a
consumer still holds a lease on.
Required faults and enabling state: none. The enumeration needs no fault. A
runtime form needs only the debug counter and any connection that publishes.
Confidence: high - [evidence](evidence/ring-a-no-producer-retains-a-committed-release-identity.md).
Verified by enumerating every `.commit(` call in the tree: the three non-test
producers are `ring_transport.rs:615`, `:628`, `:696`, all of which apply
`map_err(..)?` and discard the `Ok` value; the inline tests `:935`, `:985`,
`:1022` and `tests/support/raw_client.rs:698`, `:743`, `:799` also discard it;
`contract_tests.rs:567` and `:600` call the unrelated
`frame_channel::ProducerReservation::commit`, which returns `ProducedBody`. Also
re-verified for this disposition: `commit` is
`pub fn commit(mut self, body_len: usize) -> Result<ReleaseIdentity, ProducerError>`
at `ring.rs:1769`, and `ring.rs:1175` `Ring::release` is entered in production by
`ring_release_callback` at `:1670-1677`.
Existing check: none.
Impact: **Part 1's latency verdict on the producer-side release survives the
refactor.** Part 1 judged `Ring::release`'s producer-facing form latent because
every non-test `commit` caller discarded the identity. The refactor rewrote all
of those callers, and they still discard it. So Part 1's
`release-authority-bound-to-lease-ownership` and
`release-exactly-once-per-sequence` keep their reachability labels on the
producer side, and no re-anchoring of the verdict is needed - only of the line
numbers, from `shm_provider.rs:365` to `ring_transport.rs:615`/`:628`.
Open questions:
- Is the producer-side `ReleaseIdentity` return value intended to stay unused?
  If so, `#[must_use]` on `commit` is currently misleading, and the simpler
  contract would be for `commit` to return `()` and for identities to exist
  only on the consumer side. (needs human input)

---

## Group B: admission accounting with no quarantine owner

Two records on the charge that bounds how many connections the host can carry.
The first is the obligation that every exit path returns it, which rests on
`Admission`'s `Drop` for three initialization paths that never call `release()`
explicitly. The second is the central finding of the sub-part, and it is narrow:
no host path raises a quarantine, so a condemned ring returns its charge on
exactly the same line as a clean one and the documented quarantined figure is
structurally zero. Note what the second does **not** say. The charge itself has a
clear owner and it does come back; peer-death teardown has an owner too
(`connection.rs:195-207`). What is missing is the accounting distinction between
a clean recycle and a condemned one, and whether that distinction should exist is
a policy question this catalog does not settle. They share one mechanism, the
`Admission` guard moved into the thread closure at `:240`.

### ring-a-admission-charge-releases-on-every-endpoint-thread-exit

Type: safety
Reachability: default-production - `admit` (`ring_transport.rs:223`) runs on
every `prepare`, and `prepare` runs on every authenticated connection
(`connection.rs:148`). All five exit paths this record enumerates are on that
same unconditional path; none is behind a `cfg` or a config gate.
Status: active
Exercised: partial - `docs/shm-transport.md:79` states the obligation
("Repeated peer crashes must not increase active charges after reclamation")
and `tests/shm_failure_modes` exists, but no test in the 2b file set asserts
per-exit-path charge return.
Guarantee: Every path out of the endpoint thread returns the connection's full
admission charge exactly once, including the initialization-failure paths that
exit before `run_endpoint` is entered.
Check: `always` - after the endpoint thread for a connection has terminated,
`AdmissionController::snapshot().active` has decreased by exactly
`per_connection_limits()` relative to the value just after that connection's
`admit`. `always` fits because a charge stranded on any path is monotone: the
controller has no sweeper, so the leak persists for the host's lifetime and
each occurrence permanently lowers `max_connections`.
Fault/timing angle: the interesting paths are the ones that exit *before* the
`Admission` guard is consumed at `:276`. Two return early inside the closure:
runtime or `DuplexRing::create` failure (`:249-255`) and `worker_descriptor`
failure (`:256-259`). Both drop `admission` rather than calling `release()`, so
correctness depends on `Admission`'s `Drop` (`profile.rs:581-586`; not re-swept
post-#131) which
releases when the state is still `Active`. A third path, `initialized_tx.send`
failing at `:261-263`, likewise relies on `Drop`.
Required faults and enabling state: one fault per path. `DuplexRing::create`
failure needs shared-memory object creation to fail, reachable by exhausting
`/dev/shm` or the fd limit. `worker_descriptor` failure needs
`Ring::attachment()` to fail. Thread-spawn failure (`:279-281`) exits before
`admit`'s guard leaves the caller, so it needs the guard's `Drop` on the
`prepare` side. A panic inside `run_endpoint` needs the `catch_unwind` at `:264`
to still reach `:276`.
Confidence: high - [evidence](evidence/ring-a-admission-charge-releases-on-every-endpoint-thread-exit.md).
Verified by inspection: `Admission` carries an `AdmissionState`
(`profile.rs:546-557`) and its `Drop` releases when `Active`
(`profile.rs:581-586`); the explicit `release()` at `ring_transport.rs:276` is
outside the `catch_unwind`, so a panic inside `run_endpoint` still reaches it;
`AdmissionController::release` (`profile.rs:512-520`) is a `checked_sub` that
silently no-ops on underflow, so a double release cannot go negative but also
cannot be detected.
Existing check: none in the 2b file set. `crates/shm-transport/tests/contract.rs:472`
covers `Admission::release` at the transport layer. Status unaudited.
Impact: a stranded charge is permanent. Since `process_limits` multiplies the
per-connection charge by the connection count - post-#131 additionally capped
by `MAX_RING_RESIDENT_BYTES` (`ring_transport.rs:60-80`) - one
stranded connection's worth of arena bytes permanently removes one connection
slot, and the failure presents much later as `RingUnavailable` on an unrelated
connect with `state: "healthy"` in diagnostics (see
`ring-a-host-doctor-emits-one-of-five-declared-terminal-classes`).
Open questions:
- `AdmissionController::release` swallows a `checked_sub` underflow
  (`profile.rs:516-519`). Is a double release meant to be silent, or should it
  be a detectable accounting fault?

### ring-a-host-never-quarantines-an-admission-charge

Type: safety
Reachability: default-production for the release path this record contrasts
against, and **compiled-with-no-production-caller** for the subject itself.
`admission.release()` (`ring_transport.rs:276`) runs on every endpoint exit of
every authenticated connection (`connection.rs:148`), with no gate.
`Admission::quarantine` (`profile.rs:568`) is compiled into every host build
because `shm-transport` is a non-optional dependency, and it has **no caller
anywhere in `crates/host-runtime/src`**, in production or in test. That is stated
here rather than defaulted: the label is neither `default-production` nor
`test-only` for the quarantine half, because the path is reachable from no host
code at all.
Status: active
Exercised: not yet - nothing in `host-runtime` can construct the state, so no host
test can reach it.
Guarantee: Every admission charge an endpoint takes is accounted exactly once at its exit: released, or quarantined when the exit is a ring-corruption exit that the transport contract (`docs/shm-transport.md:21`, `:65`, `:79`) says should quarantine; never both and never neither. The slug names the discovery-time finding that no `host-runtime` path calls `Admission::quarantine`, so today the quarantined side of that accounting is structurally zero and every corrupt exit releases as if the storage were cleanly recycled, which is a contract-versus-code disagreement, not a forbidden location.
Check: `always` - across every endpoint exit in a campaign, including corrupt-ring and swallowed-panic exits, `released + quarantined` charges equal the charges taken (no leak, no double count), and for every exit the transport contract classifies as corrupt, `snapshot().quarantined` has grown by that endpoint's charge. The second clause is a predicted violation at HEAD, because `Admission::quarantine` (`profile.rs:561`) has no `host-runtime` caller; the check asserts the documented contract rather than freezing the gap. `always` because accounting must balance at every exit.
Fault/timing angle: the window that matters is a `Corrupt` exit. When
`Ring::try_receive` fails descriptor validation it calls `enter_quarantine()`
inside the transport (`ring.rs:1098`), so the ring is terminally quarantined per
Part 1's `quarantine-authority-survives-peer-writes`. The host maps that to
`ReadClose::Corrupt` (`ring_transport.rs:499`), exits `run_endpoint` at
`:406-411`, and still calls `admission.release()` at `:276`. The process-wide
accounting therefore shows the arena bytes as free while the ring that held
them is condemned.
Required faults and enabling state: a quarantined peer-to-host ring plus an
inspection of `accounting().quarantined` afterwards. Two producers reach it, and
the second is cheaper than this record originally recorded: a
descriptor-validation failure inside `try_receive` (`ring.rs:1098`), or a peer
that calls the public `Ring::enter_quarantine` (`ring.rs:1373-1378`) directly on
the endpoint it already holds (`RingClientEndpoint.to_host` is a `pub` field,
`ring_transport.rs:651-656`). See
[ring-a-lease-release-failure-is-observable-only-on-the-success-path](#ring-a-lease-release-failure-is-observable-only-on-the-success-path).
Confidence: high - [evidence](evidence/ring-a-host-never-quarantines-an-admission-charge.md).
Verified by enumerating `Admission::quarantine` calls in the tree: the only two
are `crates/shm-transport/tests/contract.rs:368` and `:479`. A `quarantine`
grep over `crates/host-runtime/src` at `HEAD` returns only unrelated hits: the
`LeaseTracker` flag (`frame_channel.rs:392`, `:420-433`), two `instance.rs` doc
comments (`:67`, `:250`), and one contract test on the tracker
(`contract_tests.rs:690`). `RingTransport` holds no `Admission` value after
`prepare` returns, because the guard moved into the thread closure at `:240`.
Existing check: `ring_transport.rs:880` asserts
`accounting.quarantined.arena_bytes == 0` on a fresh transport, which is the
same value the property says can never change. A second assertion of the same
fact is at `:855`. Status unaudited.
Impact: the quarantine accounting that
`docs/shm-transport.md:21`, `:65`, and `:79` present as a live safety
mechanism is inert on the host. Because the mapping is genuinely unmapped when
`run_endpoint` drops the `DuplexRing`, releasing the charge is arguably correct
and the doc is what is wrong; but the two readings differ on whether a
quarantined ring's arena bytes should be retained against the process bound,
and only a human can settle which was intended. This is the
release-versus-quarantine policy question, and it is bias 2 in
[portfolio-evaluation.md](ring-datapath/portfolio-evaluation.md); it must be settled before
this record and
[ring-a-admission-charge-releases-on-every-endpoint-thread-exit](#ring-a-admission-charge-releases-on-every-endpoint-thread-exit)
can both be right, because one requires the charge to come back on every exit
and the other asks whether a condemned ring is an exception.
Open questions:
- Is the missing quarantine caller a deferred feature or a decision that the host never quarantines? The transport document says the former; the record's second clause fails until one of them is implemented or the document changes. (needs human input)
- Was host-side quarantine accounting deliberately dropped with
  `provider_recovery.rs`, or lost? Part 1's
  `quarantine-charge-transition-is-atomic` cited
  `provider_recovery.rs:187` as its host-side driver, and that file has no
  successor. (needs human input)

---

## Group C: failure attribution on every exit path

Three records on what the host tells the connection engine when the transport
itself fails. A publication failure arrives as a clean peer EOF, an endpoint
panic arrives as orderly completion, and a failed `prepare` arrives as nothing
at all. Each is a distinct mechanism reaching the same shape: a host-caused
fault indexed as something else, or as silence. Grouped because all three turn
on the erasure of a cause that existed at the failure site.

### ring-a-publish-failure-is-reported-as-a-clean-peer-close

Type: safety
Reachability: default-production - `publish_one` (`ring_transport.rs:560`) is
called from `run_endpoint` (`:479-484`) and from the charge wait (`:533-540`),
both on the endpoint thread every authenticated connection runs
(`connection.rs:148`). `ShmReceiver::recv`'s `CleanEof` mapping (`:354`) and its
consumer (`connection.rs:401-404`) are on the same ungated path.
Status: active
Exercised: not yet - needs an outbound publication that fails while the
connection is otherwise healthy, plus an assertion on the resulting close
disposition rather than on liveness.
Guarantee: An outbound publication failure is reported to the connection engine
with a close cause distinct from a clean peer EOF, so a host-side transport
fault is never attributed to the peer.
Check: `always` - whenever `publish_one` returns `Err`, the cause delivered on the inbound channel is not `ReadClose::CleanEof`, and the connection's final disposition or operator-visible classification distinguishes the host-side transport fault from a peer close. The second clause is a predicted violation at HEAD: `read_loop` folds `Err(ReadClose::Corrupt(_))` into the same `ReadExit::Peer` arm as `CleanEof` (`crates/host-runtime/src/connection.rs:362-365`, re-verified), so the intermediate enum distinguishes the cause and the disposition does not. `always` fits because the close disposition is a total function of the cause (Part 2a, `close-disposition-is-a-total-function-of-the-read-exit-cause`) and a misclassified cause silently selects the wrong teardown every time it occurs.
Fault/timing angle: no interleaving is needed; the misreport is the
straight-line behaviour. `run_endpoint:479-484` cancels `queue.retired` and
`root` and returns without sending on `inbound`. Dropping the sender closes the
channel, and `ShmReceiver::recv` maps a closed channel to
`Err(ReadClose::CleanEof)` (`:354`), which `connection.rs:401-404` maps to
`ReadExit::Peer` - a silent retirement with no terminals and no Goodbye
(`connection.rs:309-315`). The one exception is a publish failure raised from
inside the charge wait, which does return a distinguishable cause,
`ReadClose::Corrupt("shared-memory publish failed")` (`:535-537`). So the same
fault classifies two different ways depending on which loop observed it.
Required faults and enabling state: an outbound publish failure. Four
mechanisms reach it: reservation deadline expiry under a full host-to-peer ring
(`reserve_until`, `ring.rs:980`, deadline exits at `:989`, `:1005`, `:1024`,
`:1044`), a wire-header/length disagreement rejected by
`commit_reservation` (`ring.rs:1577-1593`), a panic in the direct serializer
caught at `:584-587`, and `ReservationWriter` exhaustion (`:635-643`). The
cheapest to construct is a peer that attaches and then never receives, filling
the host-to-peer ring until `reserve_until` hits its deadline.
Confidence: high - [evidence](evidence/ring-a-publish-failure-is-reported-as-a-clean-peer-close.md).
Verified by inspection: `publish_one` returns `Result<(), ()>` (`:560-565`), so
every distinct cause is erased to a unit before `run_endpoint` sees it; the
`:479-484` block sends nothing; `:354` is the only `CleanEof` producer in the
crate; `connection.rs:401` is the only `CleanEof` consumer.
Existing check: none. `connection.rs:401-404` is the consuming match, not a
check.
Impact: two consequences. Operationally, a host-side ring fault is indexed as a
peer disconnect, so the diagnostics counters and any operator narrative blame
the client. Protocol-wise, `ReadExit::Peer` retires silently and discards the
queued frames (`connection.rs:315-318`), which is the correct handling for a
peer-caused close but means a host-caused close also produces no terminal, so
every pending correlation becomes `outcome_unknown` with no recorded reason.
Open questions:
- Should `publish_one` carry a cause enum rather than `()`? The information
  exists at each of the four failure sites and is discarded at `:588-590`.
- Is the asymmetry between `:535-537` (`Corrupt`) and `:479-484` (`CleanEof`)
  for the identical fault deliberate? (needs human input)

### ring-a-endpoint-thread-panic-is-reported-as-orderly-completion

Type: safety
Reachability: default-production - the unprotected window at
`ring_transport.rs:587-600` is entered on every publication, and the hook that
runs inside it at `:598` is the production `written` completion hook supplied
through `frame_channel.rs:630` by `dispatch.rs`. The `PublishHook` at `:594` is
test-only (reached only via `run_with_publish_hook`, `runtime.rs:641`, whose
callers are `tests/support/mod.rs:597`, `:614`, `:650`), and it is named here as
the cheapest injection point rather than as the record's subject.
Status: active
Exercised: not yet - needs an induced panic on the endpoint thread. The publish
hook (`test-only`) is the cheapest injection point, but the property is about
the production `written` hook too.
Guarantee: A panic that escapes `run_endpoint` is distinguishable by the
connection engine from an orderly endpoint exit, and the frame it was
publishing is not left recorded as complete.
Check: `always-or-unreached` - if the outer `catch_unwind` at
`ring_transport.rs:261` observes `Err`, then the connection observes a cause
other than a clean completion, and no `QueuedOutboundFrame` remains in state
`COMPLETE` without having reached the ring. `always-or-unreached` fits because
a panic on this thread is an optional path that a correct build never takes, but
it must be safe when it does; `always` would overstate a requirement that the
path be exercised.
Fault/timing angle: the exposed window is between `:587` and `:600`. The inner
`catch_unwind` protects only the reserve-fill-commit block. A panic in the
publish hook (`:594`), or in the `written` local-completion hook (`:598`),
unwinds `publish_one` and `run_endpoint`, is swallowed by `let _ =` at `:264`,
and then `admission.release()` (`:276`) and `done_tx.send(())` (`:277`) run
exactly as on an orderly exit. Neither `queue.retired` nor `root` is cancelled,
so `FrameSender::send_ticket_before` keeps admitting frames until its own
admission timeout fires (`frame_channel.rs:742-750`), and the `io` future
completes successfully, which `connection.rs:347` reads as a clean join. A
second, narrower window: a panic inside `on_publish()` (`frame_channel.rs:653-655`)
leaves the ticket state at `PUBLISHED` with the frame never written, so a
later `FrameSendTicket::cancel` returns `PossibleSend` for a frame that was
provably not sent, contradicting
`docs/host-wire-protocol.md:60`.
Required faults and enabling state: a panicking hook. `written` is the
production one; `dispatch.rs` supplies it through `OutboundFrame::written`
(`frame_channel.rs:630`). Part 2a owns writer-hook panics on the writer task
(`no-writer-hook-panic-poisons-a-generation-lock`); this is the ring thread, a
different owner, and the panic there has no boundary at all.
Confidence: high - [evidence](evidence/ring-a-endpoint-thread-panic-is-reported-as-orderly-completion.md).
Verified by inspection: `:264` discards the `catch_unwind` result; the inner
`catch_unwind` closes at `:587`; `:591` stores `COMPLETE` before the hooks run
at `:592-598`; `panic_boundary::redact_sync` wraps only the direct serializer
(`:610-613`) and not the hooks.
Existing check: none for the ring thread. `panic_boundary.rs` is Part 2a scope.
Impact: the host loses its only transport thread and reports success. Frames
admitted after the panic sit in the queue until each hits its admission
deadline, so the connection degrades over `frame_deadline` per frame rather than
retiring, and diagnostics records nothing at all: no `peer_death`, no
`exhaustion`, and `state: "healthy"`.
Open questions:
- Should `:591`'s `COMPLETE` store move after the hooks, or should the hooks
  move inside the inner `catch_unwind`? The two answers differ on whether a
  hook panic should retire the connection.

### ring-a-ring-unavailability-fails-closed-without-a-classified-reason

Type: safety
Reachability: default-production - all five `Err(RingUnavailable)` returns are
inside `prepare` (`ring_transport.rs:217-303`), which every authenticated
connection calls at `connection.rs:148`, and the refusal is consumed by the
straight-line `let ... else` at `connection.rs:149-164`. `diagnostics()`
(`:142-196`) is reached from the daemon's own status surface with no gate.
Status: active
Exercised: partial - the exhaustion sub-case is covered by
`docs/shm-transport.md:79`'s stated gate and is counted at
`ring_transport.rs:224`; the other four causes are uncounted and untested.
Guarantee: When the ring cannot be prepared the host refuses the connection
before any application frame can flow, and the refusal is attributable to a
cause rather than presenting as an unexplained socket close.
Check: `always` - for every `prepare` returning `Err(RingUnavailable)`, no
`activate_server` runs on that connection, and exactly one host-observable
record names the cause. The first clause is the fail-closed half and holds; the
second is the reportability half and is where the property is expected to fail.
`always` fits because there is no fallback path to degrade onto: after the
refactor, `docs/shm-transport.md:7` makes ring failure terminal for the
connection, so every occurrence must be attributable or the operator has no
signal at all.
Fault/timing angle: none for the fail-closed half; `connection.rs:149-164` is a
straight-line `let ... else { return; }` placed before the
`activate_server` call at `connection.rs:170`. The reportability half has a
timing wrinkle: `connection.rs:158-164` wraps `prepare` in `timeout_at`, and
`spawn_blocking` work cannot be cancelled, so on timeout the blocking task
continues, `prepare` succeeds, and the resulting `PreparedRing` is dropped
inside the blocking task. Dropping a `CancellationToken` does not cancel it, so
teardown falls to the mpsc-closure path at `:455-458`, and that path is only reached
through the `select!`, which requires `receive_one` to have returned
`Ok(false)`.
Required faults and enabling state: one fault per cause. Admission exhaustion
needs `max_connections` concurrent live rings. `DuplexRing::create` failure
needs shared-memory creation to fail. `worker_descriptor` failure needs
`Ring::attachment()` to fail. Thread-spawn failure needs the thread limit.
`initialized_rx.recv` failure needs the endpoint thread to die between spawn and
handshake. The timeout path needs `prepare` to exceed
`transport_setup_deadline`.
Confidence: high - [evidence](evidence/ring-a-ring-unavailability-fails-closed-without-a-classified-reason.md).
Verified by inspection: `RingUnavailable` (`:103-112`) is a unit struct with a
fixed `Display` string and no cause field; only `:224` increments a counter;
`connection.rs:149-164`'s `else` branch is a bare `return` that emits no
`ServerMessage`, so the peer observes a closed setup socket and
`activate_client` (`client.rs:367`) reports the generic
`ClientError::new("setup_failed", ...)` at `client.rs:368`.
Existing check: `ring_transport.rs:884` asserts
`diagnostics["exhaustion"]["observed"] == 0` on a transport with no admissions. Nothing
covers the other four causes. Status unaudited.
Impact: the host fails closed, which is the important half and holds. But four
of five causes are invisible: a host that cannot create shared-memory objects at
all refuses every connection while reporting `state: "healthy"` with all five
counters at zero, and the client sees only `setup_failed`. That is a silent
total outage of the only datapath.
Open questions:
- Should `RingUnavailable` carry a closed cause class matching the doctor's
  five terminal classes (`docs/shm-transport.md:53-59`)?
- On the `prepare` timeout path, should the connection task cancel the ring it
  abandoned? It currently relies on sender-drop, which the `received == true`
  branch can defer (see
  `ring-a-cancellation-close-requires-an-empty-inbound-observation`).

---

## Group D: what diagnostics can and cannot witness

Two records on the observability surface an operator actually reads. The first
is a counter that can advance before the thing it is read as proving has
happened. The second is a five-class terminal taxonomy that the host does not
own at all: the classes are synthesized client-side from an observed error, and
the host's own `diagnostics()` cannot leave `state: "healthy"` on any condition
short of a poisoned mutex. Grouped because both are read as `diagnostics()`
claims, and because together they explain why the failures in Group C leave no
trace.

### ring-a-reclamation-count-does-not-witness-charge-release

Type: safety
Reachability: default-production - `record_reclamation` (`connection.rs:209`)
runs at the end of every `run_connection`, and the early return that skips the
`io_task` await (`:273-276`) is on the ordinary drain path, reached whenever
`shared.draining` is set or `shared.shutdown` is cancelled. Both are shipped
states, not configured ones.
Status: active
Exercised: not yet - needs a connection that retires while the host is already
draining, plus a read of `accounting()` at the moment the counter increments.
Guarantee: The `reclamation.completed` value that `diagnostics()` reports is
never larger than the number of connections whose admission charge has actually
been returned.
Check: `always` - at every observation, `diagnostics()["reclamation"]["completed"]`
is at most the number of endpoint threads that have executed
`admission.release()`. `always` fits because the counter is a monotone
diagnostic: a single premature increment permanently overstates reclamation, and
`docs/shm-transport.md:79` makes "active charges after reclamation" a
release gate, so an operator reading the count as a witness of release draws a
false conclusion.
Fault/timing angle: the ordering holds on the normal path and breaks on one
early return. Normally `serve_generation` awaits `io_task` at
`connection.rs:347`; `io` is `done_rx.await` (`ring_transport.rs:286-288`) and
`done_tx.send(())` runs at `:277`, after `admission.release()` at `:276`. So
`record_reclamation` at `connection.rs:209` follows the release. But
`connection.rs:273-276` returns from `serve_generation` without awaiting
`io_task`, and `io_task` is an `AbortOnDropHandle` (`connection.rs:190`), so the
awaiting task is aborted. Control returns to `connection.rs:209`, which
increments the counter while the endpoint thread may still be running. The
window is bounded only by how long `run_endpoint` takes to observe
`writer.discard()` (`connection.rs:353` via
`discard_unregistered_generation`), which the `received == true` branch can
defer indefinitely.
Required faults and enabling state: a connection that reaches
`serve_generation` and finds `shared.draining` already set or
`shared.shutdown` already cancelled - that is, a connection accepted and
authenticated during the shutdown sequence.
Confidence: high - [evidence](evidence/ring-a-reclamation-count-does-not-witness-charge-release.md).
Verified by inspection: `connection.rs:208-209` places `record_reclamation`
after the `serve_generation` await, and an inner `return` at `:275` still
returns there; `AbortOnDropHandle` aborts on drop; `ring_transport.rs:276-277`
orders release before the done signal.
Existing check: `ring_transport.rs:883` asserts
`diagnostics["reclamation"]["completed"] == 1` after a direct
`record_reclamation()` call, which exercises the counter and not the ordering.
Status unaudited.
Impact: the exact metric a release gate would read as proof that charges came
back can be incremented before they did. The gate would pass on a host that is
in fact still holding the charge.
Open questions:
- Should `record_reclamation` move onto the endpoint thread, immediately after
  `admission.release()`, so the counter is release-witnessed by construction?

### ring-a-host-doctor-emits-one-of-five-declared-terminal-classes

Type: reachability
Reachability: test-only - the host counters and the healthy arm of `diagnostics()`
(`ring_transport.rs:142-196`, ungated) are default-production, but the guarantee's
subject, a `daemon doctor` report carrying one of the five terminal classes, is
produced by the plugin package (`packages/plugin/src/shared/host-client/shared-memory-failure.ts`
and `host-lifecycle/policy.ts` in the source repository), and `packages/` in this
tree holds only `shm-native`; no `classifySharedMemoryFailure`,
`terminalSharedMemoryDiagnostics`, or `daemon doctor` implementation exists here.
Until the plugin lands, the five outcomes can be reached only by a test that
stands in for the classifier. The plugin file citations below are source-repository
evidence, kept for the wave that reclassifies this record.
Status: active
Exercised: partial - `ring_transport.rs:867-868` asserts the healthy shape end
to end; no campaign drives the doctor to a terminal outcome in any of the five
classes.
Guarantee: A campaign reaches at least one end-to-end `daemon doctor` outcome in
each declared terminal class, so the five-class taxonomy the operator contract
promises is a set of situations that actually occur rather than a set of names.
Check: `sometimes` - at least once per campaign, for each of `missing_addon`,
`identity_mismatch`, `setup_failure`, `peer_death`, and `resource_exhaustion`,
observe a completed `daemon doctor` report whose `shared_memory.error_class`
equals that class, produced by the real classification path from a real host or
addon condition rather than by constructing the value; in this tree that path is
absent, so the check is deferred with the record's reachability and no in-tree
campaign can discharge it yet. **This record previously
claimed `reachable` over "five distinct emission points" in the host, and both
the boundary and the semantics were wrong.** There are no five host emission
points to reach, and there never were: the terminal report is synthesized
**client-side**. `classifySharedMemoryFailure`
(`packages/plugin/src/shared/host-client/shared-memory-failure.ts:10-30`) maps
an observed error into `SharedMemoryTerminalClass` (`types.ts:68-73`), and
`policy.ts:648-672` feeds that into `terminalSharedMemoryDiagnostics`
(`policy.ts:854-872`), which builds the entire terminal object including
`error_class`, `bounds`, `peer_death`, and `exhaustion` without consulting the
host at all. Exactly one of the five literals exists in Rust, `"setup_failure"`
at `ring_transport.rs:173`, and it is the host's own poisoned-mutex arm rather
than a member of the client taxonomy. So `reachable` was location coverage over
locations that do not exist. The five classes are **situations** - an addon that
will not load, an identity that does not match, a setup that failed, a peer that
died, a resource that ran out - and METHOD.md's rule is that situation coverage
is `sometimes`. Reaching the classifier's lines proves nothing: a campaign can
execute `classifySharedMemoryFailure` on a constructed error and never produce
the operational state the class names, which is exactly what the existing
TypeScript coverage does.
Fault/timing angle: none for the classification itself, which is a total
function on an observed error. The window that matters is per class: each needs
its own host or addon condition to exist while the doctor runs.
Required faults and enabling state: one condition per class, and they do not
share a mechanism. `missing_addon` needs a load that fails to find the packaged
addon, which is 2c's S6 and is structurally suppressed in CI by `ci.yml:193`'s
`build:source`. `identity_mismatch` needs a `connect_setup` failure carrying
that message. PR #131 split `shm-native`'s `lib.rs` into `lifecycle`,
`napi_buffers`, `scheduling`, and `setup` modules; the pre-merge citation
`lib.rs:579-587` now lands in `RingGrant` decode code, and the identity check
lives at `packages/shm-native/src/setup.rs:229`, with the message itself
built at `setup.rs:413-416`. `setup_failure` is the classifier's default arm and is
reachable from any other native startup failure. `peer_death` needs an `ECONNRESET`, `EPIPE`, or unexpected-EOF error
from a peer that died, which the coarse kill harness already produces. And
`resource_exhaustion` needs a `memory_cap` code or a capacity message, which
admission exhaustion produces at `ring_transport.rs:223-226`.
Confidence: high - [evidence](evidence/ring-a-host-doctor-emits-one-of-five-declared-terminal-classes.md).
Verified by grepping the five literals across `crates` and `packages` and then
reading the client path end to end for this disposition: `"setup_failure"`
appears in Rust only at `ring_transport.rs:176`; the other four appear only in
TypeScript, at
`packages/plugin/src/shared/host-client/types.ts:68-73` and
`shared-memory-failure.ts:14-30`. `policy.ts:669-671` calls
`terminalSharedMemoryDiagnostics(classifySharedMemoryFailure(error))`, and
`terminalSharedMemoryDiagnostics` (`policy.ts:854-872`) hard-codes `state:
"terminal"`, zeroes `bounds`, sets `accounting: null`, and derives
`peer_death.observed` and `exhaustion.observed` from the class it was handed. So
the terminal shape is not a host projection at all. `peer_death` and
`resource_exhaustion` do exist host-side, but only as counters
(`ring_transport.rs:191`, `:193`), and `state` stays `"healthy"` while
`exhaustion.observed` is non-zero because the host `match` keys on
`accounting()` alone.
Existing check: `ring_transport.rs:859-897` covers the host's healthy branch and
its lifecycle counters - four post-#131, since `record_attachment` and the
`attachment` field were removed with the eventfd rewrite - and it is a
host-side check that cannot reach the client
taxonomy at all. On the client side,
`packages/plugin/src/shared/host-client/shm-frame-channel.test.ts:47-58`
`shared-memory failures collapse to five terminal diagnostic classes` reaches
all five classes, but every one of its nine cases is a hand-constructed `new
NativeStartupError(...)` or `new Error(...)` (`:49-57`) rather than a produced
condition, so it is location coverage of the classifier and not situation
coverage of the classes. Status unaudited.
Impact: `docs/shm-transport.md:53-59` promises the operator a five-class
terminal taxonomy from `eidnara daemon doctor`. That contract is real and
it is met by the client, not by the host, and this record's earlier framing -
that four of five producers "do not exist at all" - was an artifact of looking
for them in Rust. What remains true and matters is narrower: a host that has
refused every connection for capacity, or lost every endpoint thread to a hook
panic, still reports `state: "healthy"` from `diagnostics()`, so the client's
classifier only ever sees a terminal condition when its *own* call fails. A host
that is unhealthy but still answering produces no terminal class from either
side.
Open questions:
- The five-class taxonomy is the client's, and the doc attributes it to
  `eidnara daemon doctor`. Should the host's `diagnostics()` also derive a
  class from its own counters, so an unhealthy-but-answering host is
  classifiable? Today `state` keys on `accounting()` alone
  (`ring_transport.rs:165-179`) and no counter can move it. (needs human input)

---

## Group E: the inbound loop, its lease, and its cancellation bound

Three records on `receive_one` and the loop that drives it. One is the
asymmetry that a lease-release failure is reported only on the paths that go on
to deliver a frame. One is the bound on how long a cancelled generation keeps
consuming. One is the operational state in which a held lease and an outbound
publication coincide, which is the enabling state for the other two. Grouped
because all three live inside the same `:380-533` region and the third is a
precondition of the first.

### ring-a-lease-release-failure-is-observable-only-on-the-success-path

Type: safety
Reachability: default-production - `receive_one` (`ring_transport.rs:487`) runs
on the endpoint thread of every authenticated connection, and all five
lease-holding return points (`:509`, `:525`, `:531`, `:539`, `:548`) are on that
ungated path. `ReceiveLease::Drop` (`lease.rs:201-206` post-#131) is likewise
unconditional.
Status: active
Exercised: not yet - needs a `release` that fails, which needs a quarantined or
identity-mismatched ring while a lease is held. **Constructible today**; see
`Required faults` for the mechanism, which this record originally recorded as
unavailable.
Guarantee: A receive-lease completion failure is reported on every inbound path
that holds a lease, not only on the paths that go on to deliver a frame.
Check: `always` - for every `receive_one` invocation that acquired a lease, if
the underlying `Ring::release` returns `Err` then the invocation returns a
`ReadClose` other than the cause it would have returned had the release
succeeded. `always` fits because a lease that fails to release does not free
its slot, so the loss is cumulative against `max_leases` = 8
(profile-pinned post-#131; asserted at `ring_transport.rs:907`) and eight
silent failures wedge the direction.
Fault/timing angle: the two explicit release calls (`:507-509` for the oversize
rejection and `:546-548` on the delivery path) map `Err` to
`ReadClose::Corrupt("shared-memory completion failed")`. The three early
returns that hold a lease do not: `Cancelled` at `:525`, `Overloaded` at
`:531`, and `Cancelled` at `:539` all drop the lease, and
`ReceiveLease::Drop` (`crates/shm-transport/src/lease.rs:201-206`) calls
`release_once` and discards its `Result`. So exactly the paths taken under
cancellation and overload - the paths most likely to coincide with a stressed or
quarantined ring - are the ones that cannot report a completion failure.
Required faults and enabling state: a held lease **and** a release failure.
`Ring::release` returns `Err` on quarantine (`ring.rs:1176-1178`), wrong incarnation
(`:1179-1181`), wrong lane (`:1182-1184`), stale sequence (`:1193-1196`), and duplicate
release (`lease.rs:186`). **Quarantine is reachable directly from the peer, and
this record previously said it was not.** The original text required "a peer that
publishes a malformed descriptor" so `try_receive` would quarantine from inside
the transport, and recorded that as unavailable. It is not the only route.
`Ring::enter_quarantine` is a **public** method (`ring.rs:1373-1378`, in
`crates/shm-transport/src/backend/ring.rs`) that stores the flag on the shared
lifecycle page, and a peer already holds the ring it needs: `RingClientEndpoint`
exposes `to_host` and `from_host` as `pub` fields (`ring_transport.rs:651-656`),
and the existing fixture at `tests/support/raw_client.rs` attaches one and
already reaches through those fields (`:691`, `:738`, `:781`). So the whole fault
is `endpoint.to_host.enter_quarantine()` from the test peer, one line, no seam
and no malformed producer. `Ring::release` checks `is_quarantined()` before any
other validation (`ring.rs:1176-1178`), so the host's next release on that
direction fails. Held-lease timing still needs the ingress-wait state below: park
the host inside the budget wait with a lease held, quarantine from the peer, then
let the wait exit on `Cancelled` or `Overloaded`.
Confidence: high - [evidence](evidence/ring-a-lease-release-failure-is-observable-only-on-the-success-path.md).
Verified by inspection: `receive_one`'s return points that follow a
successful `try_receive` are `:509`, `:516`, `:525`, `:531`, `:536`, `:539`,
`:545`, `:548`, `:556`, `:557`; of those, only `:509` and `:548` route a release
error;
`lease.rs:201-206` discards the drop-path `Result` with `let _ =`. Re-verified
for this disposition: `pub fn enter_quarantine(&self)` at `ring.rs:1373` writes
`quarantined` on the lifecycle page with `Ordering::Release`, `is_quarantined`
reads it with `Ordering::Acquire` (`:1381-1388`), and both directions of one
duplex pair map the same object, so the peer's store is visible to the host's
consumer.
Existing check: `crates/shm-transport/tests/ring.rs:240`
`quarantine_rejects_all_operations_and_reports_conservation` covers the
transport-side `Err(Quarantined)` from `release`, not the host's handling of it.
Status unaudited.
Impact: this is the host-side counterpart of Part 1's
`release-failure-is-observable`, which Part 1 marked `medium` confidence with
its host anchor at `shm_provider.rs:365`. That anchor is gone; the surviving
host behaviour is the asymmetry above. Scoped correctly after investigation: all
three untracked paths return an `Err(ReadClose::..)` that ends the read loop
(`:406-411`), so a silent release failure always coincides with the connection
retiring and cannot accumulate across its life. What is lost is the signal that
the ring was quarantined rather than merely overloaded, and that matters because
`ReadClose::Overloaded`'s own doc comment (`frame_channel.rs:40-43`) asserts
"the peer and the transport are healthy" - false on a quarantined ring. Today
`connection.rs:401-404` collapses `Corrupt` and `Overloaded` into the same
`ReadExit::Peer`, so the gap is latent and becomes live only if that taxonomy is
split.
Open questions:
- Should the `Overloaded` and `Cancelled` paths release explicitly and upgrade a
  release failure to `Corrupt`? Investigation found this buys nothing until
  `connection.rs:401-404` stops collapsing the two causes into one `ReadExit`,
  so the two changes travel together or not at all.
- A peer can condemn the shared ring unilaterally through the public
  `Ring::enter_quarantine` (`ring.rs:1373`) while the host holds a lease. Is that
  intended peer authority, or should quarantine be host-initiated only? It is the
  cheapest route to this record's fault and simultaneously a capability the
  threat model may not want. (needs human input)

### ring-a-cancellation-close-requires-an-empty-inbound-observation

Type: liveness
Reachability: default-production - the `received == true` branch
(`ring_transport.rs:415-421`) and the `select!` at `:441-474` are the endpoint
loop every authenticated connection runs, and `read_cancel` is a child token of
the generation root created in `prepare` (`:227-228`) for every connection.
`frame_deadline` (`config.rs:165`, 30 seconds) ships with a default.
Status: active
Exercised: partial - `budget_wait_observes_read_cancellation`
(`ring_transport.rs:1008-1043`) covers cancellation observed *inside* the
ingress-charge wait, which is the one path that does not need an empty
observation, and the post-#131 test
`finish_wakes_after_read_cancellation_with_unread_peer_data` (`:809-846`)
drives the main loop's report once: it cancels `read_cancel` on an empty ring,
asserts the receiver observes `Err(ReadClose::Cancelled)`, then proves the
finishing loop still wakes with unread peer data. Neither asserts the
frame-count drain bound under sustained traffic.
Guarantee: After the generation is cancelled and the peer stops publishing, the
endpoint thread reports `ReadClose::Cancelled` and exits within a bounded number
of further inbound frames, provided the connection task is still draining the
inbound channel.
Check: `always` - evaluated at the end of an explicit bounded window: run
sustained inbound traffic, cancel `read_cancel`, **stop the peer's publication
and let the peer-to-host ring drain**, then poll until the endpoint thread has
exited. Assert two bounds, both counted in frames rather than in wall-clock
time. First, the thread performs at most `N + 1` further `receive_one`
invocations, where `N` is the number of frames the peer committed before it
stopped publishing, snapshotted at the publication stop rather than at the
cancellation edge: the loop at `ring_transport.rs:384-409` calls `receive_one`
before it checks `read_cancel`, so a frame committed after the edge but before
the stop is forwarded on its `Ok(true)` pass and counts toward `N`. The first
empty observation returns `Ok(false)` and reaches the `read_cancel` check
(`:397`), which takes the `inbound` sender (`:398`) and sends `Cancelled`
(`:400`). Second, nothing is forwarded on the inbound channel after `Cancelled`
is sent. Third, because the guarantee holds only while the connection task
drains the inbound channel, the campaign keeps that drain running, and the bound is
stated in the unit the code bounds: with the receiver draining, `send` on the
bounded `inbound` channel (`:400`) completes as soon as one slot is free, so the
check asserts the `Cancelled` report is received by the drain within
`queue_frames + 1` receives after the cancellation edge (the channel holds at
most `queue_frames` earlier frames, `ring_transport.rs:227`), and that the
endpoint thread has exited by the time that report is received plus one join;
the send carries no wall-clock deadline, so with the drain stopped the record
makes no exit claim and the check does not run. `always` rather than `sometimes`
because the assertion is a bound that must hold every time the window closes,
not a state to reach.
**Re-derived 2026-08-31 against the eventfd transport (PR #131), which removed
`POLL_INTERVAL`.** The polling-era record had already withdrawn its invented
wall-clock bound; the rewrite makes the frame-count unit the only honest one
left, because the empty-ring wait is now event-driven rather than periodic.
`frame_deadline` still bounds exactly one thing inside `receive_one`, now the
charge wait: `let deadline = Instant::now() + frame_deadline` at `:516` is
consumed by the `sleep_until` arm at `:524-529`, exiting `Overloaded` at
`:528`. It bounds nothing else in the loop. The cancellation report itself is
still an unbounded await: `:400` does
`inbound.send(Err(ReadClose::Cancelled)).await` on a bounded `mpsc` channel of
`queue_frames` capacity (`:227`), and if the connection task is not draining,
that send parks with no deadline. The rejection and frame-delivery sends at
`:507-512` and `:548-553` have the same shape. So the residual wall-clock
question is unchanged by the rewrite and stays recorded as unresolved in the
open questions.
Fault/timing angle: the drain-before-report design survived the rewrite. The
`received == true` branch (`:415-421`) checks neither `discard`, `finish`,
`root`, nor `read_cancel`; `read_cancel` is observed in exactly three places:
the `Ok(false)` branch (`:400-404`), the biased `select!` arm at `:448-454`
(whose comment restates the intent: re-enter the receive path once, drain
frames committed before the cancellation edge, then report `Cancelled` "after
the first empty observation"), and the charge wait inside `receive_one`
(`:525`). The consequence is unchanged: a peer that keeps the peer-to-host ring
non-empty defers the cancellation report for as long as it keeps publishing.
What the rewrite changed is the empty-ring wait. Instead of sleeping
`POLL_INTERVAL`, the loop arms the transport's wake protocol
(`rings.second.arm_data_wait()` at `:429`, `ring.rs:828-854`) and parks on the
duplicated doorbell descriptor (`duplicate_data_ready` wrapped in an `AsyncFd`
at `:371-380`; the `readiness.readable()` arm at `:459-471` clears readiness
and calls `complete_data_wait`). The wake protocol is the new timing surface:
`arm_data_wait` publishes a parked epoch and re-checks data availability before
returning `true`, and the producer's `signal_wake` (`ring.rs:1418-1432`) rings
the doorbell only when a parked epoch is visible, so a frame committed between
the host's arm and its park is delivered by the doorbell rather than lost. A
lost or racing wake would not defer the cancellation *report* - the
`read_cancel.cancelled()` arm is a `CancellationToken`, not an eventfd, and the
post-cancellation drain calls `try_receive` directly (`:496-498`) - but it
would strand committed pre-cancellation frames the drain contract says to
deliver. That failure mode is new with #131 and is covered in the evidence
file's failure scenario.
Required faults and enabling state: an attached peer publishing continuously,
enough ingress budget that each `charge` future (`:520-521`) resolves
immediately, and a cancellation of `root` or `read_cancel` from the host side
while that traffic continues. `connection.rs:183-189`'s peer-death handler is
one natural trigger, since it cancels `peer_gen.token` - the ring's `root` -
while frames may still be queued in the ring. Closing the window additionally
needs the peer to stop publishing, which is a fixture choice, and the
connection task to keep draining, which is the assumption the bound is
conditional on.
Confidence: medium - [evidence](evidence/ring-a-cancellation-close-requires-an-empty-inbound-observation.md).
The code structure is verified by inspection at post-#131 HEAD and the intent
is stated in the comment at `:449-453`. Re-verified for this pass:
`frame_deadline` is consumed only at `:519` and `:527-532` inside `receive_one`
and at `publish_one`'s own reservation deadline (`:583`), and all three
`inbound.send(..).await` sites (`:402`, `:510-515`, `:551-556`) are
undeadlined. What I did not verify is the exact behaviour of `read_loop` under
cancellation, so I cannot state whether the host reliably stops draining and
closes the inbound channel promptly; that is why this is medium and not high,
and it is the first open question below.
Existing check: `ring_transport.rs:1008-1043`
`budget_wait_observes_read_cancellation` covers the charge-wait path, and
`:809-846` `finish_wakes_after_read_cancellation_with_unread_peer_data` covers
the empty-ring report plus the post-cancellation finishing wake. `host-runtime`
inline tests run in CI in this tree (`ci.yml:118`, `:126`, `cargo test --workspace --all-targets`). Status unaudited.
Impact: a cancelled generation's endpoint thread can keep consuming and
forwarding peer frames after the close decision. Since the charge is released
only when the thread exits (`:276`), a peer that floods during teardown extends
the window in which a retiring connection still holds its full admission
charge, which is exactly the pressure that turns an ordinary retirement into
`RingUnavailable` for the next connect.
Open questions:
- Does `read_loop` stop draining the inbound channel promptly on
  `read_cancel`, closing the channel and bounding this window? That is in
  Part 2a's `connection.rs` scope and I did not resolve it. Until it is resolved,
  the case where the channel neither closes nor drains has **no bound at all**:
  the `Cancelled` report parks on `inbound.send(..).await` at `:402`. That
  residual is recorded as unresolved rather than given a wall-clock stand-in.
- Should the `received == true` branch check `root.is_cancelled()`, at the cost
  of dropping frames the current comment deliberately drains?
- Should the three `inbound.send(..).await` sites carry a deadline, so a report
  cannot outlive the generation that produced it? Today only the charge wait is
  deadlined (`:519`, `:527-532`).

### ring-a-ingress-wait-holds-a-lease-while-servicing-egress

Type: reachability
Reachability: default-production - the charge wait
(`ring_transport.rs:519-542`) is entered whenever `ingress.charge(header.len)`
(`:520`) cannot resolve immediately against the process-wide `ByteBudget` built
at `runtime.rs:761-767` and cloned into every connection at
`connection.rs:113`. The budget is derived from `max_resident_bytes` with
shipped defaults, so no opt-in is needed to reach the wait; sustained ingress
pressure is sufficient.
Status: active
Exercised: not yet - no test holds a lease across a saturated ingress budget
while an outbound frame is published from inside that wait.
Guarantee: The state in which one receive lease is held across a saturated
ingress-budget wait while the same loop publishes a queued outbound frame occurs
at least once.
Check: `sometimes` - at least once per campaign, observe both preconditions
jointly: `receive_one` is parked inside the charge `select!` at `:519-539`,
meaning a lease is held (bound at `:493-498`) and the `charge` future
(`:517-518`) has been polled pending at least once; and the publish arm at
`:530-537` executed during that same invocation. `sometimes` rather than
`reachable` because executing those lines is not the point: a campaign can run
the charge-wait branch and the publish-from-wait branch in separate invocations
without ever producing the operational state in which they coincide. Per the
METHOD coverage rule this asserts the independent preconditions, not a
violation, so the marker still fires on a correct implementation.
**Re-derived 2026-08-31 against the eventfd transport (PR #131), which removed
`POLL_INTERVAL`.** The wait is no longer a 50-microsecond poll loop over
`try_charge`: `ByteBudget::charge` (`wire.rs:397-407`) queues on a tokio
semaphore and resolves when another holder's `ByteCharge` drops, so the wait
parks instead of spinning, and the third polling-era precondition (covering the
`POLL_INTERVAL` sleep on a second, empty-queue iteration) no longer exists.
Fault/timing angle: this is the state where the ingress budget and the outbound
deadline interact. The ingress budget is process-wide, a single `ByteBudget`
built at `runtime.rs:761-767` from `config.limits.max_resident_bytes` minus the
egress, scratch, catalog, and retained reservations, and cloned into every
connection at `connection.rs:113`, so pressure originating elsewhere in the
host stalls this receive. The `select!` at `:522-542` is biased: `read_cancel`
first (`:525`, exiting `Cancelled`), then the `charge` future, then the
absolute deadline (`:527-532`, `sleep_until` on the `Instant` taken at `:519`,
exiting `Overloaded` at `:531`), then queued outbound frames through
`queue.recv()` (`:533-540`), whose publish failure exits
`Corrupt("shared-memory publish failed")` at `:536`. The polling-era comment
that justified servicing egress from inside the wait was removed with the
rewrite; the surviving statement of the same intent is `run_endpoint`'s
alternation comment at `:416-420`. Scoped after investigation: an earlier draft
also required `active_leases == max_leases` on the peer-to-host direction. That
is unreachable and the clause is dropped. `receive_one` holds at most one lease
at a time, every return path releases or drops it, and `run_endpoint` calls
`receive_one` serially, so the host's contribution to `active_leases` is
bounded by one against a budget of eight - pinned post-#131 by the profile
rather than by file-local constants, and asserted by
`ring_profile_pins_per_connection_grant_geometry` (`:901-907`).
Required faults and enabling state: an ingress budget too small for the frame
in hand, so the `charge` future stays pending; and at least one queued outbound
frame while it is pending, so `:533-540` runs. No fault at all: both are
fixture parameters.
Confidence: high - [evidence](evidence/ring-a-ingress-wait-holds-a-lease-while-servicing-egress.md).
Verified by inspection at post-#131 HEAD: the lease is bound at `:496-501` and
not released until `:546-548`, so it is live for the whole `:519-542` wait; the
publish-from-wait arm is `:533-540`; the deadline exit is `:527-532`;
`run_endpoint` calls `receive_one` serially at `:386-397`, and every
`receive_one` return path releases or drops its lease.
Existing check: two inline tests are each one precondition short.
`copied_control_frame_records_one_host_adapter_copy` (`:961-1005`) uses
`ByteBudget::new(1024)` (`:994`), so the charge resolves immediately and the
wait is never entered. `budget_wait_observes_read_cancellation` (`:1008-1043`)
uses `ByteBudget::new(0)` (`:1028`) and does park in the wait, but its sender
queue is empty (`:1024-1026`), so `:533-540` never runs. Neither runs in CI.
Status unaudited.
Impact: if this state is never reached, three mechanisms are untested together.
The `Overloaded` exit at `:527-532`, whose `ReadClose::Overloaded` doc
(`frame_channel.rs:40-43`) asserts "the peer and the transport are healthy" - an
assertion this record's window can falsify. The outbound servicing whose intent
the alternation comment at `:416-420` states. And the longest window in which
host code holds a reference into shared storage, which is where Part 1's
`quarantine-authority-survives-peer-writes` scenario has the most room. It is
also the enabling state for
`ring-a-lease-release-failure-is-observable-only-on-the-success-path` and for
observing the `Corrupt`-versus-`CleanEof` asymmetry in
`ring-a-publish-failure-is-reported-as-a-clean-peer-close`, so leaving it
unreached leaves both unfalsifiable.
Open questions:
- Should `receive_one` distinguish "ring empty" from "leases saturated"? Both
  arrive as `Ok(None)` from `try_receive` (`ring.rs:1063-1068`, `:1073-1074`)
  and both collapse to `Ok(false)` at `:500-501`. Investigation found this is
  moot under the current single-active-lease design and would matter only for a
  concurrently-leasing consumer, so it is a latent API gap rather than a live
  one.

---

## Group F: taxonomy arms with no producer

Two records on machinery the refactor left behind. One `ReadClose` variant and
one `InboundFrame` constructor have no producer at `HEAD`, and each keeps a
downstream branch alive that no input can reach. Neither is a current defect.
Both are catalogued because a dead arm behind an `#[allow(dead_code)]` or a
stale `reason` string reads as coverage, which is how it survives review.

### ring-a-rejected-drain-failure-close-has-no-producer

Type: reachability
Reachability: default-production - the consumer side runs on every connection, but the subject is compiled with no production producer. Stated rather than
defaulted: `connection.rs:391` and
`:397` are on the ungated read-exit match every connection runs. The subject,
`ReadClose::RejectedDrainFailed` (`frame_channel.rs:47`), has no producer
anywhere in the tree, in production or in test, so `ReadExit::PeerKeepQueue`
(`connection.rs:355-358`) and the `serve_generation` arm at `:281-285` are compiled
and unreachable. `#[allow(dead_code)]` on the enum (`frame_channel.rs:32`) is
what keeps that compiling silently.
Status: active
Exercised: not yet - unconstructible; no test can reach it without a code
change.
Guarantee: Every `ReadClose` variant the connection engine handles is
producible by the transport, so the engine's close taxonomy has no dead arm and
`docs/host-wire-protocol.md:321`'s authoritative-early-terminal guarantee
has a live carrier.
Check: `reachable` - for every `ReadClose` variant the engine handles
(`frame_channel.rs:32-45`: `CleanEof`, `Corrupt`, `Cancelled`, `Overloaded`,
`Io`, `RejectedDrainFailed`), the engine arm that consumes it is executed at least
once per campaign, asserted per variant; the two arms the producer census found
unproduced are the `Err(ReadClose::RejectedDrainFailed)` arm at
`connection.rs:355-358`, which is the only producer of `ReadExit::PeerKeepQueue`,
and the `Err(ReadClose::Io(_))` arm at `:364`; `connection.rs:397` is the
ordinary oversize-rejection task and must not be used as the marker, since an
ordinary rejection would satisfy it while the close variant stays dead. `reachable` fits because the claim is
location coverage over each branch, and the finding is that no input can reach
those two.
Fault/timing angle: none. Static producer enumeration.
Required faults and enabling state: for the branch to be reachable at all, the
transport would have to emit `ReadClose::RejectedDrainFailed` after an
oversize channel-0 rejection whose realignment failed. On the ring there is no
realignment: a frame is one descriptor, and `receive_one:475-477` releases the
lease and returns `Ok(true)` with no drain step.
Confidence: high - [evidence](evidence/ring-a-rejected-drain-failure-close-has-no-producer.md).
Verified by grepping both variants: `ReadClose::RejectedDrainFailed` appears at
`frame_channel.rs:47` (declaration) and `connection.rs:391` (consumer) and
nowhere else; `ReadClose::Io` appears at `frame_channel.rs:45` and
`connection.rs:364` and nowhere else. `ReadExit::PeerKeepQueue` is produced only
at `connection.rs:355-358`, so the `serve_generation` arm at
`connection.rs:281-285` plus the `reject_written` bookkeeping at
`connection.rs:349`, `:357`, and `:393` are dead. `#[allow(dead_code)]` on the `ReadClose` enum
(`frame_channel.rs:32`) is what keeps this compiling silently.
Existing check: none. Part 2a's
`the-client-body-budget-refusal-drain-is-never-entered` is the closest analogue
and was written against the deleted `frame_read.rs`.
Impact: the wire contract at `docs/host-wire-protocol.md:321` promises that
an early oversize-control terminal "is authoritative for its correlation even
if the declared body then truncates, stalls, or EOFs". On the ring that
promise is satisfied vacuously, since there is no separate body to truncate,
but the engine still carries the machinery that would have honoured it. The
risk is not a current defect; it is that the dead arm looks like coverage.
Open questions:
- Should `RejectedDrainFailed` and `Io` be removed, or retained for a future
  transport? Removing them would make Part 2a's drain records genuinely closed
  rather than superseded.

### ring-a-segmented-inbound-body-has-no-production-producer

Type: reachability
Reachability: default-production - the host inbound path runs on every connection, but the subject is compiled with no production producer. Stated rather than
defaulted: the host inbound path and always takes the
`owned` constructor (`ring_transport.rs:552`). The subject,
`InboundFrame::segmented` (`frame_channel.rs:477`), has zero call sites
tree-wide including tests, so `ReceiveBody::Segmented` (`:448`) is
unconstructible and `decode_contiguous`'s `None` arm (`connection.rs:586`) is
compiled and unreachable.
Status: active
Exercised: not yet - unconstructible from any host path.
Guarantee: The zero-copy segmented inbound path that the frame-channel
abstraction and the transport doc both describe has a production producer, so
the copy accounting and the wrap-around lease handling are exercised.
Check: `reachable` - the code location `InboundFrame::segmented`
(removed at HEAD; see the evidence file) is executed at least once per campaign.
`reachable` fits because this is location coverage; the derived state claim,
that every host inbound frame carries exactly one copy, is what a cheaper
screen would assert.
Fault/timing angle: none. Static producer enumeration. The interesting
consequence is that a body wrapping the arena end is copied twice on the peer
side of the in-process client (`client.rs:1878` charges then
`try_recv_with` calls `lease.to_vec()` at `ring_transport.rs:735`) and once on
the host side, and neither ever takes the segmented path.
Required faults and enabling state: for the segmented path to matter at all, a
body whose descriptor spans two arena ranges, which the transport produces when
`span_count == 2` (`ring.rs:1105-1112`). That is reachable: it needs a body that
straddles the arena wrap point. But `receive_one` collapses it with
`lease.to_vec()` (`:544`) before the host ever sees the span structure.
Confidence: high - [evidence](evidence/ring-a-segmented-inbound-body-has-no-production-producer.md).
Verified by grepping: `InboundFrame::segmented` has zero call sites in the
tree, including tests. `ReceiveBody::Segmented` (`frame_channel.rs:448`) is
therefore unconstructible, so `with_lease` (`:506-513`) always takes the
`Owned` arm and `decode_contiguous`'s `None` arm (`connection.rs:586`) is dead.
`ring_transport.rs:552` is the only `InboundFrame` constructor call on the
host path and it uses `owned`.
Existing check: `frame_channel/contract_tests.rs:141` calls `with_lease` and
asserts `lease.segment(0)`, on a hand-built frame. Status unaudited.
Impact: two things. First, the attribute at `frame_channel.rs:476` reads
`#[allow(dead_code, reason = "shared-memory backends supply wrapped bodies")]`
and that reason is false at `HEAD`: the shared-memory backend supplies `owned`.
A stale suppression reason is how a genuinely dead branch survives review.
Second, `docs/shm-transport.md:19` says the receiver "validates the
descriptor and header before exposing a scoped lease", which is true of the
transport but not of the host boundary: the host exposes a lease over its own
copy.
Open questions:
- Is the segmented path intended to return, or should
  `InboundFrame::segmented`, `ReceiveBody::Segmented`, and
  `frame_channel::LeaseTracker` be deleted together? `LeaseTracker`
  (`frame_channel.rs:398-444`), `frame_channel::ProducerReservation`
  (`:117`), and `ProducedBody` (`:231`) are in the same position: test-only,
  with `ProducedBody::into_charge` (`:283`) having no caller at all.

---

## Relationship map

Grouped by shared mechanism rather than by the headings above, because the
sharpest relationships cross groups. **Every dominance statement below is a
hypothesis** about which oracle subsumes which, offered to order the work, not a
verified claim. None has been tested, because no check in this sub-part executes
in CI beyond the two `compile_fail` doctests, and neither doctest touches any of
these records.

- **One charge, four ways to lose track of it.**
  [ring-a-admission-charge-releases-on-every-endpoint-thread-exit](#ring-a-admission-charge-releases-on-every-endpoint-thread-exit),
  [ring-a-host-never-quarantines-an-admission-charge](#ring-a-host-never-quarantines-an-admission-charge),
  [ring-a-reclamation-count-does-not-witness-charge-release](#ring-a-reclamation-count-does-not-witness-charge-release),
  [ring-a-cancellation-close-requires-an-empty-inbound-observation](#ring-a-cancellation-close-requires-an-empty-inbound-observation).
  All four turn on `admission.release()` at `ring_transport.rs:276` being the
  single point where the charge comes back, and on that line sitting outside the
  `catch_unwind` at `:264-275`. That line is an owner, not an absence: the charge
  returns on every exit including a swallowed panic. The release record is about
  whether it returns on *every* path, the quarantine record about whether
  returning it is even the right answer for a condemned ring, the reclamation
  record about a counter that can claim it returned before it did, and the
  cancellation record about how long the return can be deferred. **These four are
  not mutually consistent until the release-versus-quarantine policy question is
  answered**, because the release record requires an unconditional return and the
  quarantine record asks whether a condemned ring is an exception to it. That is
  bias 2 in [portfolio-evaluation.md](ring-datapath/portfolio-evaluation.md). Hypothesis: an
  oracle that reads `snapshot().active` before and after each connection
  *dominates* the reclamation record, because a release-witnessed delta makes the
  counter's ordering observable as a side effect. It dominates neither the
  quarantine record, which is a call-graph absence no runtime delta can reveal,
  nor the cancellation record, which is a bound on frames rather than a claim
  about a total.
- **A cause that existed and was thrown away.**
  [ring-a-publish-failure-is-reported-as-a-clean-peer-close](#ring-a-publish-failure-is-reported-as-a-clean-peer-close),
  [ring-a-endpoint-thread-panic-is-reported-as-orderly-completion](#ring-a-endpoint-thread-panic-is-reported-as-orderly-completion),
  [ring-a-ring-unavailability-fails-closed-without-a-classified-reason](#ring-a-ring-unavailability-fails-closed-without-a-classified-reason),
  [ring-a-host-doctor-emits-one-of-five-declared-terminal-classes](#ring-a-host-doctor-emits-one-of-five-declared-terminal-classes).
  This is one finding attacked from four sides, and it is the cluster an operator
  would feel first. `publish_one` erases four distinct failure causes to `()`
  (`:560-565`, discarded at `:588-590`); the outer `catch_unwind` erases a panic
  with `let _ =` (`:264`); `RingUnavailable` is a unit struct with no cause field
  (`:103-112`); and `diagnostics()` has two arms and no counter can move `state`
  off `"healthy"` (`:165-179`), so the client classifier that owns the five-class
  taxonomy only ever sees a terminal condition when its own call fails.
  Hypothesis: giving `RingUnavailable` and `publish_one` a shared cause enum,
  surfaced through `diagnostics()`, would dominate all four, because each
  record's oracle reduces to "a host-observable record names this cause". Fixing
  the client taxonomy alone dominates none of them: the classes already exist and
  are already reachable client-side, and nothing host-side populates them.
- **The lease and the window that makes its failure visible.**
  [ring-a-ingress-wait-holds-a-lease-while-servicing-egress](#ring-a-ingress-wait-holds-a-lease-while-servicing-egress),
  [ring-a-lease-release-failure-is-observable-only-on-the-success-path](#ring-a-lease-release-failure-is-observable-only-on-the-success-path),
  [ring-a-publish-failure-is-reported-as-a-clean-peer-close](#ring-a-publish-failure-is-reported-as-a-clean-peer-close).
  The ordering here is not a preference, it is a dependency the ingress-wait
  record states in its own `Impact:` line. Reaching the state where a lease is
  held across a saturated budget while an outbound frame publishes is the
  enabling state for observing a release failure on a cancellation or overload
  path, and it is also where the `Corrupt`-versus-`CleanEof` asymmetry becomes
  observable, because `:535-537` is the one publish-failure site that returns a
  distinguishable cause. Hypothesis: constructing the ingress-wait state
  *dominates* the enabling half of the other two, in the specific sense that
  neither is falsifiable until it exists. It does not dominate their oracles: a
  release failure additionally needs a quarantined ring. That is now known to be
  cheap rather than blocked - a peer calls the public `Ring::enter_quarantine`
  (`ring.rs:1373`) through `RingClientEndpoint`'s `pub` ring fields
  (`ring_transport.rs:651-656`) - so the two records compose into one fixture
  rather than two capabilities.
- **Ownership as the premise, not a finding.**
  [ring-a-endpoint-thread-solely-owns-both-ring-endpoints](#ring-a-endpoint-thread-solely-owns-both-ring-endpoints),
  [ring-a-no-producer-retains-a-committed-release-identity](#ring-a-no-producer-retains-a-committed-release-identity).
  Both are static and both currently hold. They are in the catalog as premises
  the other twelve records assume: single-thread confinement is what makes the
  transport's unsynchronized cursors safe, and the discarded release identity is
  what keeps the producer-side release contract unreachable. Both are now
  `safety`/`always`; the second was retyped from `reachability`/`unreachable`
  under the portfolio disposition, because a provenance restriction on an
  *executed* function is a state and not a forbidden location. Hypothesis: a
  compile-time enforcement of confinement, on the model of the two `ReceiveLease`
  `compile_fail` doctests at `frame_channel.rs:296-308`, would dominate the
  runtime form of the first record, since those doctests are the only checks here
  CI already runs. Nothing dominates the second: a call-graph absence is proved
  by enumeration, and the only alternative is a debug counter on a path that
  should stay unentered.
- **Machinery with no input that reaches it.**
  [ring-a-rejected-drain-failure-close-has-no-producer](#ring-a-rejected-drain-failure-close-has-no-producer),
  [ring-a-segmented-inbound-body-has-no-production-producer](#ring-a-segmented-inbound-body-has-no-production-producer).
  Two unproducible surfaces, each hidden by a different suppression: an
  `#[allow(dead_code)]` on the `ReadClose` enum (`frame_channel.rs:32`) and an
  `#[allow(dead_code, reason = ...)]` whose reason is false at `HEAD` (`:476`).
  The doctor record was the third member of this cluster and **has left it**: its
  five classes are not unproducible machinery, they are client-side situations
  with a live classifier, and the third suppression the cluster named
  (`docs/shm-transport.md:53-59`, a documentation claim with no compiler
  involvement) is a doc that the client satisfies rather than a dead branch. No
  dominance relation holds between the two that remain; they are grouped because
  the same review reflex misses both. The shared oracle is a producer-enumeration
  check rather than a test, and it costs one pass over the tree per variant. Note
  what that means for their `Exercised` lines: a census proves the absence, and
  **no campaign can satisfy their `reachable` checks at all**, which is why their
  fault-map rows moved from `Yes` to `No` under the portfolio disposition.

---

## Group G: the wire header decode contract

Four records on `crates/host-runtime/src/wire.rs`, the 21-byte envelope header, its
decoder and its encoders. **All four were carried into this sub-part from the
superseded pre-refactor sub-part `part-2b-wire-and-channels`**, where they were
records 1, 2, 3 and 6 of `_lenses/lens-a-wire-format.md`. See
`part-2b-wire-and-channels/README.md` (source-tree only, not migrated)
for that directory's disposition.

They were orphaned rather than retired, and the mechanism was a scope move that
no lens followed. The re-scope retired the `wire-and-channels` label, moved
`wire.rs` into this sub-part's declared scope, and routed these four forward
expecting them to be carried unmodified. This sub-part's two lens passes then
looked at the ring transport: all fourteen records above carry the `ring-a-`
prefix and every one of their `Guarantee:` lines is about endpoint-thread
ownership, release identities, admission charges, publication failure, leases,
reclamation counts or close classification. Not one is about the codec.
`wire.rs` appears in the rest of this catalog only twice, in the scope sentence
and in the test inventory that counts its 14 in-file tests. So the codec was in
scope and uncataloged, and the absorbing sub-part's lenses never re-derived
these properties.

**This group sits after the relationship map because it was carried in a later
pass, and the relationship map above does not cover it.** No dominance relation
is claimed between these four and the fourteen. Within the group, the first
three are readings of one function and the fourth is the encode-side mirror
that nothing enforces.

**Why these four and not the other eight lens A records.** `wire.rs` is
byte-identical between the lens-era commit and `HEAD`: `git rev-parse` returns
blob `fd0bb178` for `crates/host-runtime/src/wire.rs` at `1c193ae0`, `793a973e` and
`e447c927` alike, and `wc -l` gives 973 at all three. These four cite nothing
outside `wire.rs`, `tests/protocol_vectors.rs`, and the encoder call graph. The
other eight lens A records each enumerate a consumer set the ring-transport
refactor rewrote, and they stay salvage.

**These are not Part 1's decode records, and the distinction is load-bearing.**
`part-1-shm-transport` holds `decoder-totality-over-arbitrary-bytes` and
`accepted-decode-consumes-its-declared-width`, and both are scoped by their own
`Confidence:` and `Fault/timing angle:` lines to the `crates/shm-transport`
decoders: `descriptor.rs`, `sample.rs`, `ring.rs` and `harness.rs`. That family
guards the ring's own metadata, the descriptors and samples the transport reads
before it hands anything to the host. `wire::decode_header` (`wire.rs:306`) is a
different function in a different crate over a different byte layout: the 21-byte
envelope header two hosts exchange, whose frozen prefix is specified at
`wire.rs:16-18` and whose eleven gates are listed in the first record below. The
two families meet at exactly one line, `ring_transport.rs:503`, where
`decode_header` is handed the `[u8; 21]` that `Lease::wire_header`
(`crates/shm-transport/src/lease.rs:152` post-#131) returns *after* the transport's own
decoder has already validated the descriptor. Verified at carry time:
`WIRE_V2_HEADER_BYTES` is 21 (`crates/shm-transport/src/descriptor.rs:10`)
and `wire::HEADER_LEN` is 21 (`wire.rs:28`), so the two layouts are the same
width and still different content. Part 1's records end where this group begins.
Lens A excluded Part 1's records from its own scope on exactly this ground in
its "Not re-reported here" preamble, and counting either family as cover for the
other would double-count in the wrong direction.

**Reachability for all four rests on one chain, re-verified at carry time
rather than inherited.** `decode_header` has three production call sites and one
behind a test-only hook. Production: `ring_transport.rs:503` in `receive_one`,
paired with `validate_inbound_header` at `:505`; `ring_transport.rs:730` in
`RingClientEndpoint::try_recv_with`; and `client.rs:1978` in `decode_outbound`.
The fourth, `ring_transport.rs:593`, is inside the `if let Some(hook)` branch at
`:592` and so is reached only through the test-only `PublishHook` this catalog
already labels. The ungated chain under the first of those is the one this
sub-part established against three misleading signals, in
[Reachability is `default-production`, and three signals argued
otherwise](#what-this-part-is-about): the profile literal containing "test", the
wrong `RingClientEndpoint` doc comment, and `#[doc(hidden)]` on the module. Its
anchors were re-printed here: `RingTransport` is constructed unconditionally at
`runtime.rs:876` and stored non-optionally as `HostShared.ring` (`:104`), and
every authenticated connection calls `ring.prepare(...)` at `connection.rs:148`.
`wire.rs` contains exactly two `#[cfg]` attributes, `:541` and `:646`, and
neither is on the decode path; `:541` gates the test-only `encode_frame`, which
matters to the fourth record and is recorded there.

**Citations repaired at carry time, per METHOD rule 1.** Six, across three of
the four records; the bijection record needed none. They are listed at each
record and collected here: the `reject_unknown_frame_type_and_reserved_flag_encodings`
span is `:745-774` and not `:745-773` (two records cited the short form, the
closing brace is at 774); `structural_corruption_closes_silently` was renamed to
`structural_corruption_is_rejected_before_dispatch` and moved from
`tests/protocol_vectors.rs:512` to `:351`; `pure_header_frames_accept_any_valid_priority`
moved from `:656` to `:504`; the count of production `decode_header` callers is
three and not two; `wire.rs:548` is inside a `#[cfg(test)]` encoder rather than a
production one; and the wire protocol's retirement clause is
`docs/host-wire-protocol.md:296`, not `:293`. Two cited files changed and
neither is a subject file: `tests/protocol_vectors.rs` went from 976 lines at
`1c193ae0` to 762 at `e447c927` under `63c4d277` ("refactor(shm): enforce
ring-only architecture"), which is what the earlier triage predicted for the
third record; and `docs/host-wire-protocol.md` went from 1,031 lines to 936,
which the triage did not predict and which the fourth record cited. One open
question was also resolved rather than repaired, in the fourth record: the route
allocator cannot mint an epoch-0 handle.

### decode-header-is-total-over-arbitrary-bytes

Type: safety
Reachability: default-production - `decode_header` (`wire.rs:306`) has three
production call sites, all on ungated paths: `ring_transport.rs:503` in
`receive_one`, `ring_transport.rs:730` in `RingClientEndpoint::try_recv_with`,
and `client.rs:1978` in `decode_outbound`. The first is under the chain this
catalog established against three misleading signals: `RingTransport` built
unconditionally at `runtime.rs:876`, stored non-optionally at `:104`,
`ring.prepare` called by every authenticated connection at `connection.rs:148`.
A fourth call site, `ring_transport.rs:593`, is inside the test-only
`PublishHook` branch at `:592` and is not counted. Neither `#[cfg]` in the file
(`:541`, `:646`) is on this path.
Status: active
Exercised: partial - `wire.rs:722-742` covers three specific short and
bad-version inputs, and `wire.rs:745-774` covers four bad flag or type bytes.
Missing: any sweep over arbitrary bytes, any exhaustive length sweep from 0 to
21, and any structured mutation of an accepted seed. There is no fuzz target for
this decoder anywhere in the repository (`crates/shm-transport/fuzz` is the
only fuzz directory; its three targets are `frame_descriptor.rs`,
`provider_grant.rs` and `provider_sample.rs`, all transport decoders).
Guarantee: For every byte slice, `decode_header` returns either an
`EnvelopeHeader` satisfying all eleven gate postconditions or a typed
`DecodeError`; it never panics and never allocates.
Check: `always` - call `decode_header` on arbitrary bytes of arbitrary length;
assert the call returns, that on `Ok` every one of the eleven gate
conditions holds on the returned value, and, under a counting global allocator,
that the call performs zero heap allocations for every input. A panic is a forbidden state with no
dedicated detection point, so this is `always(!panic)`; `unreachable` is wrong
because no code location must never execute.
Fault/timing angle: none. The function is pure over one immutable slice. The
structural exposure is that every index past the first is a constant index
(`bytes[4]`, `bytes[5]`, `bytes[6]`, `bytes[7..9]`, `bytes[9..13]`,
`bytes[13..21]`) whose in-bounds-ness rests entirely on the single
`bytes.len() < need` gate at [wire.rs:312] and on `header_len_for_version`
returning 21 [wire.rs:294]. Narrowing that constant, or adding a version whose
`header_len_for_version` value is smaller than the largest constant index,
converts [wire.rs:355-357] into a panic.
Required faults and enabling state: none. Arbitrary bytes are the entire
enabling state. The property holds at `HEAD` and is under-evidenced, not
violated.
Confidence: high - [evidence](evidence/decode-header-is-total-over-arbitrary-bytes.md).
Every gate and every index was read directly, and re-read at carry time: the
eleven gates are `:307`, `:311`, `:312`, `:321`, `:323`, `:326`, `:329-331`,
`:332-339`, `:340`, `:345` and `:352`. `EnvelopeHeader` is constructed once,
after all eleven, at [wire.rs:359-367], and its fields are public but the value
cannot escape a rejected path. No allocation occurs: the function returns a
`Copy` struct.
Existing check: `wire.rs:722` `reject_truncated_headers_and_unsupported_versions`
and `wire.rs:745` `reject_unknown_frame_type_and_reserved_flag_encodings`, both
table-driven over single hand-picked inputs. Neither runs in CI, under this
sub-part's `R0`. Status unaudited. **One citation repaired at carry time:** the
second test's span is `:745-774`, not `:745-773`; the closing brace is at 774
and the lens range truncated it by one line.
Impact: today, none observable, and the reason was refreshed at carry time. All
three production callers pass an exactly-21-byte array, not a variable-length
slice: `ring_transport.rs:503` and `:730` pass `&lease.wire_header()`, typed
`[u8; WIRE_V2_HEADER_BYTES]` at `crates/shm-transport/src/lease.rs:163` with
that constant equal to 21 at `descriptor.rs:10`, and `client.rs:1978` passes
`header_bytes: &[u8; HEADER_LEN]` narrowed at `:1977`. **The lens said "both
production callers" and there are three; the count is repaired and the
conclusion is unchanged.** The value of the record is that the reasoning keeping
totality true lives nowhere in the tree, and the moment a caller passes a
variable-length slice - a coalescing reader, a batched shared-memory descriptor,
a future version with a shorter header - the constant indexes become the only
thing between a peer and a panic in the read loop.
Open questions:
- Should `header_len_for_version` be required to return at least the largest
  constant index used by the parse body, so a future version cannot silently
  make the parse out of bounds? (needs human input)

### accepted-header-decode-is-a-bijection-on-twenty-one-bytes

Type: safety
Reachability: default-production - the decode direction is the three production
`decode_header` call sites named in the record above. The encode direction is
`EnvelopeHeader::encode` (`wire.rs:205-216`), reached from both production
encoders: `encode_owned_frame`, whose `EnvelopeHeader { .. }.encode()` chain is `:584-593`,
and `encode_split_frame`, whose chain is `:622-631` and which also delegates
small bodies to `encode_owned_frame` at `:615`. Those
encoders are called from `dispatch.rs:292`, `:329`, `:723`, `:802`, `:1458`,
`connection.rs:779`, `:866`, and `client.rs:1329`, `:2092`, none `cfg`-gated.
Status: active
Exercised: partial - `wire.rs:703-719` pins all seven field offsets with
distinctive byte values, and `wire.rs:680-690` round-trips one header. Missing:
a per-bit influence oracle, and any assertion that `decode_header` reads nothing
past `HEADER_LEN`.
Guarantee: For every accepted header, `encode` and `decode_header` are mutually
inverse, every one of the 21 bytes influences exactly one decoded field, and no
byte at or beyond offset 21 is consumed.
Check: `always` - for every accepted 21-byte input, `decode_header(bytes)` then
`.encode()` reproduces `bytes` exactly; flipping any single bit inside the 21
bytes either changes the decoded value or causes rejection; and appending
arbitrary trailing bytes changes nothing about the result. `always` rather than
`reachable`: the condition is evaluated on every accepted decode, and the
forbidden state is an accepted header with an inert or aliased byte, which has
no dedicated detection point.
Fault/timing angle: none. The interesting axis is that `encode` writes its seven
fields by hand-written literal ranges [wire.rs:207-213] and `decode_header`
reads them back by independently hand-written literal ranges
[wire.rs:319], [:343], [:344], [:355-357]. Nothing ties the two sets of offsets
together, and a same-width transposition - `channel` against the low half of
`epoch`, or two bytes inside `corr` - is invisible to a round-trip test whose
fixture uses non-distinctive values.
Required faults and enabling state: none. Any accepted input suffices; what is
missing is the oracle.
Confidence: high - [evidence](evidence/accepted-header-decode-is-a-bijection-on-twenty-one-bytes.md).
`encode` covers `0..4`, `4`, `5`, `6`, `7..9`, `9..13`, `13..21` with no gaps and
no overlaps, and the decode side reads the identical seven ranges. Both sides
were re-printed at carry time and every citation in this record verified
unchanged; this is the one carried record that needed no repair. The
`little_endian_and_frozen_prefix_layout` test at `wire.rs:703` does use
distinctive ascending values, so it would catch a transposition today; nothing
forbids a future fixture from losing that property, and the test asserts on
`encode` only, never on the decode direction's offsets.
Existing check: `wire.rs:703` `little_endian_and_frozen_prefix_layout` (encode
direction, distinctive values, plus `buf.len() == HEADER_LEN` at `:718`);
`wire.rs:680` `round_trip_request`; `wire.rs:693` `round_trip_all_frame_types`.
None runs in CI, under this sub-part's `R0`. Status unaudited.
Impact: this bijection is what makes the frozen-prefix promise in the module
header [wire.rs:16-18] mean anything, and it is the only reason a peer's
independently written codec can interoperate. A drifted offset that still
satisfies the eleven gates produces a frame both sides accept and interpret
differently.
Open questions:
- Should `encode` and `decode_header` be generated from one offset table so a
  transposition is impossible by construction? (needs human input)

### reserved-encodings-and-identity-pairings-reject-at-decode

Type: safety
Reachability: default-production - same three production `decode_header` call
sites as the two records above. The reserved-encoding gates are unconditional
statements inside `decode_header`, at `:323`, `:326`, `:329-331`, `:332-339`,
`:345` and `:352`, with no `cfg`, no feature and no config branch between the
call site and any of them.
Status: active
Exercised: partial - `wire.rs:745-774` covers reserved flag bit 7, reserved
priority, reserved admission, and type byte 99; `wire.rs:836-862` covers
Sheddable on all ten illegal types and both legal ones; `wire.rs:795-833` covers
both halves of the channel-and-epoch pairing. Missing: an exhaustive sweep of
all 256 flag bytes and all 256 type bytes, and any check that a rejected
encoding is never masked, defaulted, or silently normalized.
Guarantee: A header carrying a reserved flag bit, a reserved priority or
admission value, an unassigned type byte, Sheddable on a delivery-required type,
or a mismatched channel-and-epoch pairing is rejected, never accepted with the
offending field cleared or defaulted.
Check: `always` - sweep all 256 values of the flags byte crossed with all 256
values of the type byte and both channel-and-epoch classes; assert every
combination the protocol calls invalid returns the specific `DecodeError`
variant for it, and that no accepted result has reserved bits set or a reserved
enum value. `always` because the obligation is per-frame and the forbidden
state - an accepted header whose reserved region was normalized rather than
refused - has no dedicated detection point.
Fault/timing angle: none. The exposure is that `Flags::priority` and
`Flags::admission_class` return `Option` [wire.rs:169-176] while
`Flags::is_binary` and `Flags::is_last` return `bool` [wire.rs:159-166]. A
future accessor written in the `bool` style over a widened bit field would mask
rather than reject, and the only thing forcing rejection today is that
`decode_header` propagates the `None` at [wire.rs:326] and [wire.rs:329-331].
Required faults and enabling state: a peer-authored header, which is the
baseline trust model. No concurrency, no timing.
Confidence: high - [evidence](evidence/reserved-encodings-and-identity-pairings-reject-at-decode.md).
Every gate read directly and re-read at carry time. The channel-and-epoch
pairing is a true biconditional: `channel == 0 && epoch != 0` at [wire.rs:345]
and `channel != 0 && epoch == 0` at [wire.rs:352], matching protocol section
6.1's "0 on channel 0; routed epochs are nonzero".
Existing check: `wire.rs:745` `reject_unknown_frame_type_and_reserved_flag_encodings`,
`wire.rs:836` `sheddable_rejected_on_every_illegal_frame_type`, `wire.rs:795`
`epoch_boundaries_round_trip_and_control_channel_epoch_is_reserved`, plus the
end-to-end `tests/protocol_vectors.rs:351`
`structural_corruption_is_rejected_before_dispatch` and `:504`
`pure_header_frames_accept_any_valid_priority`. None runs in CI. Status
unaudited. **Three citations repaired at carry time**, and this is the record the
earlier triage predicted would need a refresh because
`tests/protocol_vectors.rs` changed (976 lines at `1c193ae0`, 762 at `HEAD`,
under `63c4d277`). First, the in-file span is `:745-774`, not `:745-773`.
Second, `structural_corruption_closes_silently` at `:512` no longer exists: it
was **renamed** to `structural_corruption_is_rejected_before_dispatch` and moved
to `:351`. The rename is not a rewrite - the doc comment above it is unchanged
("Each structurally illegal frame retires the generation with no `Error` frame
and no resynchronization (protocol §6.3, AE2, V13-V15, V17, V42)") and so is the
`Case { name, bytes }` table that follows, so the check the record cited is the
check that still exists. Third, `pure_header_frames_accept_any_valid_priority`
kept its name and moved from `:656` to `:504`.
Impact: the reserved regions are the whole forward-compatibility budget. Any
implementation that masks instead of rejecting spends that budget silently: a
version-3 field placed in bits 6-7 would be ignored by a version-2 peer that
should have closed the generation.
Open questions: None.

### encoder-never-emits-a-frame-its-own-decoder-rejects

Type: safety
Reachability: default-production - the two production encoders are
`encode_owned_frame` (`wire.rs:571`) and `encode_split_frame` (`:608`), called
from `dispatch.rs:292`, `:329`, `:723`, `:802`, `:1458`, `connection.rs:779`,
`:866`, and `client.rs:1329`, `:2092`, none of them `cfg`-gated and all on the
terminal-emission path this catalog's siblings in 2e describe. The illegal
argument region is reachable from outside the crate: `pub mod wire` (`lib.rs:36`;
post-#131 it carries `#[doc(hidden)]` at `:35`, which hides it from rustdoc but
not from linkage)
exposes both encoders and `FrameId::routed`, and `pub mod handler` (`:14`)
exposes `RouteHandle` with both fields `pub` (`handler.rs:36-40`).
Status: active
Exercised: not yet - no test feeds encoder output back through
`decode_header` plus `validate_inbound_header` over anything but hand-chosen
legal inputs. The existing round-trips at `wire.rs:680` and `:693` construct
`EnvelopeHeader` directly, and `hdr` derives a legal epoch from the channel
(`wire.rs:650-652`, `u32::from(channel != 0)`), so they cannot reach the illegal
region.
Guarantee: For every argument tuple the production encoders accept, the emitted
bytes decode successfully and pass inbound validation on a conforming peer.
Check: `always` - for arbitrary `(ty, flags, id, body)`, and for each production encoder, `encode_owned_frame` (`wire.rs:543`, re-verified) and `encode_split_frame` (`:578`) with bodies below, at, and above the split threshold, either the encoder returns `Err`, or `decode_header` on its output returns `Ok` and the result satisfies the pure-header, Sheddable, channel-and-epoch, and reserved-bit rules. `always` because it must hold on every emission, and the forbidden state - a frame the local decoder would reject - has no detection point on the emitting side.
Fault/timing angle: none; this is a static contract gap. Four concrete holes,
all re-verified at carry time and all reachable from the crate's public surface
(O7): `Flags(0b1100_0000)` sets reserved bits, which [wire.rs:323] rejects;
`Flags(0b0000_0110)` sets reserved priority, which [wire.rs:326] rejects;
`encode_owned_frame(FrameType::Ping, .., body)` with a nonempty body emits
`len != 0` on a pure-header type, since `Ping` is in `is_pure_header`'s set
[wire.rs:86-88] and `encode_owned_frame` [wire.rs:571-602] tests only
`body.len() > MAX_BODY_LEN` at [:577], which [wire.rs:340] rejects; and
`FrameId::routed` [wire.rs:525-531] copies `RouteHandle`'s channel and epoch
without checking that a nonzero channel carries a nonzero epoch, which
[wire.rs:352] rejects.
Required faults and enabling state: none beyond a caller passing an
out-of-contract value. For the `FrameId::routed` hole specifically, a
`RouteHandle` with a nonzero channel and epoch 0. **The lens left whether the
route allocator can mint one open, and it is resolved here: it cannot.**
`RouteRegistry::reserve` (`routing.rs:113-156`) skips channel 0 with
`if candidate != 0` at `:123`, initializes a fresh slot with `last_epoch: 0` at
`:125`, and mints `epoch = slot.last_epoch + 1` at `:129-130`, so the least epoch
it can produce is 1 and the least channel is 1. That is pinned by
`reserved_channels_are_nonzero_distinct_and_start_at_epoch_one`
(`routing.rs:512`), whose asserts at `:522-526` require both channels nonzero and
both epochs equal to 1. So the enabling state is a **hand-constructed**
`RouteHandle`, which the public fields at `handler.rs:36-40` permit. Hand-building
a handle the allocator would never mint is already established practice in-tree,
though not with epoch 0: `routing.rs:715-718` builds a stale-epoch handle and
`:750-753` builds `epoch: handle.epoch + 1`, both to drive registry rejection
paths.
Confidence: high - [evidence](evidence/encoder-never-emits-a-frame-its-own-decoder-rejects.md).
The gap is high confidence and unchanged: all encoders were read end to end and
the only rejection in either production encoder is the body-length cap, at
[wire.rs:577] and [:618]. `Flags::new` [wire.rs:146-156] cannot produce the
illegal flag values, and the two host flag helpers `response_flags`
[wire.rs:636-638] and `pure_header_flags` [wire.rs:642-644] both go through it,
so the in-tree host emission paths are safe today by construction rather than by
enforcement. **Two things the lens recorded are corrected here.** First, the
lens counted three production encoders and cited a third cap at [wire.rs:548];
that cap is inside `encode_frame`, which carries `#[cfg(test)]` at
[wire.rs:541] and whose only two callers are
`frame_channel/contract_tests.rs:93` and `:163`. So there are two production
encoders and one test-only one, and the guarantee is stated over the two.
Second, the lens's `medium on reachability` rested on not having audited route
allocation; that audit is done above and the allocator is closed, which leaves
the hole reachable only through a hand-built handle. The finding survives both
corrections: nothing on either production encoder checks the pure-header,
Sheddable, reserved-bit or channel-and-epoch rules its own decoder enforces.
Existing check: none. `tests/protocol_vectors.rs:143`
`committed_header_vectors_decode_to_their_documented_fields` asserts the
document's byte vectors against the independent `raw_client::decode_header`
oracle (`tests/support/raw_client.rs:286`), which is the decode direction over
fixed inputs and not encoder refusal. Status: none found.
Impact: this is the encode side of the framing contract, and it is entirely
unenforced. A host that emits a frame its own decoder would reject produces
stream-alignment corruption at the peer, which the protocol requires the peer to
answer by retiring the connection without resynchronization and with no error
frame (`docs/host-wire-protocol.md:296`, which lists "unsupported version,
unknown type, invalid flags, nonzero channel-0 epoch, zero epoch on a routed
channel, pure-header body" - three of this record's four holes by name) - an
unattributable connection drop. **One citation repaired at carry time:** the lens
cited `:293`, which was correct at `1c193ae0`, where that line began "Clean EOF
before any byte of the next header is orderly connection close. EOF after the
first header byte, truncated header/body, unsupported version, unknown t...".
The document shrank from 1,031 lines to 936 and that sentence was rewritten;
both its clean-close and its retirement clauses now sit in `:296`. `:293` is
blank at `HEAD`.
Open questions:
- Should the encoders validate, or should the illegal region be made
  unconstructible by removing the public field from `Flags` and by giving
  pure-header types a body-free encoder? (needs human input)
- Should `encode_frame`'s `#[cfg(test)]` gate be reconsidered? It is the only
  encoder that takes `&[u8]` rather than an owned body, and its existence means
  the contract-test suite exercises an encoder the production path never uses.
  (needs human input)


## Part 2c catalog: the authenticated setup socket and peer identity

Scope: the trust boundary a peer crosses to obtain mappable ring memory.
`crates/host-runtime/src/setup_socket.rs` (826 lines) is the setup protocol and the
descriptor transfer. `crates/host-runtime/src/auth.rs` (1,112) is the three-message
mutual proof. `crates/host-runtime/src/instance.rs` (1,423) mints the credentials and
publishes them. `crates/host-runtime/src/connection_file.rs` (471) is the publication
format and its client-side reader. **3,832 in-crate lines.** One external file is
in scope as the peer half and is cited throughout because it is the only unit here
whose tests execute in CI: `packages/shm-native/src/setup.rs` (433), for
**4,265 total**.

Boundary context, read but not cataloged: `connection.rs` (Part 2a's file, but the
sole authorization gate lives in it at `:130-133` and the activation token is
minted in it at `:165`), `runtime.rs:834-850` and `:1017-1046` (2f's file, but the
listener and the handshake bound live there), `ring_transport.rs:636-656` and
`client.rs:346-369` (2b and 2d), and `packages/shm-native/src/lib.rs:491-629`.

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). Method contract in
[../METHOD.md](../METHOD.md). Code read from
`the `host` source checkout, branch
`feat/shared-memory-release-gate-audit`, `HEAD` = `e447c927`
("refactor(shm): trim final review leftovers"). Both lens agents read and verified
their line references at that commit, and this synthesis re-opened every reference
it restates in its own prose.

**This surface is post-refactor and almost entirely new.** Three of the four
refactor commits named in the re-scope reach it, and one of them created its
central file:

| Commit | Subject | Effect here |
| --- | --- | --- |
| `0f336d3c` | `refactor(shm): collapse to fixed ring transport` | fixed the ring profile and descriptor schema the setup protocol pins |
| `d8bde128` | `feat(host): add authenticated ring setup socket` | **added `setup_socket.rs`** |
| `793a973e` | `build(shm): require packaged native transport` | made the manifest and checksum gate the addon load path |
| `ed487e11` | `refactor(host): make ring transport mandatory` | made the ring the only transport, and **deleted the TypeScript handshake** (`auth.ts`, 314 lines; `auth.test.ts`, 365 lines) |

`setup_socket.rs` did not exist before `d8bde128`, so **nothing had ever been
cataloged against this boundary.** The re-scope records that directly:
`part-2-rescope/scope-map-and-risk-ranking.md:107` marks the file "new by
refactor, never scoped", and `:604` records "Salvage input: none. No lens file
covered either file." Every record below is a first pass, and no prior record is
being revised or inherited.

Three provenance refinements this synthesis made against the lens files, recorded
per METHOD.md rule 1 rather than silently applied. A fourth, on
`packages/shm-native/src/lib.rs` line numbers, is recorded below the list
because it touches record text this synthesis may not edit.

- `send_grant` is not literally `activate_server`'s first *statement*. The
  deadline computation at `setup_socket.rs:246-248` precedes it. `send_grant` is
  the first statement that touches the peer, and the substance of the finding is
  unchanged: it completes at `:260` before the first `read_message` at `:261`.
- The `fchmod(0o700)` on the runtime directory is at `instance.rs:571-572`. The
  `:560-573` range cited in lens A and in the task is the enclosing block, whose
  owner and directory-type checks are at `:561-570`. Both are correct; the
  narrower citation is the one that does the work.
- Lens B's production-guard inventory undercounts. It reports zero assertions,
  three `.expect(`, four constant-time comparisons, and five `let _ =` in the
  production halves. Re-derived per file by cutting each at its last
  `#[cfg(test)]`, the figures are **one** `debug_assert!`
  (`instance.rs:592-595`), **five** `.expect(`, **five** constant-time
  comparisons, and **nine** `let _ =`. The corrections and their sites are in
  [existing-checks.md](setup-identity/existing-checks.md); none of them changes a record.

**A fourth refinement, on the native addon's line numbers, now applied to the
record text.** Two records cited `packages/shm-native/src/lib.rs` a few lines
off. The earlier revision of this catalog recorded the corrections here and left
the record text alone, on the reasoning that the records were carried verbatim
from lens A. That was the wrong call: METHOD.md rule 1 requires the reference to
be corrected where it is written, and leaving a known-wrong number in a `Check:`
line sends a later reader to the wrong predicate. **The corrections are applied in
the records now, and this paragraph records what moved.** Re-derived at `HEAD` and
re-verified by grep for this disposition: in `attach` (`:491`) the
aliased-fd-or-grant rejection is `:533-535` (lens A: `:534-537`) and the
`GrantReservation::claim` is `:540-543` (lens A: `:539-549`); in `connect_setup`
(`:571`) the equal-grant rejection is `:588-590` (lens A: `:582-584`) and the
claim is `:591-594` (lens A: `:585-588`). The two registry insertion sites are the
`insert_channel` calls at `:551` and `:612`, not the `:550-556` and `:589-596`
ranges the records named; `:655` and `:672` are two further calls inside
`create_test_pair` (`:631`), which is a separate surface. Every finding the two
records state is unaffected: both predicates, both claims, and both insertion
sites exist where the records say they do in structure, and the counts are
unchanged.

## What this part is about

Seven facts frame every record below. They are stated with evidence because
several of them read as design decisions until the line is opened. One caution
about that framing, applied throughout after an independent evaluation refuted its
earlier form: **"undocumented" is a claim under test like any other, and the first
of the seven turned out to be documented.** Where a fact below is documented
design, the doc is cited and the record is a regression property rather than a
finding.

**Authority to map shared memory is possession of a 32-byte key and nothing
else, and this is documented design rather than a gap.** State the second half
first, because an earlier revision of this catalog did not and the framing was
refuted. `docs/host-wire-protocol.md:27` says it outright: "The 32-byte
connection key is a bearer capability. Possession grants every direct-profile
operation ... Client `role`, `consumer_identity`, `project_root`, `harness`, and
`session` are claims or scoping metadata; none grants authority." The code says the
same thing in the same words. `Authenticated`'s doc comment (`auth.rs:70-81`) is a
deliberately empty struct explaining that "WHAT THIS PROVES: the peer possesses the
connection key ... Nothing more", and that `ClientHello.role` "is parsed and then
discarded - any peer holding the key can claim any role, so it must never decide
admission, capacity, or privilege". So there is **no second factor to bypass**, and
nothing below should be read as reporting one. What follows is the mechanism, which
is worth pinning against regression precisely because it is intended. The only gate
on the accept path is the `if auth.is_err() { return; }` at
`connection.rs:130-133`, immediately after `authenticate_server`. Everything past
it is unconditional from the peer's point of view. `connection.rs:146-164` builds
the ring, and `activate_server`'s first peer-facing statement is the `send_grant`
call at `setup_socket.rs:249-260`, which writes the grant and both file
descriptors in one `sendmsg` with `SCM_RIGHTS` (`:151-159`) before the first
`read_message` at `:261`. So both descriptors leave the host before any
setup-phase byte is read. The activation token cannot gate mapping, because the
host mints it (`connection.rs:165`, `:212-226`), ships it *inside* the same
`GrantMessage` that carries the descriptors (`setup_socket.rs:254`), and only then
checks that the peer echoed it back (`:266-276`). A peer that never echoes has
already been paid. That peer is not hypothetical: `tests/shm_failure_modes.rs:44-58`
builds it against the real host, authenticating at `:50`, calling `receive_grant`
at `:53-56`, and then parking on `std::future::pending()` forever at `:58` without
ever sending `Activate`.

**A granted descriptor is never revoked.** Nothing in the protocol takes it back.
`activate_server` can fail after `send_grant` succeeded, on `InvalidActivation`
(`setup_socket.rs:275`), `InvalidIdentity` (`:278`), `InvalidMessage` (`:279`,
`:283`), or a `Timeout` in either `read_message`, and in every one of those cases
the host's response is to drop *its own* copies and cancel its own work
(`connection.rs:180-185`). The peer keeps both mapping descriptors. They are
ordinary file descriptors over ordinary memfds; the only stated containment is the
Linux size seal. The `docs/shm-transport.md:85` macOS caveat previously
cited here is gone: after PR #131 (merge `5d638e3e8`) that doc states a
`linux-x64-gnu`-only platform contract and no longer mentions macOS.

**Credentials do not survive an incarnation, and that is the good news here.**
The 32-byte key and the 16-byte daemon id are each a fresh `getrandom` inside
`InstanceGuard::acquire` (`instance.rs:263-264` and `:265-266`), drawn after the
lock race is won on the reasoning stated at `:222-231`, and the server nonce is a
fresh `random_nonce()` per handshake (`auth.rs:245`, drawing at `:379-383`). So a
snapshot from a previous incarnation fails at `InvalidServerProof` or
`DaemonIdMismatch` before the peer emits `ClientAuth`, and a captured `ClientAuth`
replayed against a live host fails at `auth.rs:275-277`. **Neither is
negative-tested.** `auth.rs:924-939` asserts that two handshakes receive distinct
server nonces, which proves the precondition of replay resistance and never
attempts a replay; no test authenticates against incarnation N+1 with
incarnation N's snapshot.

**The socket is mode `0600` applied after `bind`.** `bind_owner_only` binds at
`setup_socket.rs:44` and narrows the mode at `:45`, rolling back with an unlink if
the chmod fails (`:46-47`). Between those two lines the socket exists at
`0777 & ~umask`, so a permissive umask opens a real window. It is unexploitable
only because the containing runtime directory is unconditionally `fchmod`ed to
`0700` (`instance.rs:571-572`, inside the validated `O_NOFOLLOW`-anchored walk the
module doc describes at `:4-7`). The socket's own mode is therefore not the gate a
reader would believe it is. The path is fully predictable, with no random
component: `${dataDir}/eidnara/run/setup.sock`, from `instance.rs:167`,
`:177-179` and `runtime.rs:834`. And the pre-existing-occupant gate at
`setup_socket.rs:30-32` is a three-clause conjunction, requiring a socket, owned
by the effective uid, at exactly mode `0600`, of which only the `is_socket()`
clause is tested: the sole test plants a regular file (`:494-501`, planted at
`:497`). A same-uid socket at mode `0666`, which is exactly the residue a previous
incarnation under a permissive umask leaves, is untested.

**The native-versus-managed asymmetry Part 1 found still exists and has
inverted.** The refactor added `packages/shm-native/src/setup.rs`, and the
native peer now validates wire version and schema (`:115-118`), decodes both
grants (`:120-121`), and rejects a wrong profile or an aliased grant pair in one
expression (`:122-124`), while `lib.rs:533-535` rejects an aliased fd or grant and
`:540-543` and `:591-594` take a process-wide `GrantReservation::claim`. Those four
native line ranges are the corrected ones. The
managed Rust peer has none of that at the same layer: `activate_client`
(`setup_socket.rs:302-306`) checks wire version and schema only and returns the
descriptor as an unvalidated `serde_json::Value`, and `ring_transport.rs:642-650`
checks the profile and then rejects grants whose **geometries differ** (`:648`)
where the native side rejects grants that are **equal** (`setup.rs:122`,
`lib.rs:588-590`). Two identical grants have identical geometry, so they pass the
managed check and fail the native one. **On grant-distinctness the polarity is
reversed, not merely absent**, and the managed path takes no claim at all. Part 1's
`native-boundary-not-weaker-than-its-wrapper` recorded the mirror-image gap and
should be re-read rather than assumed still-oriented; the weaker boundary is now
the managed Rust client.

**Two auth doc comments cite a cross-language contract whose other half the
refactor deleted.** `auth.rs:693-698` states that the TypeScript client asserts
its handshake against the same fixed proof vectors in
`packages/plugin/src/shared/host-client/auth.test.ts`, "so they form a
cross-language contract: changing the domain separator, the field order, or the
MAC breaks the build here, where the change is being made." That file does not
exist. Neither does `auth.ts`. `git show --stat ed487e11` shows both deleted at
365 and 314 lines by the commit that made the ring mandatory, and the surviving
directory listing confirms their absence. Separately, `auth.rs:394-396` names
`foreign_server_reused_port_never_receives_client_auth` as the always-true fence
on the proof comparison; a repository-wide grep for that identifier returns
exactly one hit, the comment itself. **The consequence is that the CI-enforced
authority for the proof construction is now the peer's implementation rather than
the host's.** The host's `committed_wire_vectors_pin_the_proof_construction` and
the peer's `auth_proofs_match_committed_wire_vectors`
(`packages/shm-native/src/setup.rs:401-432`) assert the same literal vectors
from `docs/host-wire-protocol.md:180` and `:182`; only the peer's runs. Per
METHOD.md rule 3 both disagreements are recorded with each side cited and neither
is resolved in the comment's favour.

### Coverage: the two halves of one protocol had opposite CI status at the source

There are **51 in-crate tests** across the five scope files: 22 in `instance.rs`,
12 in `setup_socket.rs`, 11 in `auth.rs`, 4 in `connection_file.rs`, and 2 in
`packages/shm-native/src/setup.rs`. Counts re-derived at `HEAD` by matching
`#[test]` and `#[tokio::test]` per file. **49 of them never run.**

The exclusion is structural, and so is the inclusion. Every `-p host-runtime` test
invocation in `ci.yml` carries a `--test <name>` filter, which selects one
integration binary and does not build the lib target, so the 386-line test module
in `setup_socket.rs:441-826` and the other three `host-runtime` modules are never
compiled in CI. The peer half is in a different crate, and `ci.yml:167` runs
`cargo nextest run -p shm-native -p shm-transport` **unfiltered** on Linux;
the unfiltered macOS `shm-native` run this preamble previously cited was
removed with every other macOS job by PR #131 (merge `5d638e3e8`).
So the 2 tests in the peer's `setup.rs` do run while the 11 in `auth.rs` that pin
the same proof construction on the host side do not.

**There is no other source-resident check.** `ci.yml:175` runs
`cargo test -p host-runtime --doc`, but this sub-part has zero doctests: a grep for
`/// ``` ` and `//! ``` ` fences across `setup_socket.rs`, `auth.rs`,
`instance.rs` and `connection_file.rs` returns zero in each file, verified at
`HEAD`. The one `debug_assert!` in scope (`instance.rs:563`, re-verified) is compiled out
of release builds; CI's debug-profile test runs compile it, so it fires under test
and never in a release binary.

Six of the crate's 24 integration binaries reach this boundary. In this tree all
six run in CI: `ci.yml:118` and `:126` run `cargo test --workspace --all-targets`,
which builds and runs every integration binary. They are `lifecycle.rs` (35
tests), `client.rs` (6), `shm_failure_modes.rs` (6), `instance_security.rs` (15),
`host_roundtrip.rs` (4) and `activation.rs` (4). The last three are the sole homes
of descriptor-anchored discovery, symlink and replacement safety, fenced shutdown
removal, credential rotation, and the normative startup order. Nothing else covers
any of them. (The source catalog recorded three of the six as named in no
workflow; that was the source repository's CI, not this one.)

## Index

16 records, listed in the order lens A discovered them, with the two records the
portfolio disposition added placed beside their siblings. The group sections below
re-present the same 16 by shared mechanism, so section order and index order
differ deliberately; every record appears exactly once in each.

| Slug | Type | Confidence |
| --- | --- | --- |
| [setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof](#setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof) | safety | high |
| [setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token](#setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token) | safety | high |
| [setup-a-a-captured-client-proof-never-authenticates-twice](#setup-a-a-captured-client-proof-never-authenticates-twice) | safety | high |
| [setup-a-credentials-do-not-survive-a-host-incarnation](#setup-a-credentials-do-not-survive-a-host-incarnation) | safety | high |
| [setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it](#setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it) | safety | high |
| [setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid](#setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid) | safety | high |
| [setup-a-a-hostile-occupant-of-the-socket-path-fails-closed](#setup-a-a-hostile-occupant-of-the-socket-path-fails-closed) | safety | high |
| [setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof](#setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof) | safety | high |
| [setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released](#setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released) | safety | high |
| [setup-a-an-abandoned-setup-strands-no-ring-charge](#setup-a-an-abandoned-setup-strands-no-ring-charge) | safety | high |
| [setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline](#setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline) | liveness | high |
| [setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap](#setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap) | safety | high |
| [setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input](#setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input) | liveness | high |
| [setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection](#setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection) | safety | high |
| [setup-a-only-an-authenticated-grant-enters-the-native-channel-registry](#setup-a-only-an-authenticated-grant-enters-the-native-channel-registry) | safety | high |
| [setup-a-concurrent-setup-saturation-is-reached](#setup-a-concurrent-setup-saturation-is-reached) | reachability | high |

Distribution after the portfolio disposition in
[portfolio-evaluation.md](setup-identity/portfolio-evaluation.md): **13 `safety`, 2 `liveness`, 1
`reachability`**; **15 `always` and 1 `sometimes`**; 16 high confidence and 0
medium. Reachability classes are 15 `default-production` plus one record whose
subject is a published export **compiled with no shipped-plugin caller**
(`setup-a-only-an-authenticated-grant-enters-the-native-channel-registry`); each
label carries its own evidence on the record, per METHOD.md rule 4.

Before the disposition this read 13 `safety` and 1 `reachability`, 13 `always` and
1 `sometimes`, all 14 `default-production`, with 1 medium confidence, **and no
`liveness` record at all**. That absence was defended in the relationship map on a
misreading of METHOD.md's liveness rule and has been corrected: the rule admits a
deadline as a bound, so the rejected candidate is now
[setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline](#setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline),
and the cancellation clause that had been smuggled into the sentinel safety record
is now [its own liveness record](#setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input)
with an explicit bound. There is still no `unreachable` record, and that remains
correct rather than a gap: no record here is about a forbidden code location.

**Group names below are this synthesis's, not the lens's.** Lens A produced 14
numbered records with no grouping. The five groups are chosen by the mechanism
that would break, because that is what decides which oracle subsumes which.

---

## Group S1: the one authorization gate and what possession of the key buys

Three records on the single `if auth.is_err()` at `connection.rs:130-133` and on
what the code does immediately after it. The first is the ordering invariant that
no descriptor precedes a verified proof. The second is the documented consequence
of a bearer-capability model, that once the proof lands the descriptors are
unconditional, so the activation token gates the host's acknowledgement and not the
peer's capability. The third is what the token does still buy: it distinguishes the
peer that received a grant from any other authenticated peer, one connection at a
time.

They are grouped because they are three readings of one straight-line sequence,
`connection.rs:120-170` into `setup_socket.rs:249-284`, and because the second and
third are the two halves of the doc's "one-use activation token" claim
(`docs/host-wire-protocol.md:561`): the mechanism is real, and it is structural
rather than a consumed nonce.

### setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof

Type: safety
Reachability: default-production - `run_connection` is the only accept-path body
(`runtime.rs:1042-1044`), the ring is mandatory after the refactor, and
`config.rs:223` gives `auth_deadline` a shipped default.
Status: active
Exercised: partial - three integration tests prove an unauthenticated socket
receives no bytes; none instruments the descriptor-send site itself.
Guarantee: A `SCM_RIGHTS` message carrying ring descriptors is never written to a
setup socket on which `authenticate_server` has not returned `Ok`.
Check: `always` - instrument `send_grant` (`setup_socket.rs:153-159`); every
invocation is preceded on that same stream by an `Ok(Authenticated)` from
`auth.rs:251`. `always` because it is a per-send ordering invariant with no
optional path and no eventual convergence; a single violation is a full loss of
the boundary.
Fault/timing angle: none in the ordering itself, it is straight-line. The window
worth attacking is `connection.rs:130-133`: it discriminates on `is_err()`, so
any future refactor that makes `authenticate_server` return `Ok` on a partial
handshake silently opens the gate.
Required faults and enabling state: a peer that connects and then presents a
malformed `ClientHello`, a short nonce, a wrong `ClientAuth`, or nothing at all,
while the send site is instrumented.
Confidence: high - [evidence](evidence/setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof.md).
Verified: `authenticate_server` is called at `connection.rs:120-129` and its
error return exits at `:130-133`; `activate_server` is reached only at `:170`;
`send_grant` is `activate_server`'s first statement (`setup_socket.rs:249`).
Existing check: `crates/host-runtime/tests/lifecycle.rs:1643-1673`
`shutdown_requires_authentication_and_a_valid_shape` and
`crates/host-runtime/tests/protocol_vectors.rs:294`
`malformed_and_wrong_proof_handshakes_close_without_envelope_traffic` both assert
no byte reaches an unauthenticated socket, which subsumes descriptors. Status
unaudited.
Impact: a peer with no credential obtains read-write mappings of host memory.
Part 1 established the whole object is mapped `PROT_READ|PROT_WRITE` with no
`F_SEAL_WRITE` (`quarantine-authority-survives-peer-writes`), so this is
arbitrary write access to host transport state, not merely disclosure.
Open questions: None.


### setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token

Type: safety
Reachability: default-production - same path as the record above.
Status: active
Exercised: partial - `shm_failure_modes.rs:44-58` constructs the peer that takes
the descriptors and never activates, but it asserts capacity return, not the
authority question.
Guarantee: The activation token is not a mapping gate, and is not intended to be
one. A peer that has proved key possession can map the ring whether or not it ever
presents a correct token, and no host-side check between message 3 and message 4
can refuse it.
Check: `always` - for a peer that authenticates and then sends nothing, a wrong
token, or a truncated `Activate`, the two descriptors it already holds still map
successfully. Stated as `always` because it is a standing property of the message
order at `setup_socket.rs:247-274` rather than an occasional outcome.
**This is a regression property over documented design, not a report of a
second-factor bypass**, and an earlier revision of this record read as the latter.
The bearer-capability model is stated in `docs/host-wire-protocol.md:26` and
restated in `auth.rs:61-64`, which says the handshake proves key possession and
"Nothing more". So the check is not "the token fails to gate mapping" - nothing
claims it does - but "the relationship between key possession and mapping
authority is still exactly one-to-one", which a future refactor could silently
break in either direction: by making a token check appear to gate mapping when it
runs after the descriptors are gone, or by admitting a peer that never proved key
possession at all.
Fault/timing angle: the window is from the host's `sendmsg` at
`setup_socket.rs:151-159` to its token compare at `:267-272`, which is at least
one peer round trip wide and is bounded only by `transport_setup_deadline`,
2 seconds by default (`config.rs:227`).
Required faults and enabling state: a peer that completes authentication, calls
`receive_grant`, and then diverges from the protocol. `shm_failure_modes.rs:44-58`
already builds it; the missing part is mapping the received fds and writing.
Confidence: high - [evidence](evidence/setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token.md).
Verified: `activate_server` sends before it reads (`setup_socket.rs:249-261`);
the token is minted by the host (`connection.rs:165`, `:216-226`) and travels
inside the same message as the descriptors (`setup_socket.rs:254`).
Existing check: none for the authority claim. `setup_socket.rs:768-808`
`client_rejects_stale_identity_without_activate_write_or_returned_descriptors`
proves the well-behaved client does not *return* descriptors it rejected; it does
not and cannot prove a hostile peer lacks them.
Impact: the security argument for the setup socket rests on the connection file's
`0600` mode and the runtime directory's `0700` mode, which is what a bearer-key
model implies and what `docs/host-wire-protocol.md:27` says: a key reader "MUST
therefore be trusted as the same local security principal as the host". The
consequence worth guarding is forward-looking rather than current. Any future
design that treats the token as a second factor - to fence a compromised key, for
example - would be relying on a check that runs after the asset is gone, and the
message order makes that mistake easy to make and hard to see. That is what this
record protects against, and it is the whole of its claim.
Open questions:
- Was descriptor-before-validation chosen so the host need not hold the ring
  while waiting on a peer round trip, or is it incidental? Reordering to
  `Activate`-then-grant would make the token a real gate, at the cost of one
  extra round trip inside the setup deadline. (needs human input)


### setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it

Type: safety
Reachability: default-production.
Status: active
Exercised: partial - the matching and mismatching wire-identity cases are tested
in-crate; the cross-connection token case is not.
Guarantee: An activation token accepted on one connection is refused on every
other connection, and no connection accepts a second `Activate`.
Check: `always` - run two setups concurrently, feed connection A's token to
connection B, and assert `SetupError::InvalidActivation` from
`setup_socket.rs:273`. Separately, send `Activate` twice on one connection and
assert the second is `SetupError::InvalidMessage` from `:281`. `always` because
both are per-connection invariants.
Fault/timing angle: two setups overlapping inside the same
`transport_setup_deadline`. `max_handshakes` defaults to 32 and
`max_connections` to 64 (`config.rs:128-129`), so overlap is the normal case
rather than a rare one.
Required faults and enabling state: two peers that both authenticate and then
swap the tokens they received.
Confidence: high - [evidence](evidence/setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it.md).
Verified: the token is drawn per `run_connection` at `connection.rs:165` from a
32-byte `getrandom` (`:216-226`), compared with `subtle::ConstantTimeEq` at
`setup_socket.rs:267-272`, and `activate_server` reads exactly one message in the
`Activate` position (`:261-280`) then only accepts `Commit` (`:281-284`).
Existing check: `setup_socket.rs:725-765`
`stale_wire_or_descriptor_schema_is_invalid_identity` covers the wire-version and
schema half of the same match arm, giving `InvalidIdentity` but never
`InvalidActivation`. So the token comparison itself has **no** negative test.
Status unaudited.
Impact: the token is what makes "one grant, one activation" observable. If the
comparison were always-true, activation would stop distinguishing the peer that
received a grant from any other authenticated peer, and the doc's "one-use
activation token" (`host-wire-protocol.md:561`) would be vacuous.
Open questions:
- The token is compared but never *consumed* into any store. "One-use" holds only
  because each connection mints its own. Is that the intended reading of
  `host-wire-protocol.md:561`? (needs human input)


## Group S2: credential freshness and the two directions of proof refusal

Three records on the proof itself, and they partition the three ways it can be
attacked. A captured `ClientAuth` replayed against the live host that produced it.
A whole connection-file snapshot replayed against a *later* incarnation of the
host. And an impostor listener at the published path trying to extract a
`ClientAuth` from an honest peer.

The mechanism under all three is that only the host's own randomness carries any
freshness burden. The `server_nonce` is drawn per handshake (`auth.rs:245`), the
key and daemon id are drawn per incarnation (`instance.rs:263-266`), and the
peer-supplied `client_nonce` is never inspected for freshness, uniqueness, or
non-repetition. `role` is inspected even less: it is parsed and discarded
(`auth.rs:70-83`). So the first two records are refusals the host performs and the
third is a refusal the *peer* performs, which is why the third is the only one of
the three whose existing tests are direct.

### setup-a-a-captured-client-proof-never-authenticates-twice

Type: safety
Reachability: default-production.
Status: active
Exercised: partial - nonce freshness is asserted; no test replays a captured
`ClientAuth`.
Guarantee: Bytes captured from one successful handshake, replayed verbatim as
`ClientHello` then `ClientAuth` on a fresh connection to the same live host, are
refused with `InvalidClientAuth`.
Check: `always` - record a full transcript, open a new connection, send the
recorded `ClientHello` and then the recorded `ClientAuth` without recomputing,
and assert `AuthError::InvalidClientAuth` from `auth.rs:247-249` specifically,
not merely a closed socket. `always` because every handshake must resist it.
Fault/timing angle: none temporal. The defence is structural: the host draws a
fresh `server_nonce` at `auth.rs:245` and folds it into the expected proof at
`:268-274`, so a replay matches only if the same nonce recurs. The peer's
`client_nonce` is fully attacker-controlled and never inspected, which is why the
server nonce carries the whole burden.
Required faults and enabling state: a passive observer of one handshake. On a
Unix socket that means a same-uid process able to trace the peer, so this is a
defence-in-depth property under the stated trust model.
Confidence: high - [evidence](evidence/setup-a-a-captured-client-proof-never-authenticates-twice.md).
Verified: `random_nonce` at `auth.rs:379-383` is a direct `getrandom` per call,
called once per `authenticate_server_inner` at `:245`; nothing caches or reuses
it.
Existing check: `auth.rs:924-939` `repeated_handshakes_receive_fresh_server_nonces`
asserts distinctness across two handshakes. It never attempts a replay, so it
proves the precondition and not the property. Status unaudited.
Impact: if the nonce ever became derived, fixed, or counter-based, one observed
transcript would become a permanent credential for that incarnation, and it would
still satisfy the existing test if the counter merely incremented.
Open questions:
- `client_nonce` is unchecked. Should the host reject an all-zero or repeated
  client nonce, or is server-nonce freshness genuinely sufficient? The doc claims
  sufficiency at `host-wire-protocol.md:177`. (needs human input)


### setup-a-credentials-do-not-survive-a-host-incarnation

Type: safety
Reachability: default-production - `InstanceGuard::acquire` runs on every host
start (`runtime.rs` startup path), and the fields it mints are the only ones the
handshake consults (`runtime.rs:913` region, `connection.rs:120-129`).
Status: active
Exercised: not yet - no test authenticates against incarnation N+1 using
incarnation N's snapshot.
Guarantee: A connection-file snapshot from a previous host incarnation
authenticates against no later incarnation, and the peer refuses before it emits
`ClientAuth`.
Check: `always` - capture a snapshot, restart the host, dial the new socket with
the old snapshot, and assert the peer fails at `InvalidServerProof`
(`auth.rs:306-308`) or `DaemonIdMismatch` (`:309-311`) and that no `ClientAuth`
frame was written. `always` because it must hold for every pair of incarnations.
Fault/timing angle: the interesting window is a host restart while a client holds
a cached `ConnectionInfo`. The client is not required to re-read the file, so
this is the realistic path into the property rather than an attack.
Required faults and enabling state: two host incarnations in the same data
directory, plus a peer that reuses the earlier snapshot.
Confidence: high - [evidence](evidence/setup-a-credentials-do-not-survive-a-host-incarnation.md).
Verified: key and daemon id are each a fresh `getrandom` inside `acquire`
(`instance.rs:263-266`), the ordering comment at `:222-231` states credentials
are minted after the lock is won, and `ConnectionInfo` carries both by value
(`connection_file.rs:37-38`) so nothing persistent backs them.
Existing check: none direct. `auth.rs:385-403` claims two bootstrap tests
"named for key rotation and singleton probing" carry the always-false coverage;
those are in other files and were not located in this pass, so the claim is
unverified here.
Impact: without per-incarnation rotation, an old snapshot would be a permanent
bearer credential, and `daemon_ver` fencing (`auth.rs:346-348`) would be the only
thing distinguishing incarnations.
Open questions:
- Where are the two bootstrap tests named at `auth.rs:390-392`? Not found in
  `crates/host-runtime/tests/` in this pass. Locating them changes this record's
  `Existing check` line. (unresolved, needs a repository-wide test search)


### setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof

Type: safety
Reachability: default-production - both peer implementations connect without
inspecting the socket (`client.rs:347`, native `setup.rs:106`).
Status: active
Exercised: partial - the in-crate unit suite covers the three refusal reasons
individually.
Guarantee: A listener that occupies the published socket path without holding the
connection key learns nothing from a peer and receives no `ClientAuth`.
Check: `always` - stand up a listener that answers `ClientHello` with a
syntactically valid `ServerProof` carrying a wrong proof, a wrong `daemon_id`, or
a wrong `daemon_ver`, and assert the peer writes exactly one message, the
`ClientHello`, and then closes. `always` because it must hold on every dial.
Fault/timing angle: none. The peer performs all three checks
(`auth.rs:326-348`; native `setup.rs:200-205`) before the `write_message` at
`auth.rs:357-363`, so the ordering is straight-line and the property is about
that ordering not regressing.
Required faults and enabling state: an impostor listener. Constructible in-process
with `UnixStream::pair`, which is what the existing tests do.
Confidence: high - [evidence](evidence/setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof.md).
Verified: all three peer checks precede the `ClientAuth` write in both
implementations, and the native side short-circuits them into one `if` with
`ct_eq` on the proof and the daemon id (`setup.rs:200-205`).
Existing check: `auth.rs:1022-1073` `rejected_server_sends_no_client_auth`,
driven by `:1074-1081` `invalid_server_proof_sends_no_client_auth` and
`:1082-1089` `daemon_id_mismatch_sends_no_client_auth`. The always-true guard
on the comparison is unnamed in `auth.rs`; the source catalog cited a
`foreign_server_reused_port_never_receives_client_auth` identifier that no file
under `crates/` declares. The `daemon_ver` mismatch case is not visibly covered
by the two named tests. Status unaudited.
Impact: this is the only thing standing between a same-uid squatter and a peer's
`ClientAuth`, because neither peer checks the socket's ownership or mode before
connecting. A leaked `ClientAuth` is not directly a credential, since it is
nonce-bound, but it is an oracle on the key.
Open questions:
- Should the peer stat the socket for owner and mode before connecting, as the
  connection-file reader already does for the file (`connection_file.rs:267-287`)?
  It would be defence in depth over a check the mutual proof already carries.
  (needs human input)


## Group S3: the socket as a filesystem object

Two records on `bind_owner_only` (`setup_socket.rs:27-50`), the twenty-four lines
that decide whether the setup socket is reachable at all. They are the same
mechanism seen from the two sides of the `bind` call: what the socket looks like
after it exists, and what the function does about something that was already
there.

Both records have the same shape and the same weakness. Each rests on a
conjunction or an ordering that is correct today for a reason stated in a
*different* file, `instance.rs:571-572`'s unconditional `fchmod(0o700)` on the
parent directory, and neither file states that dependency. And in both records the
untested residue is the clause a reader would most expect to be load-bearing: the
pre-chmod window in one, the mode and owner clauses of the occupant gate in the
other.

### setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid

Type: safety
Reachability: default-production - `bind_owner_only` is called unconditionally at
`runtime.rs:836` on the publication path.
Status: active
Exercised: partial - the final mode is asserted; the interval before the chmod is
not.
Guarantee: From the instant the setup socket appears in the filesystem until it
is unlinked, no principal outside the effective uid can connect to it.
Check: `always` - under a permissive umask such as `0o000`, sample the socket's
mode from a concurrent observer between `bind` and `set_permissions`, and assert
either that the mode is already `0600` or that the containing directory denies
traversal to every other uid. `always` because the exposure is instantaneous and
a single sample inside the window is a violation.
Fault/timing angle: the window is exactly `setup_socket.rs:44` to `:45`. `bind`
creates the socket with `0777 & ~umask`; the tightening is a separate syscall
afterwards. The mitigation is not in this file: `instance.rs:560-573`
unconditionally `fchmod`s the containing runtime directory to `0700`, so the
window is closed by the parent rather than by the socket's own mode.
Required faults and enabling state: a permissive umask in the host's process, and
an observer sampling the mode. Demonstrating actual cross-uid connectability
additionally needs a second uid, which may be unconstructible in CI and should be
recorded as such rather than skipped silently.
Confidence: high - [evidence](evidence/setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid.md).
Verified: the bind-then-chmod order at `setup_socket.rs:44-48`, the failure
rollback that unlinks on a failed chmod at `:45-47`, and the parent's
unconditional `fchmod(0o700)` at `instance.rs:560-573`.
Existing check: `setup_socket.rs:480-491` `setup_socket_is_owner_only` asserts the
mode after `bind_owner_only` returns, so it covers the end state and not the
window. `instance.rs:979` `permissive_umask_still_yields_owner_only_dir_and_file`
covers the directory and the connection file under a permissive umask but not the
socket. Status unaudited.
Impact: low today, because the parent directory is the real gate. The property is
worth holding because the socket's own mode is the layer a reader would believe,
and a future change that moves the socket out of the `0700` directory would
inherit an unprotected window.
Open questions:
- Would binding through a temporary name and `renameat` into place, or setting
  the umask around the bind, be preferred to relying on the parent directory?
  (needs human input)


### setup-a-a-hostile-occupant-of-the-socket-path-fails-closed

Type: safety
Reachability: default-production.
Status: active
Exercised: partial - one of four failing occupant shapes is tested.
Guarantee: `bind_owner_only` refuses every pre-existing occupant that is not a
socket owned by the effective uid at exactly mode `0600`, refuses without
following links, and never removes an occupant it refused.
Check: `always` - for each of a dangling symlink, a symlink to a live socket, a
socket at mode `0666`, a socket owned by another uid, a directory, and a FIFO,
assert `io::ErrorKind::PermissionDenied` and assert the occupant is still present
afterwards. `always` because it is a per-call invariant over adversary-chosen
filesystem state. This is the same shape as Part 1's
`runtime-directory-authentication-is-a-precondition-not-a-container`, whose
finding was that the conjunction is never negative-tested.
Fault/timing angle: two windows. `symlink_metadata` at `setup_socket.rs:28` to
`remove_file` at `:39`, and `remove_file` at `:39` to `bind` at `:44`. The second
lets a same-uid attacker take the name and force `EADDRINUSE`; the outcome is a
failed start with no connection file published, so it is a denial primitive and
not an impersonation.
Required faults and enabling state: filesystem state planted at the socket path
before the host starts. Four of the six shapes are constructible unprivileged in
a temporary directory. The wrong-owner case needs a second uid.
Confidence: high - [evidence](evidence/setup-a-a-hostile-occupant-of-the-socket-path-fails-closed.md).
Verified: `symlink_metadata` and not `metadata`, so a symlink is classified as a
symlink and fails the `is_socket()` clause (`setup_socket.rs:28-32`); the three
clauses are one conjunction at `:30-32`; the refusal at `:33-38` precedes the
unlink at `:39`.
Existing check: `setup_socket.rs:493-501`
`insecure_stale_occupant_is_not_replaced` covers the regular-file case and does
assert the occupant survives. Symlink, wrong mode, wrong owner, directory and
FIFO are untested. Status unaudited.
Impact: a mode or owner clause that silently stopped being evaluated would let
the host adopt and then unlink an attacker-planted object, or bind over a live
socket. The conjunction is exactly the shape that passes for the wrong reason
when one clause is dropped.
Open questions:
- The stale-socket branch removes and rebinds. Is there a case where the occupant
  is a *live* socket of a still-running incarnation that lost its lock, and
  should the instance lock be consulted before the unlink? (needs human input)


## Group S4: bounded unauthenticated work, abandoned setups, and the sentinel

Six records on resource accounting across the boundary, plus the coverage marker
that keeps three of them from passing vacuously. This group grew by two under the
portfolio disposition: it gained the deadline record the earlier revision rejected,
and it gained the sentinel's cancellation clause as a record of its own.

The five substantive records follow one connection's charge from accept to death.
`runtime.rs:1035-1040` takes an unauthenticated handshake permit before spawning
anything, bounded at `max_handshakes` = 32 (`config.rs:128`). The swap at
`connection.rs:137-141` acquires the *connection* permit before releasing the
handshake permit, bounded at `max_connections` = 64 (`config.rs:129`), which is why
the post-auth descriptor transfer and its 2-second `transport_setup_deadline`
(`config.rs:227`) are charged to 64 rather than to 32. Then the prepared ring's own
charge must come back on every abandoned exit, and the same deadline bounds how
long a stalled peer can hold that ring at all - those two are the group's pair on
the same window, one about whether the charge returns and one about when. Finally
the post-commit sentinel must stay cheap for the whole life of an idle connection,
which splits into a length cap that is a safety invariant and an exit-on-cancellation
obligation that is a liveness one; the earlier revision carried both in one record
and the liveness half therefore carried no bound.

The last record is the reason the others are here as a group rather than
scattered. It is the part's only `reachability` record and its only `sometimes`
check, and it exists because the two existing saturation tests pin
`max_handshakes` to 1 (`tests/lifecycle.rs:239`) and 4 (`:339`) and use squatters
that never speak, so no campaign has yet produced two overlapping setups. Without
that state, the bounding record, the token-scoping record in Group S1, and the
charge-release record here can all pass on a run that never ran two setups at
once.

### setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released

Type: safety
Reachability: default-production - the bound is `config.rs:128`, default 32, with
no opt-in.
Status: active
Exercised: partial - two lifecycle tests cover saturation and non-starvation.
Guarantee: The number of connections that have been accepted but not yet
authenticated never exceeds `max_handshakes`, excess accepts are closed without
reading a client byte, and every terminal outcome releases the slot.
Check: `always` - with `max_handshakes = 1`, hold the slot with a socket that
never speaks, assert a second accept closes with no bytes read, then release the
squatter and assert the slot becomes available. Enumerate every exit from
`run_connection` before `drop(handshake_permit)` and assert each releases:
auth error (`connection.rs:101-103`) and connection-permit exhaustion
(`:106-108`), and the two exits that take neither return: the tracked
`run_connection` future aborted by forced shutdown while awaiting
authentication, and the future dropped by an unwinding panic; in each case assert
the handshake slot is available again, since a permit transferred or leaked on
those paths is invisible to the two explicit returns. `always` because the bound must hold at every instant.
Fault/timing angle: the permit swap at `connection.rs:137-141` acquires the
connection permit *before* releasing the handshake permit, so a peer is briefly
charged to both classes. The consequence is that the post-auth descriptor
transfer, bounded by `transport_setup_deadline` at 2 seconds
(`config.rs:227`), is charged to `max_connections` and not to `max_handshakes`.
Required faults and enabling state: a squatter that authenticates and stalls, and
a squatter that never speaks; both already exist in the test support
(`tests/support/raw_client.rs:878`).
Confidence: high - [evidence](evidence/setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released.md).
Verified: `try_acquire_owned` before spawn at `runtime.rs:887-894`, the
`drop(stream)` on failure at `:889`, and the two pre-swap exits in
`connection.rs`.
Existing check: `crates/host-runtime/tests/lifecycle.rs:237`
`saturated_handshake_capacity_closes_without_reading_client_bytes` and `:337`
`an_unauthenticated_flood_cannot_starve_established_work`;
`crates/host-runtime/tests/handler_contract.rs:256` asserts the default is positive.
Status unaudited.
Impact: without the bound an unauthenticated peer drives unbounded task and
descriptor growth. `host-wire-protocol.md:161` states the requirement as a
MUST, and the code satisfies it; the residual is the class-crossing window.
Open questions:
- Should the 2-second post-auth setup window have its own bound rather than
  sharing `max_connections`? Sixty-four concurrent stalled setups each hold a
  prepared ring, which is 128 MiB of arena per connection by
  `host-shm-transport.md:77`. (needs human input)


### setup-a-an-abandoned-setup-strands-no-ring-charge

Type: safety
Reachability: default-production - `ring.prepare` runs on every authenticated
connection (`connection.rs:148`) and all four post-prepare exits are on that
ungated path; `transport_setup_deadline` ships with a 2-second default
(`config.rs:227`).
Status: active
Exercised: partial - SIGKILL after `receive_grant` is covered; the `prepare`
timeout path is not, and reaching it is a race rather than a configuration (see
`Required faults`).
Guarantee: Every exit from `run_connection` that occurs after `ring.prepare`
succeeds and before activation completes releases the prepared ring's charge, so
repeated abandoned setups do not ratchet capacity.
Check: `always` - drive N abandoned setups through each distinct exit and assert
the ring accounting reported at `ring_transport.rs:186-190` returns to its
pre-attempt value. `always` because the accounting must balance after every
attempt, not eventually.
Fault/timing angle: the `prepare`-timeout exit is now explicit rather than
implicit. `timeout_at` (`connection.rs:121-124`) cannot abort the `spawn_blocking`
task, so on timeout `connection.rs:128-134` moves the join handle into a tracked
async task that awaits the late result and calls `late.sender.discard()` and
`late.root.cancel()`, the same pair the other three exits call inline
(`:166-169`, `:180-185`). The earlier mechanism, sender-drop closing the queue so
`run_endpoint` returns (`ring_transport.rs:437-440`) and reaches
`admission.release()` at `:291`, still holds as the backstop, with the `Admission`
guard's `Drop` (`profile.rs:581-586`) behind it. A campaign on this exit asserts
that the tracked late-cleanup task runs and performs both explicit operations,
then that the accounting returns to baseline.
Required faults and enabling state: three exits need a peer that stalls after
`receive_grant`, which `shm_failure_modes.rs:44-58` already builds. The fourth
needs `ring.prepare` to miss `transport_setup_deadline`, and **a near-zero
deadline does not deterministically force it.** `timeout_at(Instant::now() +
deadline, prepared)` races a timer against a `spawn_blocking` task that may
already have completed, so a fast `prepare` wins and the connection proceeds
normally; the test would pass having exercised the wrong path and would flake in
both directions. Reaching it deterministically needs either injected slowness
inside `prepare` - which is 2b's R1 and has no seam - or a barrier that holds the
blocking task past the deadline. That is why this record stays `partial`.
Confidence: high - [evidence](evidence/setup-a-an-abandoned-setup-strands-no-ring-charge.md).
Verified by inspection: the discard-and-cancel pairs at `connection.rs:166-169`
and `:180-185`, and the tracked late-cleanup task that performs the same pair on
the `prepare`-timeout exit at `:128-134`. Verified for this disposition and
previously recorded as unverified: `FrameSender` holds the queue's only
`mpsc::Sender` (`frame_channel.rs:685-694`), `run_endpoint` returns when
`queue.recv()` yields `None` (`ring_transport.rs:437-440`), and
`admission.release()` at `:291` is unconditional and outside the `catch_unwind`.
Part 1's `charge-release-never-silently-strands` remains the neighbouring
obligation.
Existing check: `crates/host-runtime/tests/shm_failure_modes.rs:232-245`
`setup_active_and_idle_sigkill_each_return_exact_capacity` and `:247-263`
`repeated_crashes_do_not_ratchet_single_connection_capacity` cover a killed peer
in the `setup` role, which is the post-grant pre-activation state. Neither
reaches the `prepare` timeout. Status unaudited.
Impact: with `max_connections = 1` a single stranded charge is a permanent
denial. The existing tests were written for exactly that reason, so the
uncovered exit is a gap in an otherwise deliberate campaign. What the mechanism
above changes is the shape of the gap: the risk is not that the charge is
stranded today, it is that the only thing returning it on that exit is a
channel-closure side effect three files away, which a refactor that gave the
endpoint thread another sender clone would silently remove.
Open questions: None.


### setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline

Type: liveness
Reachability: default-production - `activate_server` is called at
`connection.rs:170-179` on every authenticated connection and is passed
`shared.timing.transport_setup_deadline` (`:177`), which ships at 2 seconds
(`config.rs:227`). No opt-in and no alternative branch.
Status: active
Exercised: not yet - the existing stalling peer (`shm_failure_modes.rs:44-58`)
parks on `std::future::pending()` forever and the test asserts capacity return, so
nothing asserts that the host tore the setup down or when.
Guarantee: A peer that authenticates and then stalls anywhere in the post-grant
setup exchange has its connection torn down, and its handshake and connection
permits and ring charge released, refused within one `transport_setup_deadline` of the
grant send; the permits and charge are released once the endpoint thread the
refusal cancels has exited, which the code does not bound in time.
Check: `always` - evaluated at the close of an explicit bounded window. Drive a
peer that authenticates and then stalls at each post-grant I/O position in turn:
before the `Activate` message, mid-length-prefix, after `Activate` (so the host
blocks in the `Activated` write against a full peer buffer), before `Commit`, and
after `Commit` (blocking the `Committed` write); at each position **stop all peer
activity**, which is what makes the window fault-free; then assert two bounds in
sequence: `activate_server` returns its timeout by `transport_setup_deadline`
measured from the deadline anchor, and the teardown that follows it, the discard
and cancel at `connection.rs:166-169` and the ring-charge release the endpoint
thread performs at `ring_transport.rs:273-274`, is asserted as a separate safety clause at task quiescence, not as part of
this liveness bound: after the endpoint's completion signal (`done_rx`, awaited
by the tracked io task at `ring_transport.rs:284`) has resolved, the handshake
and connection permits and the ring charge are back at baseline
(`admission.release()` is unconditional on exit, `:273-274`). No
`lifecycle_callback_deadline` or join timeout wraps the endpoint task, so the
code enforces no wall-clock bound on that exit and this record claims none; a
campaign that awaits `done_rx` without its own cap can hang rather than refute,
so the harness applies a fixed campaign timeout to that await and reports a hit as
"cleanup not observed", which is inconclusive for the liveness clause and a
failure only for the release clause if resources are still held when the endpoint
has in fact exited. The bound is stated
in the unit the code bounds, a **single absolute deadline**:
`activate_server` computes `deadline = Instant::now() + timeout` **once**
(`setup_socket.rs:244-246`) and threads that same `Instant` through every
subsequent I/O - `send_grant` (`:247-258`), the `Activate` read (`:259`), the
`Activated` write (`:271`), the `Commit` read (`:279`), and the `Committed` write
(`:280`) - and `read_message` enforces it with `timeout_at` on **both** its
`read_exact` calls (`:365-367`, `:373-375`). So there is no accumulation across
messages: a peer that stalls at any of the four message positions, or at three of
four length bytes, is refused at the same wall-clock instant. `always` because the
bound must hold every time the window closes.
Fault/timing angle: the interesting property is that the deadline is *absolute
and shared*, not per-message. A per-message timeout would let a peer that
dribbles one byte per interval hold the setup open indefinitely; this construction
forbids that by design, and the property is that the single-anchor construction
does not regress into a per-read one. The teardown that follows is
`connection.rs:180-185`: `activate_server` returning `Err` runs `sender.discard()`
and `root.cancel()` and returns, which drops the permits and releases the ring
charge through the mechanism recorded in
[setup-a-an-abandoned-setup-strands-no-ring-charge](#setup-a-an-abandoned-setup-strands-no-ring-charge).
Required faults and enabling state: a peer that authenticates and then stalls in
the setup exchange. `tests/shm_failure_modes.rs:44-58` already builds exactly this
peer against a real host and runs in CI (`ci.yml:133`); the missing part is an
assertion on *when* the host gave up, not a new fixture. A shortened
`transport_setup_deadline` through `TestHost::start_with` makes the window cheap to
observe, and unlike the `prepare`-timeout exit this needs no race: the peer's
silence, not a scheduling outcome, is what makes the deadline fire.
Confidence: high - [evidence](evidence/setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline.md).
**Evidence file written**: this record was added by the portfolio
disposition, which was scoped to `catalog.md`, `fault-map.md`, and
`portfolio-evaluation.md` and forbidden from writing under `evidence/`. The link is
written to the schema's target so it resolves once the file lands, and the gap is
recorded in the process caveat of
[portfolio-evaluation.md](setup-identity/portfolio-evaluation.md). Everything the file would hold
is verified and stated here.
Verified: the single `deadline` computation at `setup_socket.rs:246-248` and its
reuse at `:249-260`, `:261`, `:273`, `:281`, and `:282`; `read_message`
(`:369-386`) wrapping both reads in `timeout_at(deadline, ..)` and mapping expiry
to `SetupError::Timeout`; `activate_server` being called with
`shared.timing.transport_setup_deadline` at `connection.rs:177`; the default of 2
seconds at `config.rs:227`; and the discard-and-cancel teardown at
`connection.rs:180-185`.
Existing check: none. `tests/shm_failure_modes.rs:232-245`
`setup_active_and_idle_sigkill_each_return_exact_capacity` asserts capacity returns
after a killed peer, which is a different exit and carries no timing claim.
Status unaudited.
Impact: **this is the part's only bound on how long an authenticated peer can hold
a prepared ring without completing setup**, and the resource it holds is the
expensive one. `setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released`
records that the post-auth setup window is charged to `max_connections` (64) rather
than to `max_handshakes` (32), and its own open question observes that 64
concurrent stalled setups each hold a prepared ring. Whether that is 2 seconds of
exposure or unbounded exposure is exactly this record, and nothing else in the
catalog states it.
Open questions:
- Should the post-grant exchange have a tighter deadline than the pre-grant one?
  Both halves currently share `transport_setup_deadline`, but only the post-grant
  half holds a prepared ring. (needs human input)


### setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap

Type: safety
Reachability: default-production - `observe_peer` runs for the whole life of
every activated connection (`connection.rs:196-206`), reached unconditionally
after commit; `MAX_SETUP_MESSAGE_LEN` is a compile-time constant
(`setup_socket.rs:24`) with no configuration behind it.
Status: active
Exercised: partial - the two `PeerClose` outcomes are tested; the cap is not.
Guarantee: The post-commit sentinel read never allocates more than
`MAX_SETUP_MESSAGE_LEN`, whatever length the peer declares.
Check: `always` - for declared lengths of exactly `MAX_SETUP_MESSAGE_LEN` (16 KiB, `setup_socket.rs:25`), `MAX_SETUP_MESSAGE_LEN + 1`, and `u32::MAX`, the exact-cap read is accepted and both over-cap reads return `SetupError::MessageTooLarge` before any body allocation; and across every sentinel read the maximum attempted allocation is at most `MAX_SETUP_MESSAGE_LEN` bytes (observed through the allocator or a counting wrapper), so the check protects the boundary rather than one 4 GiB refusal. `always` because it must hold on every sentinel read.
must hold on every sentinel read.
**This record was split under the portfolio disposition.** It previously carried a
second clause, "and it always yields to `read_cancel`", which is a liveness
obligation about eventual task exit rather than a safety invariant about
allocation, and METHOD.md's schema gives each record exactly one `Type`. That
clause is now
[setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input](#setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input),
with an explicit bound, because smuggled into a safety record it had no bound at
all.
Fault/timing angle: none for the cap itself; the check at
`setup_socket.rs:361-363` precedes the allocation at `:364`, so the ordering is
straight-line and the property is that the ordering does not regress.
Required faults and enabling state: a peer that completes commit and then sends a
huge length prefix.
Confidence: high - [evidence](evidence/setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap.md).
Verified: the cap at `setup_socket.rs:361-363` precedes the `vec![0u8; len]` at
`:364`, and `MAX_SETUP_MESSAGE_LEN` is `16 * 1024` (`:24`).
Existing check: `setup_socket.rs:810-825`
`goodbye_and_eof_have_distinct_outcomes` covers the `Goodbye` and EOF
classifications. `:599-651` `activation_and_commit_complete_on_setup_socket`
covers the `ProtocolError` classification. None covers the cap. Status unaudited.
Impact: the cap is the only thing between a post-commit peer and a 4 GiB
allocation, on a read that has no deadline at all, so it is what keeps an idle
authenticated connection cheap.
Open questions:
- Should `read_message_unbounded` be renamed to say what it actually is,
  time-unbounded and length-capped? The current name invites the exact wrong
  conclusion, and the re-scope document drew it. (needs human input)


### setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input

Type: liveness
Reachability: default-production - the sentinel task is spawned for every
activated connection (`connection.rs:195-207`) and `read_cancel` is the
generation's own child token, cancelled on every close path including peer death
(`:203-204`) and generation teardown.
Status: active
Exercised: not yet - no test parks the sentinel and then cancels it. The two
`PeerClose` outcomes that are tested (`setup_socket.rs:810-825`) both arrive
through the peer rather than through cancellation.
Guarantee: Once `read_cancel` fires, the sentinel task completes without requiring
any further byte from the peer, even when it is parked mid-message.
Check: `always` - evaluated at the close of an explicit bounded window. Park the
sentinel by sending three of the four length-prefix bytes and stopping, cancel
`read_cancel`, **send nothing further**, then poll the parked sentinel future
exactly once and assert that poll returns `Ready`, and assert the generation's
tracked task set is then empty; the attempt cap is one, fixed by the record, not a
test parameter. The bound is stated in the unit the
code bounds, which is a **cancellation edge and one poll of a `biased` select**,
not a duration: `connection.rs:180-190` is `tokio::select!` with `biased` and
`peer_read_cancel.cancelled()` as its **first** arm (`:182`), so the cancellation
branch is chosen the next time the task is polled and the `observe_peer` future is
dropped where it stands. A regression that adds yields after the cancellation arm fails
the one-poll assertion, which a generous cap would hide.
**This record exists because an earlier revision rejected it on a misreading of
METHOD.md, and the misreading is corrected here.** That revision argued no liveness
record could be written for this part because the available bounds are wall-clock
durations, "not an attempt count or an explicit interval the code reasons about".
METHOD.md's liveness rule says the opposite in its own words: "State the bound in
the units the code actually bounds: attempts, deadlines, or an explicit interval."
A deadline is an admissible bound. What the rule forbids is an unbounded
"eventually" and a generous timeout standing in for a bound, neither of which this
check or its sibling below uses.
Fault/timing angle: the whole property. `read_message_unbounded`
(`setup_socket.rs:355-367`) has no deadline, so a peer that sends a partial length
prefix and stops parks the read forever. That is intentional: the sentinel's
purpose is to notice the peer, and its bound is cancellation rather than time. The
name is the hazard, not the behaviour, and it resolves the re-scope open question at
`part-2-rescope/scope-map-and-risk-ranking.md:744-746`. The consequence is that
cancellation is the **only** exit from a parked sentinel, so if the `biased`
ordering were lost or the first arm removed, an idle connection would hold a task
and a socket until the peer chose to release them.
Required faults and enabling state: a peer that sends a partial length prefix and
then stalls, plus a cancellation of `read_cancel` while it is parked. Both halves
are in-process over a `UnixStream::pair`, the shape `setup_socket.rs:810-825`
already uses.
Confidence: high - [evidence](evidence/setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input.md).
Verified: `read_message_unbounded` (`:355-367`) applies no `timeout_at`, unlike
`read_message` (`:369-386`) which wraps both `read_exact` calls; the `select!` at
`connection.rs:196-206` is `biased` with `peer_read_cancel.cancelled()` first
(`:198`); `observe_peer` is `setup_socket.rs:345-353`. The evidence file for
this record is its own; the safety sibling links
`evidence/setup-a-the-peer-lifetime-sentinel-allocates-under-a-cap.md`, and the
evidence files re-verify the line citations above against this tree.
Existing check: none. `setup_socket.rs:810-825`
`goodbye_and_eof_have_distinct_outcomes` reaches `observe_peer` but always through
a peer-driven outcome, never through cancellation. Status unaudited.
Impact: this is the exit that makes an idle authenticated connection releasable on
the host's own schedule. Without it, teardown of a connection whose peer has gone
quiet mid-message depends on the peer, which is the one party a teardown path must
not depend on.
Open questions: None.


### setup-a-concurrent-setup-saturation-is-reached

Type: reachability
Reachability: default-production - reaching it needs only enough concurrent
peers, both bounds ship enabled.
Status: active
Exercised: not yet - the saturation tests pin `max_handshakes` to 1 or 4 and use
sockets that never speak, so they never produce the mixed state.
Guarantee: A campaign actually reaches the state in which the unauthenticated
handshake class is saturated at the same time as at least one authenticated
connection sits between the descriptor send and the `Activated` reply.
Check: `sometimes` - a marker fires when, at one observation, handshake permits
available equals zero **and** at least one connection is inside
`activate_server` between `setup_socket.rs:258` and `:271`. The two clauses are
independent preconditions of the vulnerable window, so the marker still fires on
a correct implementation, per the coverage-check rule. `sometimes` and not
`reachable` because a campaign can execute every line of both bounding paths
while never producing the concurrent operational state that makes the
class-crossing window at `connection.rs:106-110` observable.
Fault/timing angle: this record exists because records
`setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released`,
`setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it` and
`setup-a-an-abandoned-setup-strands-no-ring-charge` are all vacuous unless
concurrent setups overlap. With `max_handshakes = 1` they cannot.
Required faults and enabling state: `max_handshakes` and `max_connections` both
above 1, more concurrent dialers than `max_handshakes`, and at least one dialer
that authenticates and then delays its `Activate` inside the setup deadline.
Confidence: high - [evidence](evidence/setup-a-concurrent-setup-saturation-is-reached.md).
Verified: the two existing saturation tests set `max_handshakes` to 1
(`tests/lifecycle.rs:239`) and 4 (`:339`) and both use squatters that never
speak (`:243-244`, `:355-357`), so neither can populate the second clause.
Existing check: none. The two lifecycle tests establish the first clause only.
Impact: without this marker the bounding and scoping records can pass on a
campaign that never ran two setups at once, which is the same vacuity Part 1's
Group M records were introduced to prevent.
Open questions: None.

## Group S5: the two peer halves and the inverted asymmetry

Two records on the far side of the boundary, and they are the part's two
`Exercised: not yet` records with no partial credit at all.

The first is the parity claim `docs/shm-transport.md:83` makes, that
"Managed Rust clients use the same setup protocol, ring profile, wire version, and
descriptor schema." Three concrete divergences say otherwise, and one of them is a
reversed predicate rather than a missing check. The second is narrower and
sharper: `attach` (`packages/shm-native/src/lib.rs:491`) is a published napi
export that takes caller-supplied raw fd integers (`:510-513`) and reaches the
same thread-local channel registry as the authenticated `connect_setup` (`:571`),
with no `#[cfg(test)]` and no `#[doc(hidden)]`. Both are exposed in TypeScript
(`packages/shm-native/index.ts:526-529`, `:531-534`) and only `connectSetup` is
used by the shipped frame channel (`shm-frame-channel.ts:77`).

They are grouped because they share one consequence: any argument of the form "the
peer must have authenticated to hold this ring" is unsound for an in-process
caller, and the boundary that would have caught a bad grant is now the *native*
one, which the managed Rust client does not go through.

### setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection

Type: safety
Reachability: test-only - both peers lack an in-tree production caller.
`host_runtime::Client::connect` is called only from `tests/`, `benches/`, and
`examples/` (workspace-wide search), and `NativeChannel.connectSetup` has no
shipped-plugin caller here because `packages/plugin` is not in this tree; the
native reference `shm-frame-channel.ts:77` is source-repository evidence.
Public visibility and possible embedder use do not make either path
`default-production` under this catalog's reachability convention.
Status: active
Exercised: not yet - no test drives a malformed grant at the managed Rust peer.
Guarantee: Every grant-level rejection the native peer performs is also performed
by the managed Rust peer, so choosing the Rust client cannot admit a descriptor
the native addon would refuse.
Check: `always` - for each native rejection reason, construct the grant that
triggers it and assert the managed Rust path also refuses. Enumerated from the
native side: wire version (`setup.rs:113`), descriptor schema (`:114`), grant hex
and decode (`:118-119`), profile (`:120`), grant distinctness (`:120`), the
process-wide claim (`lib.rs:673-676` in `connect_setup`, `:608-611` in `attach`),
the descriptor-count and ancillary-shape rejections both receivers perform, and
an unknown top-level field on an otherwise valid grant, which the native
`GrantMessage` refuses through `deny_unknown_fields`
(`packages/shm-native/src/setup.rs:37-46`) while the managed `GrantMessage`
(`crates/host-runtime/src/setup_socket.rs:53-60`) carries no such attribute; that
case is the predicted violation at HEAD and the campaign must include it.
`always` because it is a per-descriptor invariant, the same shape as Part 1's
`native-boundary-not-weaker-than-its-wrapper`.
Fault/timing angle: none temporal. The exposure is a divergence in two
independently maintained validation lists.
Required faults and enabling state: a host, or a stand-in, that emits a grant
naming two identical grant strings, or a second concurrent attach of the same
grant in one process.
Confidence: high - [evidence](evidence/setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection.md).
Verified: two divergences, with the native-side line numbers corrected from lens A
per the provenance note above. First, `ring_transport.rs:646-650` compares
`from_host_grant.geometry() != to_host_grant.geometry()` and rejects on
*inequality*, whereas native `setup.rs:122` and `lib.rs:588-590` reject on grant
*equality*; the two checks are not the same predicate and the managed path admits
the aliased pair the native path refuses. Second, native `lib.rs:540-543`
(`attach`) and `:591-594` (`connect_setup`) take a process-wide
`GrantReservation::claim`; the managed Rust path takes no claim at all. `setup_socket.rs:302-306`, the managed peer's setup step,
checks only wire version and schema and returns the descriptor as an
unvalidated `serde_json::Value`.
Existing check: none on the managed Rust peer. Part 1's
`native-boundary-not-weaker-than-its-wrapper` recorded the mirror-image gap and
is still the reference for method.
Impact: **the asymmetry Part 1 found has inverted rather than closed.** The
refactor added `packages/shm-native/src/setup.rs` and moved profile, decode,
alias and replay-claim checks into the native boundary, so the native side is now
the stronger one. The weaker boundary is the managed Rust client. Part 1's record
should be re-read with that in mind rather than assumed still-oriented.
Open questions:
- Can an aliased grant pair actually arise? The only producer is
  `ring_transport.rs:324-327`, which encodes two distinct rings, so today this is
  latent. It becomes live under a rogue or impersonating host, which is the
  threat model this lens is written against.


### setup-a-only-an-authenticated-grant-enters-the-native-channel-registry

Type: safety
Reachability: test-only - `connect_setup` (`packages/shm-native/src/lib.rs:774`) and
`attach` (`:525`) are both `#[napi]` exports surfaced by `NativeChannel.connectSetup`
(`packages/shm-native/index.ts:542-545`) and `NativeChannel.attach` (`:537-540`);
their only in-tree callers are `packages/shm-native/tests/`. The plugin that would
call `connectSetup` on the shipped frame-channel path (`shm-frame-channel.ts:77` in
the source repository) is not in this tree, since `packages/` holds only
`shm-native`. `attach` carries no `#[cfg(test)]` and no `#[doc(hidden)]`, so it is
production surface by visibility; the shipped-plugin census below is
source-repository evidence and is the reason the guarantee is scoped as it is.
Status: active
Exercised: not yet - no test asserts the shipped wrapper never reaches `attach`.
Guarantee: **In the shipped plugin path**, every channel inserted into the native
registry originates from `connect_setup`, which authenticated over the setup
socket, and never from `attach`, which takes caller-supplied descriptors and
authenticates nothing.
Check: `always` - instrument the three `insert_channel` call sites, `lib.rs:619`
(from `attach`), `:745` (from the `finish_setup` task that completes
`connect_setup`), and `:823` (from `create_test_pair`, a test-only export), and
assert that a full shipped-plugin run inserts only through the `connect_setup`
path. `always` rather than
`unreachable` because `attach` is a published export that tests and embedders may
legitimately call; the forbidden thing is a *state*, a registry entry with no
authenticated provenance, and METHOD's rule for a forbidden state with no
dedicated detection point is `always(!X)`.
**The scope of this guarantee is narrowed, and the narrowing matters.** An earlier
form of this record read as a claim over the addon as a whole. It cannot be one.
`attach` is a `#[napi]` export (`lib.rs:524-525`) reachable from any JavaScript in
the process, so no campaign can establish that *no* caller reaches it - a claim
universally quantified over the callers of a published API is not falsifiable by
running the shipped wrapper, and it is **false** as stated for an arbitrary
embedder, who may call `NativeChannel.attach` deliberately and correctly. What is
provable, and what this record now claims, is the narrower call-graph fact about
the **shipped plugin**: the only `NativeChannel` construction on the plugin's
frame-channel path is `connectSetup` (`shm-frame-channel.ts:77`), and a census
over the source repository's `packages/plugin/src` finds no other; that package is
not in this tree. Stated plainly, so a later reader does
not recover the stronger claim: **an unauthenticated registry entry is reachable
in-process by design, and this property only says the shipped plugin does not
create one.**
Fault/timing angle: none. This is a call-graph property.
Required faults and enabling state: none beyond running the shipped plugin with
both sites instrumented.
Confidence: high - [evidence](evidence/setup-a-only-an-authenticated-grant-enters-the-native-channel-registry.md).
Verified: `attach` at `lib.rs:525` reads `hostToPeerFd` and `peerToHostFd` as
caller-supplied integers (`:510-513`) and never touches a socket;
`connect_setup` at `:774` starts the `BeginSetupTask` whose completion calls `setup::connect` which performs the three-message
handshake (`setup.rs:107-113`). Both end in `insert_channel` on the same
`REGISTRY`, at `:551` and `:612`. `index.ts:537-540` and `:542-545` expose both;
`shm-frame-channel.ts:77` uses only `connectSetup`, and grepping
the source repository's `packages/plugin/src` for any other `.attach(` call site
returns nothing outside tests; that package is absent from this tree.
Existing check: Part 1's `test-only-surface-absent-from-the-shipped-addon` is the
neighbouring property and should be checked for whether it already covers
`attach`. `attach` carries no `#[cfg(test)]` and no `#[doc(hidden)]`, so on the
face of it that record does not reach it. Status unaudited.
Impact: `attach` is an authenticated-path bypass reachable from JavaScript in the
same process. Under the same-uid trust model that is not a privilege escalation,
and it may well be intended surface - `create_test_pair` at `lib.rs:631` suggests
a test reading and a worker-thread re-attach reading is equally consistent with
the code. What it does mean, regardless of intent, is that **the setup socket is
not the only way into the ring**, so any reasoning of the form "the peer must have
authenticated to hold this ring" is unsound for in-process callers. That is the
consequence worth protecting against regression, and it is why this record stays
in the catalog after the narrowing.
Open questions:
- Is `attach` intended as production surface, test surface, or a
  worker-thread re-attach path? `create_test_pair` at `lib.rs:631` suggests the
  test reading. If it is test surface, the check strengthens to a build-time
  assertion that the shipped addon does not export it, which is Part 1's
  neighbouring record; if it is production surface, the narrowed guarantee above
  is the strongest form available. (needs human input)


## Relationship map

Grouped by shared mechanism rather than by the section headings above, because the
sharpest relationships cross groups. **Every dominance statement below is a
hypothesis** about which oracle subsumes which, offered to order the work, not a
verified claim. None has been tested, and for 14 of the 16 records no check
executes anywhere, so nothing here has been measured.

- **The gate, and the fact that nothing after it is a gate.**
  [setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof](#setup-a-no-descriptor-leaves-the-host-without-a-verified-client-proof),
  [setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token](#setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token),
  [setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it](#setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it).
  One sequence read three ways. The first record is the only one of the three whose
  failure is a boundary breach; the other two describe what the boundary does not
  cover. Hypothesis: the first *dominates neither*, because it constrains the
  ordering `auth` then `send_grant` and says nothing about what happens between
  `send_grant` and the token compare, which is exactly the window the second
  record is about. The token-scoping record is the one with genuine test payoff
  in this cluster, because it is the only one with a negative outcome the host
  emits, `SetupError::InvalidActivation` (`setup_socket.rs:275`), and that outcome
  has no test at all today: `stale_wire_or_descriptor_schema_is_invalid_identity`
  (`:725-765`) covers the wire-version and schema half of the same match arm and
  yields `InvalidIdentity` instead.

- **Only the host's randomness is fresh.**
  [setup-a-a-captured-client-proof-never-authenticates-twice](#setup-a-a-captured-client-proof-never-authenticates-twice),
  [setup-a-credentials-do-not-survive-a-host-incarnation](#setup-a-credentials-do-not-survive-a-host-incarnation).
  Two replay horizons over one construction. Within an incarnation the defence is
  the per-handshake `server_nonce` (`auth.rs:245`); across incarnations it is the
  per-start key and daemon id (`instance.rs:263-266`). Hypothesis: the
  incarnation record *dominates* the within-incarnation one for the specific
  mutation "the nonce became derived or counter-based", because rotating
  credentials would still refuse the old snapshot; it does **not** dominate for
  the mutation that matters more, a fixed nonce inside one incarnation, which
  rotation does nothing about. Both are cheap and neither is a substitute for the
  other. Note what the existing test does here:
  `repeated_handshakes_receive_fresh_server_nonces` (`auth.rs:924-939`) asserts
  distinctness, which a merely-incrementing counter satisfies, so it would survive
  the mutation the record exists to catch.

- **Two impersonations, in opposite directions.**
  [setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof](#setup-a-a-rogue-listener-at-the-published-path-obtains-no-client-proof),
  [setup-a-a-hostile-occupant-of-the-socket-path-fails-closed](#setup-a-a-hostile-occupant-of-the-socket-path-fails-closed),
  [setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid](#setup-a-the-setup-socket-is-never-connectable-outside-the-owning-uid).
  Three records about one filesystem name. The occupant record is the host
  refusing to adopt whatever is already at the path; the mode record is the host
  not leaving a window at the path it just created; the rogue-listener record is
  the peer refusing whatever answers at the path. Hypothesis: **no dominance in
  either direction**, and the interesting fact is the composition rather than any
  ordering. Neither peer checks the socket's owner, mode, or type before
  connecting (`client.rs:347`, native `setup.rs:106`), so the rogue-listener
  record is the *only* thing standing between a same-uid squatter and an honest
  peer's `ClientAuth`, and it is the one record in this cluster whose refusal
  reasons are individually tested (`auth.rs:1022-1073`, driven at `:1074-1081` and
  `:1082-1089`). The `daemon_ver` mismatch case is not visibly covered by either
  driver.

- **Every charge must come back, and the marker that proves anyone looked.**
  [setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released](#setup-a-unauthenticated-setup-work-is-bounded-and-every-slot-is-released),
  [setup-a-an-abandoned-setup-strands-no-ring-charge](#setup-a-an-abandoned-setup-strands-no-ring-charge),
  [setup-a-concurrent-setup-saturation-is-reached](#setup-a-concurrent-setup-saturation-is-reached),
  [setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it](#setup-a-an-activation-token-is-scoped-to-the-connection-that-minted-it).
  The reachability record is the load-bearing one, and it is load-bearing by
  being depended on rather than by dominating. Its own record says so: the
  bounding record, the token-scoping record, and the charge record are each
  vacuous unless two setups overlap, and with `max_handshakes = 1` they cannot.
  Hypothesis: the two saturation lifecycle tests
  (`tests/lifecycle.rs:237`, `:337`) establish the marker's first clause, permits
  exhausted, and cannot establish the second, a connection parked inside
  `activate_server`, because their squatters never authenticate. One harness
  change, a dialer that authenticates and then delays its `Activate` inside the
  2-second setup deadline, populates the second clause and serves all four
  records. That makes this the cheapest cluster in the part by payoff, and the
  fixture it needs is a small variation on one that already exists.

- **The one exit with no discard, and the question 2b has now answered.**
  [setup-a-an-abandoned-setup-strands-no-ring-charge](#setup-a-an-abandoned-setup-strands-no-ring-charge),
  [setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline](#setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline).
  Called out separately because it **was** the part's only medium-confidence record
  and the reason was a scope boundary rather than a weak reading. Three of the four
  post-prepare exits pair `sender.discard()` with `root.cancel()`
  (`connection.rs:166-169`, `:180-185`). The fourth, the `prepare` timeout at
  `:157-164`, has no handle to discard, because the `PreparedRing` is dropped inside
  a detached `spawn_blocking` task tokio cannot abort. **The 2b dependency is now
  closed and the record is `high`.** Dropping the `PreparedRing` drops the
  `FrameSender` it carries (`frame_channel.rs:685-694`), the sole holder of the
  queue's `mpsc::Sender`, so the endpoint thread's `queue.recv()` returns `None`,
  `run_endpoint` returns (`ring_transport.rs:437-440`), and `admission.release()`
  runs at `ring_transport.rs:291` outside the `catch_unwind`. Part 1's
  `charge-release-never-silently-strands` remains the neighbouring obligation.
  **What keeps the record partial is a different fact, and the lens's construction
  refinement was wrong about it.** That refinement said the exit "needs no injected
  slowness, because `config.timing.transport_setup_deadline` is an ordinary config
  field" (`tests/lifecycle.rs:165`, `tests/activation.rs:127-128`). Setting the
  field is indeed easy, but it does not *force* the timeout:
  `timeout_at(Instant::now() + deadline, prepared)` races a timer against a
  `spawn_blocking` task that may already have finished, so a fast `prepare` wins,
  the connection proceeds normally, and the test exercises the wrong path. The exit
  is reachable and not deterministically reachable, which are different claims.
  Hypothesis: the deadline record beside it *dominates nothing* here, because it
  bounds the post-grant exchange and this exit happens before the grant; the two
  are adjacent on the same config field rather than on the same window.

- **Two validation lists maintained independently, and one bypass around both.**
  [setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection](#setup-a-the-managed-rust-peer-repeats-every-native-peer-rejection),
  [setup-a-only-an-authenticated-grant-enters-the-native-channel-registry](#setup-a-only-an-authenticated-grant-enters-the-native-channel-registry),
  [setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token](#setup-a-mapping-authority-derives-only-from-the-key-never-from-the-token).
  The most important cluster here, and it is one finding attacked from three
  sides: **nothing downstream of the handshake can be relied on to have gone
  through the handshake.** The managed Rust peer omits checks the native peer
  performs and reverses one of them; `attach` reaches the registry with no
  handshake at all; and even on the authenticated path the descriptors precede
  validation. Hypothesis: making `attach` unreachable from the shipped wrapper
  would dominate the registry record and *nothing else*, because the managed Rust
  divergence is in a different crate and the descriptor-before-token order is in
  the host. Conversely, a single differential harness that runs one grant through
  both peer implementations and compares dispositions dominates the whole first
  record at once, and it is the same shape as Part 1's
  `native-boundary-not-weaker-than-its-wrapper`, whose method transfers directly
  even though its polarity no longer does.

- **The absence that was not an absence: the two liveness records.**
  Lens A surfaced "no liveness record" against its own output and left it for
  synthesis. The earlier revision of this section answered it by declining to add
  one, on the reasoning that "a stalled setup is torn down within
  `transport_setup_deadline`" is "bounded by a wall-clock duration
  (`config.rs:227`), not by an attempt count or an explicit interval the code
  reasons about". **That reasoning was refuted by an independent evaluation and the
  refutation is correct.** METHOD.md's liveness rule names three admissible units
  and a deadline is the second of them: "State the bound in the units the code
  actually bounds: attempts, deadlines, or an explicit interval." What the rule
  forbids is an unbounded "eventually" and a generous timeout standing in for a
  bound. `transport_setup_deadline` is neither: it is a single absolute `Instant`
  the code computes once (`setup_socket.rs:246-248`) and enforces on every
  subsequent read and write, so it is a bound the code reasons about explicitly and
  in one place.
  So the part now has two liveness records, and they partition the two ways a
  stalled peer is released.
  [setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline](#setup-a-a-stalled-setup-is-torn-down-within-the-transport-setup-deadline)
  is the deadline half, covering everything from the grant send to `Committed`.
  [setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input](#setup-a-the-peer-lifetime-sentinel-exits-on-cancellation-without-further-peer-input)
  is the cancellation half, covering the one read that deliberately has no deadline
  (`read_message_unbounded`, `setup_socket.rs:355-367`), and its bound is a
  cancellation edge plus one poll of a `biased` select rather than any duration.
  Hypothesis: **no dominance in either direction**, and the reason is the reason
  they are two records. They bound different phases with different mechanisms, and
  the second was previously the trailing clause of a safety record where it had no
  bound at all - which is how a liveness obligation hides. Note what that means for
  the earlier revision's argument: it was right that restating the clause "with a
  generous timeout standing in for a bound" would violate the rule, and wrong to
  conclude that no bound existed. The bound is cancellation, and cancellation is
  observable.


## Sub-part 2d catalog: the host's own client as a protocol peer

Scope: the client the host crate ships and that production binaries use to speak
to a host, about 3,998 lines centred on `crates/host-runtime/src/client.rs`. That
count was re-derived with `wc -l` at `HEAD` and the file is the crate's largest.
Production occupies `1-2264`; `#[cfg(test)] mod tests` occupies `2266-3998`,
which is 1,733 lines, 43 percent of the file.

Boundary context, read but not cataloged: `ring_transport.rs` for
`RingClientEndpoint` and its 2-second per-write bound, `setup_socket.rs` for the
encoded goodbye, `connection.rs` for the host-side peer watcher, and
`docs/host-wire-protocol.md` as the normative peer contract. Parts 1, 2a, 2b,
and 2c own the host halves of every contract named here and are cited rather
than re-derived.

**This is a post-refactor surface.** The client's byte-stream half was deleted
and its ring half is what remains, so several normative statements still describe
a reader that no longer exists. Four commits carry the refactor, and all four
subjects were re-verified with `git log -1` at authoring time:

| Commit | Subject |
| --- | --- |
| `0f336d3c` | `refactor(shm): collapse to fixed ring transport` |
| `d8bde128` | `feat(host): add authenticated ring setup socket` |
| `793a973e` | `build(shm): require packaged native transport` |
| `ed487e11` | `refactor(host): make ring transport mandatory` |

`ed487e11` is the one that matters most here: it removed 351 lines from
`client.rs` and added 137, deleting `reader_loop<R: AsyncRead>`,
`read_active_frame`, `read_exact_until`, `read_body_until`, `drain_until`,
`negotiate_tcp`, `read_setup_frame`, `read_setup_exact`,
`NEGOTIATION_CORRELATION`, `READ_BUFFER_BYTES`, and three tests. It also moved
`FIRST_APPLICATION_CORRELATION` from 2 to 1 (`client.rs:111`), because the
negotiation request that owned correlation 1 is gone.

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). System
`the `host` source checkout, branch
`feat/shared-memory-release-gate-audit`, `HEAD` = `e447c927`
("refactor(shm): trim final review leftovers"), confirmed with
`git branch --show-current` and `git log -1`. Both lens agents read and verified
their line references at that commit. Scope and CI findings come from
`part-2-rescope/scope-map-and-risk-ranking.md` (a source-tree artifact that was not migrated into this repository).

**Lens B re-derived every citation lens A made and corrected three, so lens B's
line numbers win wherever the two differ.** All three are in the normative
document and none changes a finding.

- The client `Recovering` state is at `docs/host-wire-protocol.md:762-764`,
  and the quoted string "bounded backoff, reread file" is at `:764`. Lens A's
  earlier section cited `:12`, which is "This document is the direct-only wire
  authority". Lens A's own lead L4 cites section 12 correctly.
- The shared queued-byte sentence is at `:745`, not `:746`. `:746` is blank.
  Verified by printing `:745`, which reads "Data and reserved-control frames
  share one queued-byte budget; reserved admission is not a byte-budget bypass."
- The `not_sent` definition is at `:60`, not `:62`. `:62` is the `terminal`
  bullet.

This synthesis re-verified the citations it repeats and adds two corrections of
its own, both recorded where they land: the count of CI-named fixture binaries
in [existing-checks.md](client-peer/existing-checks.md), and the coverage of `Cancel` by
`inbound_validation_enforces_the_direct_profile_table`, resolved below.

## What this part is about

Eight facts frame every record here. The first two are the reason this sub-part
was cataloged, and the third is worth stating precisely because it is the strong
part.

**A clean host close and a transport failure share one code.** All four
bridge-thread fault exits collapse to the same caller-visible outcome, and so
does a host exiting without a channel-0 goodbye. The bridge thread's loop
(`client.rs:1866-1889`) leaves by five routes: an `endpoint.send` failure
(`:1873-1875`), `write_rx` disconnection (`:1877`), a `read_tx.blocking_send`
failure (`:1882-1884`), a `try_recv_with` error (`:1887`), and ordinary
cancellation at the loop head (`:1866`). Every one of them closes the inbound
channel, and `ring_reader_loop` has exactly one handler for that: `read.recv()`
yielding `None` reaches `inner.retire("eof")` at `:1987`. A host that exits after
its drain without emitting a channel-0 `Goodbye` produces the identical
`Ok(None)`-then-close sequence, so `eof` is the only signal for either case.
Worse, the retirement cause is never stored. `retire` (`:1667-1675`) takes a
`&'static str` and forwards it only to `settle_all` (`:1672`); `Inner`'s fields
(`:934-960`) hold no cause slot, which this synthesis confirmed by printing the
struct. So only a caller holding a pending entry at the instant of
`settle_all`'s loop (`:1654-1664`) sees any cause at all, and everyone arriving
later gets the constant `connection_retired` from `admit` (`:1129`, `:1145`) or
`generation_retired` from `send_control` via `retired_error` (`:1327`, `:2237`).
Eight cause categories, spelled with ten distinct literals - `connection_goodbye`
(`:1397`), `protocol_violation` (`:1557`, `:1979`), `eof` (`:1987`),
`write_failed` (`:1954`, `:1963`), `control_capacity_exhausted` (`:1341`,
`:1356`), `invalid_route_response` (`:486`),
`stranded_route_cleanup_failed` (`:1588`), and the three local lifecycle codes
`owner_drop` (`:744`), `owner_close_dropped` (`:766`), and `shutdown_timeout`
(`:676`) - collapse to two constants.

**The peer-death counter can under-report, and the over-report direction does not
hold.** The bridge thread attempts a setup-socket goodbye unconditionally after
every exit. `:1890-1893` runs `encoded_goodbye` then `shutdown(Both)` outside the
`while` loop that closes at `:1889`, with no branch on why the loop ended, so a
client whose ring collapsed attempts to depart looking clean. On the host side
`connection.rs:199-206` observes that socket and calls `record_peer_death()`
**only** when `close != PeerClose::Goodbye` (`:200`), which this synthesis printed
and confirmed. So the host can skip its peer-death record even on a real ring
fault. Two qualifications were added during disposition and both narrow the claim.
The write is best-effort with its result discarded (`:1890` is `if let Ok(..)`,
`:1891` is `let _ = setup.write_all(..)`), so an *attempt* is not a delivery. And
the host's watcher is a `biased` select whose first arm is
`peer_read_cancel.cancelled()` (`connection.rs:196-198`), so a generation already
retired from ring evidence stops watching the socket regardless of what arrives on
it.

**The inverse does not hold, and the original catalog claimed it did.** A clean
owner close does not present to the host as an abrupt EOF. `close` sends a ring
channel-0 `Goodbye` through `send_control_wait` (`:702`) and returns from it only
after the writer's completion channel resolved and the ack fired (`:1957-1971`),
so the goodbye reaches the ring *before* `cancel.cancel()` at `:711`; and the
setup socket is moved into the bridge closure at `:1854`, so `close` returning
closes nothing the host is reading. What survives is an unjoined teardown:
`close` returns while a detached OS thread still owns the socket, the ring attach,
and the write-completion channel, which is a gap against protocol `:691`'s
"followed by joined ring teardown and setup-socket close" rather than a
misattribution. Sub-part 2b established a genuinely adjacent blind spot on the
other end: a host that cannot create shared-memory objects reports
`state: "healthy"` with `error_class: null` while refusing every connection.

**In-flight requests are handled correctly.** This is the strong part and it is
worth stating plainly, because the rest of this section is failure attribution.
`settle_all` (`:1649`) takes the whole pending map under the `admission` mutex
(`:1651-1652`, verified by printing the `std::mem::take` under
`lock_unpoisoned(&self.admission)`), so settlement is atomic against admission
rather than racing it. Per entry it runs `cancel_classification` (`:2223`), whose
`QUEUED -> CANCELLED` compare-exchange (`:2225`) races the writer's own
`claim_for_write` gate (`:1939-1945`); the loser falls through to `classify`
(`:2215`), which maps both `WRITING` and `WRITTEN` to `OutcomeUnknown`.
`NotSent` is therefore issued only when the CAS won, which means the bytes
provably never left. And no request body is ever replayed: `request` is
documented "The body is never replayed" (`:531`) and the only retry loop in the
file is `open_route`'s (`:511-525`).

**The correlation watermark cannot be violated by this client.** One allocator
serves control and routed requests alike, constructed as
`Correlations::new(FIRST_APPLICATION_CORRELATION)` at `:393`. Allocation and
enqueue are atomic under the correlations guard: `admit` takes it at `:1176` and
holds it through allocation (`:1177`), frame encoding (`:1186`), and
`data_tx.try_send` (`:1207`), with the `admission` and `pending` mutexes held too
(`:1140-1141`). Enqueue order therefore equals allocation order, which is exactly
what `docs/host-wire-protocol.md:656` obliges of a sender. The rewind path
cannot break it either: `Correlations::restore` (`:1741-1747`) only rewinds when
`self.next == correlation.checked_add(1)`, or for the `u64::MAX` exhaustion case,
and both call sites (`:1196`, `:1209`) precede any delivery to the writer.

**Pending state is bounded except routes.** Pending requests are capped at 1,024
by `CLIENT_MAX_PENDING_REQUESTS` (`:53`) at `:1169`; live streams at 64 by
`CLIENT_MAX_LIVE_STREAMS` (`:55`) at `:1058`; and four byte counters are capped
by construction - `queue_budget`, `control_budget`, `_read_budget`, and
`retained_budget` (`:398-401`, declared `:945-954`). The route map (`:944`, a
`Mutex<HashSet<RouteHandle>>`) has no cap at its insertion point (`:507`), which
tests only `closed` (`:501`) and never `routes.len()`. A grep for
`CLIENT_MAX_LIVE_ROUTES` across the file returns zero hits. The bound is
transitive only, resting on the host's willingness to keep allocating channels,
against a protocol document that names routes explicitly at `:658`:
"Implementations MUST use finite limits for live connections, routes, pending
correlations, handler tasks, queued requests, and aggregate buffered bodies."

**A detached bridge thread is never joined.** `std::thread::Builder::new()` at
`:1852` spawns at `:1854` and the only combinator applied to the result is
`.map_err(...)` at `:1895`, so the `JoinHandle` is discarded. It busy-polls:
`Ok(None) => std::thread::sleep(Duration::from_micros(50))` at `:1886`, so one
idle connection spins an OS thread at roughly 20 kHz for its whole lifetime. It
owns three things nothing else can reach - the ring attach (`:1855`), the
completion signal every outbound frame waits on (`:1872`), and the setup-socket
departure the host reads as its peer-death discriminator (`:1890-1893`).

**Correction applied during disposition: the thread is not untested at every
level, and the original text said it was.** The in-crate half of that claim holds:
a grep of `mod tests` for `start_ring_bridge`, `RingClientEndpoint`,
`Client::connect`, or `TestHost` returns zero hits, and none of the six
`tests/client.rs` integration tests observes the thread or the socket. But two
CI-executed integration tests do observe its **exit**, indirectly and genuinely.
`tests/shm_soak.rs` runs a real `Client::connect`/`open_route`/`request`/`close`
cycle (`:54-92`) and then polls `wait_for_envelope` (`:35-52`) until the process
thread count equals a post-close baseline; `tests/shm_failure_modes.rs` does the
same through `assert_resources_return_to` (`:193-210`) in
`clean_close_returns_exact_single_connection_capacity` (`:218-230`). A bridge
thread that never left its loop would hold the count above baseline and fail both
assertions inside their budgets. Both run in CI at `ci.yml:130-135`
("Mandatory ring client suite"). What remains genuinely unobserved is everything
except termination: which `break` fired, whether `:1891` wrote anything, and the
50-microsecond spin, none of which any check reaches.

**Coverage: 40 in-crate tests, none in CI, all driving a synthetic inner.** The
count was re-derived here by grepping `#[test]` and `tokio::test` from `:2266`
onward, and it matches lens B exactly at 40; an initial pass of this synthesis
under-counted at 38 by missing `#[tokio::test(flavor = ..., worker_threads = 2)]`
forms, which is recorded so a later pass does not repeat it. All 40 live in one
`mod tests` at `:2266-3998` and all 40 build their subject through `test_inner`
(`:2270`), which constructs `Arc::new(Inner { .. })` directly with a
pre-populated route set. So there are **zero hits for the real `connect`
(`:306`), `connect_info` (`:347`), or bridge entry points**. None of the 40 runs
in CI, and the reason is structural: every `-p host-runtime` invocation in `ci.yml`
carries a `--test <name>` filter, which selects one integration binary and never
builds the lib target. Re-verified at `HEAD`: the 13 `host-runtime` hits are `:87`,
`:132`, `:133`, `:134`, `:168`, `:169`, `:178`, `:187`, `:190`, `:211`, `:361`,
`:442`, and `:461`, and `:168-169` are `cargo build`.

**CI in this tree.** `.github/workflows/ci.yml:118` and `:126` run
`cargo test --workspace --all-targets --all-features --locked` on the 1.98 and stable
toolchains, so every integration binary and every inline test this section counts
executes in CI. The named-versus-unnamed distinction and the `ci.yml` line numbers
below describe the source repository's workflow at authoring time and are kept as
provenance; they are not coverage gaps here.

Six integration tests in `crates/host-runtime/tests/client.rs` (243 lines) do run, and
the binary is named in CI twice at HEAD: `ci.yml:119` ("Mandatory ring client
suite", Linux, job `shm-crash-recovery`) and `:168-169` (a wrapped
`cargo nextest run -p host-runtime --test client --test lifecycle`, Linux, job
`shm-source-build`). The third site this preamble previously counted, a
"Fixed-ring contracts (macOS)" step, was removed with every other macOS job by
PR #131 (merge `5d638e3e8`); `ci.yml` at HEAD contains only `ubuntu-latest`
jobs. Both remaining commands were printed and confirmed. **All six exercise
the client as a peer**
rather than as a fixture: each constructs `Client::connect(host.publication_path())`
and asserts on the client's own observable behaviour.

**Zero doctests, so no check resident in `client.rs` is CI-executed.**
`grep -c '```'` over the file returns 0: there is no code fence, `text` fence, or
`compile_fail` block anywhere in its 3,998 lines. This matters because
`cargo test -p host-runtime --doc` **does** run, at `ci.yml:175` under the step name
"Rust lease non-escape", and it builds the lib target's doctests. Sub-part 2b has
two `compile_fail` doctests (`frame_channel.rs:296-301`, `:303-308`) and they are
its only CI-executed source-resident checks. 2d has no equivalent. The sub-part's
entire CI-executed coverage is the six integration tests in
`crates/host-runtime/tests/client.rs`, plus - corrected during disposition - the
thread-count assertions in `tests/shm_soak.rs` and
`tests/shm_failure_modes.rs`, which are CI-executed at `ci.yml:130-135` and are
the only CI evidence in the sub-part that the bridge thread terminates.

**Five normative doc statements describe a byte-stream reader the refactor
deleted.** One is in the code, four are in the normative document, and all five
were printed and confirmed.

1. `client.rs:44` - "Deadline for a frame after its first header byte. Idle
   header waits are unbounded." The client has no first header byte;
   `ring_reader_loop` receives an already-decoded
   `(EnvelopeHeader, Vec<u8>, ByteCharge)` triple from the bridge (`:1977`). The
   deleted `read_active_frame` and `read_exact_until` were what made the sentence
   true. `CLIENT_FRAME_TIMEOUT` (`:45`) survives as a real bound, but it bounds
   the *writer's* per-frame publication (`:1353`, `:1960`).
2. `docs/host-wire-protocol.md:738` - "frame completion after first header
   byte | one 30 s absolute deadline; idle first-header wait is unbounded". The
   same mechanism, stated normatively for both managed clients, neither of which
   reads a byte stream any more.
3. `:724` - "malformed framing / EOF | no terminal possible | classify pending
   writes from byte evidence; invalidate generation". There is no byte evidence
   on the ring path. The surviving classifier is the three-state `publish` atomic
   (`client.rs:1939-1967`, `:2215-2231`), which is *stronger* than byte evidence,
   not weaker. The obligation survives; the evidence it names does not.
4. `:852` (conformance vector V14) - "Partial header/body EOF | Close as
   corruption; pending write outcomes use byte evidence". Not constructible
   against the ring, because `:294` says "A published ring descriptor names one
   complete header and body."
5. `:296` - the client-side retirement list still contains "truncated declared
   frame" alongside items that are live on the ring path. Printed in full and
   confirmed: unexpected setup-socket EOF, invalid ring descriptor, unsupported
   version, unknown type, invalid flags, nonzero channel-0 epoch, zero epoch on a
   routed channel, pure-header body, and body declaration above 64 MiB are all
   reachable; a truncated declared frame is not, for the reason `:294` gives.

Lens B checked the opposite direction too, so a later pass does not double-count:
the document's transport-selector, provider-registration, and alternate-backend
statements (`:29`, `:594`, `:263-264`) are *negative* claims the deletion made
true, not descriptions of surviving machinery.

### One correction this synthesis resolved

Lens A recorded that whether
`inbound_validation_enforces_the_direct_profile_table` (`client.rs:2658`) covers
the `Cancel` disposition was unverified. It does not. The test runs `:2658-2751`,
from its `fn` line to its closing brace, and grepping that body for `Cancel`,
`Request`, `Pong`, `Hello`, and `HelloAck` returns exactly one hit,
`FrameType::Request` at `:2750`. So of the five frame types that land in
`validate_inbound`'s residue, the densest test in the sub-part asserts one. This
tightens
[client-a-a-host-originated-cancel-retires-the-generation](#client-a-a-host-originated-cancel-retires-the-generation)
and [client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production](#client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production);
the record text is preserved verbatim from lens A and this correction is carried
here rather than edited into it.

## Reachability: client peer

**All fourteen records are `test-only` in this tree.** The source catalog labelled
them `default-production` on four facts, and the first two still hold here:

1. `Client::connect` is `pub` at `client.rs:306` and carries no `cfg` gate.
2. It reaches the ring through `connect_info` (`:343`), then `start_ring_bridge`
   (`:378`), then `RingClientEndpoint::attach_with_descriptors` (`:1855`,
   defined `ring_transport.rs:636`). None is `cfg`-gated.
3. The production callers the source catalog cited, `crates/daemon/src/bin/eidnara-host.rs`
   and `ManagedConnector::connect` in `crates/daemon/src/historian_producer.rs`, are
   not in this tree: `crates/daemon` is scheduled for U4 (`docs/properties/README.md:52`),
   and a workspace-wide search finds `Client::connect` only in `crates/host-runtime/tests/`
   and benches. Public visibility without a shipped caller is `test-only` under
   METHOD rule 4.
4. The doc comment "Thread-confined peer endpoint for integration tests"
   (`ring_transport.rs:626`) is therefore accurate about this tree's callers, and
   `RING_PROFILE = "host-test-ring-v1"` (`ring_transport.rs:31`) is a name rather
   than a gate. Sub-part 2b reached the same two verdicts independently. Every
   record here reclassifies in the wave that lands a production caller.

Two code points are production-*unreachable* inside this surface, and both are
typed `reachability` with `unreachable` semantics at the record rather than
relabelled: `dispatch`'s catch-all at `:1557`, and - noted but not cataloged as
its own record - the two `unreachable!()` arms at `:1440` and `:1457`.

## Index

Fourteen records, in the order lens A proposed them. Lens B proposed none by
design; it built the 20-claim register and the check inventory.

| Slug | Type | Confidence |
| --- | --- | --- |
| [client-a-a-retired-generation-forgets-why-it-retired](#client-a-a-retired-generation-forgets-why-it-retired) | safety | high |
| [client-a-a-clean-host-close-and-a-transport-failure-share-one-code](#client-a-a-clean-host-close-and-a-transport-failure-share-one-code) | safety | high |
| [client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye](#client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye) | safety | high |
| [client-a-a-close-completes-before-its-setup-goodbye-is-written](#client-a-a-close-completes-before-its-setup-goodbye-is-written) | reachability | high |
| [client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome](#client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome) | safety | high |
| [client-a-no-request-frame-carries-a-non-increasing-correlation](#client-a-no-request-frame-carries-a-non-increasing-correlation) | safety | high |
| [client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault](#client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault) | safety | high |
| [client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget](#client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget) | liveness | high |
| [client-a-live-route-handles-are-bounded-only-by-the-host](#client-a-live-route-handles-are-bounded-only-by-the-host) | safety | high |
| [client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle](#client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle) | safety | high |
| [client-a-host-shutdown-success-rests-only-on-a-json-echo](#client-a-host-shutdown-success-rests-only-on-a-json-echo) | safety | high |
| [client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind](#client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind) | safety | high |
| [client-a-a-host-originated-cancel-retires-the-generation](#client-a-a-host-originated-cancel-retires-the-generation) | safety | high |
| [client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production](#client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production) | reachability | high |

Semantics distribution: twelve `always`, one `sometimes`, one `unreachable`. No
`always-or-unreached` and no `reachable`. Type distribution: eleven safety, two
reachability, one liveness. The distribution moved during disposition: the
`always-or-unreached` record became `always` once its optional branch was proved
impossible rather than merely unreached. See
[portfolio-evaluation.md](client-peer/portfolio-evaluation.md).

**The seven group headings below are this synthesis's own**, chosen by shared
mechanism rather than by the order records were proposed. Grouping reorders the
records relative to the index; the index is the record-order artifact. Record
bodies are verbatim from lens A. Two formatting-only changes were applied
uniformly: fields are wrapped to about 80 columns, and in
[client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind](#client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind)
lens A's separate `Check semantics rationale:` field is folded into the `Check:`
line, which is where METHOD puts the rationale. No wording was changed.

---

## Group A: the retirement cause, erased twice

Two records on the same discarded `&'static str`. The first is that the cause is
never stored, so only a caller already pending at the instant of settlement can
ever learn it. The second is that even that caller learns little, because the one
code it receives, `eof`, is shared by a healthy host exit and a ring transport
failure. They are grouped because both turn on `retire` (`client.rs:1667`)
forwarding its cause to `settle_all` and nowhere else, and because together they
mean the client retains no diagnosis of its own death.

### client-a-a-retired-generation-forgets-why-it-retired

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test asserts what a caller arriving after retirement can
learn about the cause
Guarantee: A caller that arrives after the generation retires can determine that
it retired but never why.
Check: `always` - whenever `retire` has run and a subsequent `admit` or
`send_control` rejects a call, the returned `CallError::code()` is one of the two
constants `connection_retired` (`client.rs:1078`) or `generation_retired`
(`:2290`), and never the `&'static str` that `retire` was called with. `always`
because the condition is evaluable at every post-retirement call, and the
property is about a total function from state to observable code rather than
about one window.
Fault/timing angle: The distinguishing information exists for exactly the
duration of `settle_all`'s loop (`:1654-1664`). A caller holding a pending entry
at that instant sees the cause; a caller that calls one instruction later does
not.
Required faults and enabling state: Retire the generation by any of the eight
cited causes with the `pending` map empty, then issue any call. Compare against
the same fault with one pending request outstanding.
Confidence: high - [evidence](evidence/client-a-a-retired-generation-forgets-why-it-retired.md).
Verified that `Inner` (`:934-960`) has no cause field, that `retire`
(`:1667-1675`) forwards `code` only to `settle_all`, and that both
post-retirement rejection sites use constants.
Existing check: none. `dropped_close_retires_and_repeated_close_joins_tasks`
(`:3121`) exercises retirement but asserts nothing about cause visibility.
Impact: An operator or a recovery policy cannot tell a host reload from a ring
fault after the fact. Combined with Part 2b's finding that the host reports
itself healthy on ring unavailability, neither side of the connection retains the
diagnosis.
Open questions:
- Should `Inner` carry a `retire_cause: OnceLock<&'static str>` so late callers
  get the real code? This changes the public `CallError` code set, so it is a
  compatibility decision. (needs human input)

### client-a-a-clean-host-close-and-a-transport-failure-share-one-code

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test drives the bridge thread's four distinct break paths
and compares the resulting caller-visible code
Guarantee: A pending caller cannot distinguish a host that shut down without a
channel-0 Goodbye from a ring transport failure, because both retire the
generation with the code `eof`.
Check: `always` - whenever `ring_reader_loop` (`client.rs:2047`) leaves its
`recv` loop because the bridge channel closed, the code passed to `retire` at
`:2058` is the literal `"eof"` regardless of which bridge-thread exit closed the
channel: the `ready_tx` failure at `:1840`, the setup-peer-closed check at
`:1844`, a failed write at `:1856`, a failed inbound send at `:1906`, the poll
readiness failure at `:1937`, the data-wait completion failure at `:1945`, or the
`HUP | ERR` poll flags at `:1951`. `always` because it is a claim about a single
code path's constant, checkable on every entry; `:1987` is the start of
`writer_loop` and is not the site.
Fault/timing angle: None. This is a static property of the code, and the two
operational causes it merges are unrelated in time.
Required faults and enabling state: Two runs with one pending request each. Run
A: host exits after its drain without emitting a channel-0 Goodbye. Run B: fail
`RingClientEndpoint::send` or `try_recv_with` so the bridge breaks at `:1874` or
`:1887`. Assert both callers observe `CallError::code() == "eof"`.
Confidence: high - [evidence](evidence/client-a-a-clean-host-close-and-a-transport-failure-share-one-code.md).
Verified all five bridge exits funnel into the same channel closure and that only
`:1987` handles it. Part 2b's
`ring-a-publish-failure-is-reported-as-a-clean-peer-close` establishes the
host-side half.
Existing check: none.
Impact: This is the significant finding the task anticipated. A recovery policy
that wants to back off on transport faults but reconnect promptly on a host
reload has no signal to branch on, and Part 2b established the host's own
diagnostics are equally silent, so the fault is invisible from both ends.
Open questions:
- Does a healthy host emit a channel-0 Goodbye before its ring closes?
  `docs/host-wire-protocol.md` step 4 of graceful shutdown says the host sends
  best-effort connection Goodbye after the drain, which would give
  `connection_goodbye` instead. Whether that step is reliably reached before the
  ring drops is a 2a or 2b question, not answerable from `client.rs`.
  (unresolved, needs a host-side trace)

---

## Group B: the departure signal, one direction not two

Two records on the same unconditional post-loop block at `client.rs:1890-1893`
read by the same host gate at `connection.rs:200`. **This disposition removed the
symmetry the group was originally built on.** The claim that a clean close
over-counts peer deaths does not hold, for two independent reasons verified
below: `close` sends and *waits for* a ring channel-0 `Goodbye` before it cancels
(`:699-711`), and the bridge thread still owns the setup socket after `close`
returns, so nothing produces an abrupt EOF at that moment. What survives on that
side is an unjoined-thread window, which is a real gap against protocol `:691`'s
joined-teardown requirement but is not a misattribution. The under-count
direction survives and is itself narrowed: the goodbye write is *attempted*
unconditionally, which is not the same as delivered.

### client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test observes the setup socket after a forced ring failure
Guarantee: The client's setup-socket departure signal does not distinguish a
clean exit from a transport failure: the bridge thread *attempts* the same
goodbye write on every exit, so whenever that write lands the host's peer-death
accounting under-reports ring faults.
Check: `always` - whenever the bridge thread leaves its `while` loop at
`client.rs:1842`, it reaches `:1954-1957` and attempts `encoded_goodbye` followed
by `shutdown(Both)`, with no branch on why the loop ended. `always` because the
post-loop block is unconditional and evaluable on every thread exit. **Scope
correction applied during disposition: the check is over the attempt, not over
the host's resulting classification.** `:1954` is `if let Ok(goodbye)` and `:1955`
is `let _ = setup.write_all(&goodbye)`, so both the encode and the write can fail
with the result discarded; and the host's watcher (`connection.rs:180-190`) is a
`biased` select whose first arm is `peer_read_cancel.cancelled()`, so a
generation already retired by ring evidence stops observing the socket before
`observe_peer` resolves. Asserting that `record_peer_death` did not fire therefore
requires establishing both that the write landed and that the watcher was still
armed, neither of which follows from this client's code. The conditional case is
therefore asserted as its own clause with its own setup: in a run where the
campaign establishes that the goodbye write landed while the watcher was still
armed (the host generation not yet retired by ring evidence), assert the host did
not count the ring failure as peer death; a run where either condition cannot be
established owes nothing on this clause.
Fault/timing angle: None for the write itself. The consequence lands on the host,
whose watcher at `connection.rs:199-206` calls `record_peer_death()` only for a
non-`Goodbye` close (`:200`), and only if that arm of the select wins.
Required faults and enabling state: Force the ring endpoint to fail after
activation, so the bridge breaks at `:1874` or `:1887`. Observe the host's setup
socket and assert whether `record_peer_death` fired. Per the scope correction,
record separately whether `:1891` returned `Ok` and whether the host's
`peer_read_cancel` was still uncancelled when `observe_peer` returned; without
both, a negative result is unattributable.
Confidence: high - [evidence](evidence/client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye.md).
Verified the post-loop block is outside every `break` and that
`connection.rs:200` gates the peer-death counter on the message. This disposition
additionally verified that the write result is discarded at `:1891` and that the
host's watcher is a biased select against `peer_read_cancel` (`:196-198`), which
is what demotes the consequence from proved to conditional.
Existing check: `setup_socket.rs:820` and `:824` assert `observe_peer` returns
`Goodbye` and `UnexpectedEof` respectively, but nothing ties either to a client
transport state.
Impact: Partial rather than established. If the write lands and the host's
watcher is still armed, the metric intended to count dead peers counts only peers
that failed to complete a socket write, and a fleet losing rings would look like
a fleet of well-behaved clients. Whether both conditions hold on a ring fault is
a 2b question about which side observes the collapse first, and it is the
difference between a metric that is wrong and a metric that is merely unproven.
Open questions:
- Should the bridge thread suppress the goodbye on its failure `break`s so the
  host classifies correctly? That makes a transport fault look like an abrupt
  EOF, which is the honest signal. (needs human input)
- On a ring fault, does the host retire its generation from ring evidence before
  `observe_peer` resolves? If it always does, the counter is unreachable on this
  path for a reason unrelated to the goodbye, and this record's consequence
  collapses entirely while its `Check:` stands. (unresolved, needs 2b's
  ring-collapse ordering)

### client-a-a-close-completes-before-its-setup-goodbye-is-written

Type: reachability
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - nothing constructs the ordering, but the thread's *exit* is
observed in CI: `tests/shm_soak.rs:54-110` and
`tests/shm_failure_modes.rs:193-228` poll until the process thread count returns
to a post-close baseline after real `Client::connect`/`close` cycles, so a bridge
thread that never left its loop would fail those assertions
Guarantee: The window in which `close()` has returned `Ok` while the detached
bridge thread has not yet written its setup-socket Goodbye is genuinely
reachable, so a client's departure from the setup socket is not joined to its own
close, contrary to protocol `:691`'s "followed by joined ring teardown and
setup-socket close".
Check: `sometimes` - at least once per campaign, observe the joint state:
`close()` has returned, `join_tasks_until` reported both Tokio tasks joined, and
the bridge thread has not yet executed `client.rs:1955`. `sometimes` rather than
`reachable` because the lines at `:1954-1957` are executed on essentially every
shutdown; what must be produced is the operational *ordering* in which the owner
outruns them, and location coverage cannot witness that.
Fault/timing angle: The whole record. `close` cancels at `:711`, joins only
writer and reader at `:1682`, and returns. The bridge thread observes
`cancel.is_cancelled()` at `:1866` only at the top of its next iteration, after
up to a 50-microsecond sleep (`:1886`) or a full in-flight ring write.
Required faults and enabling state: Independent preconditions, per the
coverage-check rule: (a) `close()` observed returning with
`within_deadline == true`; (b) the bridge thread observed still inside its loop
body or its sleep at that moment. Assert both, and assert nothing about how the
host classified the departure.
Confidence: high - [evidence](evidence/client-a-a-close-completes-before-its-setup-goodbye-is-written.md).
Verified `join_tasks_until` (`:1677-1695`) iterates only
`[&self.writer, &self.reader]` and that the spawn at `:1852` discards its handle.
**The original record's consequence was withdrawn during disposition and is not a
premise of anything above.** It claimed a clean close is observed by the host as
an abrupt EOF. It is not, for two independent reasons: `close` sends a ring
channel-0 `Goodbye` through `send_control_wait` (`:702`) and that call returns
only after the writer's per-frame `completed_rx` resolved `Ok` and the `ack` fired
(`:1957-1971`), so the goodbye is published to the ring before `cancel.cancel()`
at `:711`; and the setup socket is moved into the thread closure at `:1854`, so
nothing closes it when `close` returns and the host sees no EOF at that instant.
The host's watcher is additionally a biased select on `peer_read_cancel`
(`connection.rs:196-198`), which the ring goodbye has already tripped.
Existing check: none constructs the ordering. Partial credit at the integration
layer for the thread's exit, in CI: `tests/shm_soak.rs:54-110` (`cycle` plus
`wait_for_envelope`) and `tests/shm_failure_modes.rs:193-228`
(`assert_resources_return_to` plus `clean_close_returns_exact_single_connection_capacity`
at `:218`), both run by `ci.yml:130-135`. They prove the thread terminates after
a real close; they observe neither the goodbye write nor the ordering. Status
`unaudited`.
Impact: Narrower than originally recorded and still real. The client's own
teardown contract is unjoined: `close` returns while an OS thread it spawned
still holds the setup socket, the ring attach, and the write-completion channel,
and the protocol requires that teardown be joined (`:691`). The consequence is a
contract gap and an unbounded-in-principle residency of one thread past a
successful `close`, not a peer-death miscount.
Open questions:
- Should `Inner` hold the bridge thread's `JoinHandle` so `close` can join it
  under the same 5-second budget? That budget is already shared with route
  teardown. (needs human input)
- Is there any path on which `close` returns `Ok` *without* the ring goodbye
  having been published, leaving the host's watcher armed? `close` returns `Err`
  on the `send_control_wait` timeout (`:706-709`) and cancels regardless, so the
  candidate is a `close` whose goodbye timed out; that path returns `Err`, so it
  does not satisfy this record's precondition (a). (unresolved, needs the
  shutdown-timeout path traced against the host's watcher)

---

## Group C: in-flight work, which the client gets right

Two records that currently hold, and both are premises rather than findings. The
first is the client's core replay-safety guarantee: every pending request is
settled exactly once with an outcome that is `NotSent` only when the bytes
provably never left. The second is that no request frame can carry a
non-increasing correlation, so a conforming host's per-generation watermark never
closes the generation on this client. Grouped because both are proved by the same
locking discipline inside `admit` (`client.rs:1119-1217`) and both are stated so
that a regression has something to violate.

### client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - `dropped_unary_future_cleans_pending_and_possibly_sent_request`
(`client.rs:3090`) and
`a_dropped_sender_after_an_absent_entry_reports_the_send_outcome` (`:3014`) cover
single-request classification, not bulk settlement on host death
Guarantee: When the host dies, every pending request is failed exactly once with
a send outcome that is `NotSent` only if its bytes provably never reached the
writer, and no pending request is silently dropped or retried.
Check: `always` - after any `retire`, the `pending` map is empty and, per pending
identity, exactly one settlement was delivered whose outcome is `NotSent` if and
only if `cancel_classification` (`client.rs:2276`) won the `QUEUED -> CANCELLED`
CAS. Per METHOD's effect-accounting rule the per-identity check is primary; the
cheap screen is that observed host-side effects lie between the count of
`NotSent` settlements subtracted from the total and the total. `always` because
it must hold at every retirement.
Fault/timing angle: The CAS at `:2225` races `claim_for_write` at `:1942`, which
is the writer's own `QUEUED -> WRITING` transition. A frame claimed by the writer
but not yet completed must classify `OutcomeUnknown`, which `classify` (`:2215`)
delivers by mapping both `WRITING` and `WRITTEN` there.
Required faults and enabling state: Kill the host with N pending requests
spanning all four publish states. Assert one settlement per identity and that no
`NotSent` claim was issued for a frame the host actually received.
Confidence: high - [evidence](evidence/client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome.md).
Verified `settle_all` drains under the `admission` mutex, that `finish_pending`
is the single settlement funnel, and that no retry path exists outside
`open_route`.
Existing check: `cancel_winning_queued_prevents_writer_claim_and_frame` (`:2478`)
and `writer_winning_cancel_is_outcome_unknown_and_queues_cancel` (`:2508`) cover
the CAS race for one request; status `unaudited`.
Impact: If a `NotSent` were ever issued for a delivered request, the caller would
replay a side-effecting operation. This is the client's core replay-safety
guarantee.
Open questions: None.

### client-a-no-request-frame-carries-a-non-increasing-correlation

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - `max_correlation_is_used_once_then_exhausted`
(`client.rs:2328`) and
`data_capacity_spares_control_reserve_and_does_not_burn_correlation` (`:3155`)
cover allocation and rewind in isolation
Guarantee: The sequence of `Request` correlations the client places on the wire
is strictly increasing, so a conforming host's per-generation watermark never
closes the generation on this client.
Check: `always` - for every pair of `Request` frames the writer completes in
order, the second correlation is strictly greater than the first, across control
(`0/0`) and routed identities alike, since both draw from one `Correlations`
(`client.rs:368`). `always` because the host evaluates it on every ingress frame
(`docs/host-wire-protocol.md:713`).
Fault/timing angle: Two windows. First, `admit` must not release the
`correlations` guard between allocation and enqueue; it does not (`:1176-1217`).
Second, `restore` must never rewind past a frame already handed to the writer;
its guard (`:1742-1744`) plus the fact that a failed `try_send` returns the frame
(`:1207`) prevents that.
Required faults and enabling state: Concurrent `request`, `request_stream`,
`open_route`, `host_status`, and `host_shutdown` callers, interleaved with encode
failures (oversize body) and `data_tx` saturation to drive both `restore` sites.
Record the correlation of each frame as the writer completes it.
Confidence: high - [evidence](evidence/client-a-no-request-frame-carries-a-non-increasing-correlation.md).
Verified guard scope by reading `admit` end to end, and verified both `restore`
call sites precede any delivery to `data_tx`. Part 2a's
`request-correlation-strictly-increases-per-generation` is the host-side
enforcement this satisfies.
Existing check: `max_correlation_is_used_once_then_exhausted` (`:2328`); status
`unaudited`.
Impact: A violation is a host-side generation close before dispatch
(`docs/host-wire-protocol.md:882`, vector V44), taking every unrelated route
down. I found no path that produces one.
Open questions: None.

---

## Group D: the liveness path and its two failures

Two records on the `Pong`, which is the client's only protocol obligation toward
host liveness. The first is that a `Pong` the client fails to enqueue takes the
whole generation down with a local admission code, because both surviving
enqueue-failure paths retire; the `Ping` arm binds the result to `_`, so the
teardown is never attributed to the probe. The second is that a `Pong` it does
enqueue can wait a full 30-second frame deadline, because the one thread that
would publish it may be parked delivering inbound frames. Grouped because both
make the same probe late for different reasons, and because in neither case does
anything name the probe as the thing that failed.

### client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - `a_ping_at_any_valid_priority_is_answered_with_an_exact_flag_echo`
(`client.rs:2754`) covers the success path, and
`control_exhaustion_retires_and_releases_all_queued_bytes` (`:3196`) drives the
retiring branch from a different caller
Guarantee: A `Pong` the client fails to enqueue is never merely dropped: both
enqueue failure paths retire the whole generation with
`control_capacity_exhausted`, so a missed host probe is reported as a local
admission fault rather than as a liveness event, and the `Ping` arm proceeds as
though the answer was sent.
Check: `always` - whenever `send_control` returns `Err` while called from the
`Ping` arm (`client.rs:1318`), `self.retired` is true afterwards, and the code
that produced it is `control_capacity_exhausted` from the charge branch
(`:1268-1275`) or the try-send branch (`:1283-1290`), never a code naming the
probe. The encode branch (`:1259-1265`) is excluded because it cannot run for a
`Pong`; see the confidence line. `always` because the disjunction over
`send_control`'s failure paths is total, and every path in it retires.
Fault/timing angle: None for the retirement itself, which is synchronous inside
the failing call. The consequence lands one layer up: the `let _ =` at `:1390`
discards the `Err`, so `dispatch` continues to the post-dispatch
`retired.load` check at `:1983` rather than attributing the teardown to the
probe it was answering.
Required faults and enabling state: Exhaust `control_budget` (`:399`, funded by
`CLIENT_CONTROL_QUEUED_BYTES` at `:76`) or fill `control_tx`, then deliver a
`Ping`. Assert the generation retired, that the code is
`control_capacity_exhausted`, and that no state names the unanswered probe.
Confidence: high - [evidence](evidence/client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault.md).
The original record's `always-or-unreached` encode branch is **impossible**, not
merely unresolved, which is what this disposition changed. `encode_owned_frame`
(`wire.rs:571-601`) returns `Err` only when `body.len() > MAX_BODY_LEN`
(`:577-583`), and the `Pong` call passes `Vec::new()` (`client.rs:1329`), so the
one branch that did not retire cannot be entered. Both surviving branches retire,
verified by printing `:1340-1361`.
Existing check: `a_ping_at_any_valid_priority_is_answered_with_an_exact_flag_echo`
(`:2754`) for the success path; `control_exhaustion_retires_and_releases_all_queued_bytes`
(`:3196`) reaches the retiring branch but not through the `Ping` arm. Status
`unaudited` for both.
Impact: Smaller than the original record claimed and differently shaped. The
client does not silently believe it is healthy: it tears the generation down.
What is lost is attribution, which routes this back into
`client-a-a-retired-generation-forgets-why-it-retired`: a caller sees
`control_capacity_exhausted` or, arriving later, a bare constant, and nothing
records that a host liveness probe went unanswered. Part 2a's
`a-timely-pong-sustains-the-generation-within-a-bounded-round` is the host-side
property, and the two ends disagree about what happened.
Open questions:
- Is escalating one unanswerable probe to a full-generation retirement the
  intended policy? The comment at `:1336-1339` argues the reserved-pool choice so
  that ordinary request traffic cannot cause it, which makes exhaustion a real
  local fault rather than load; that argues the escalation is deliberate. It does
  not establish that the caller should be unable to tell a probe failure from any
  other control-admission failure. (needs human input)

### client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget

Type: liveness
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test stalls inbound delivery and measures Pong egress
Guarantee: Once inbound delivery backpressures, an enqueued Pong waits on the
same bridge thread that is parked delivering inbound frames, and the client
tolerates that for the full 30-second frame deadline before reacting.
Check: `always` - with the inbound channel full and the bridge parked in `read_tx.blocking_send` (`client.rs:1905`), a control frame enqueued at `:1283` is not written until the bridge resumes or `timeout_at(frame.deadline, completed_rx)` (`:2031`) expires, where `frame.deadline` is `now + CLIENT_FRAME_TIMEOUT` (`:1281`, 30 s per `:45`); and then exactly one terminal outcome follows: if the bridge resumes with enough of the frame deadline left for the ring reservation (`endpoint.send` at `client.rs:1851` receives the same `frame.deadline`, handed over at `:2021`, so there is no independent send budget), the Pong's write completes within that frame deadline with the generation still live, and if the frame deadline expires first, whether at the writer's `timeout_at` (`:2031`) or inside the ring reservation against the same instant, the client retires the generation with `write_failed` (`:2025`, `:2034`); a bridge that resumes with too little budget left falls into the second outcome, not a false failure of the first. The bound is one frame deadline in the unit the code bounds, not "eventually". `always` because the dependency and its terminal outcome hold on every control write once the precondition is met.
Fault/timing angle: The bridge thread is the sole producer of write completions
(`:1872`) and the sole consumer of the write channel, so any inbound stall is
also an egress stall. This is the client-side mirror of Part 2b's
`ring-a-ingress-wait-holds-a-lease-while-servicing-egress`.
Required faults and enabling state: Bounded fault-free window, per METHOD's
liveness rule. Stall `ring_reader_loop` so the 256-slot inbound channel (`:1850`)
fills, enqueue a Pong, release the stall, then poll until the write completes
within an explicit bound of one frame deadline.
Confidence: high - [evidence](evidence/client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget.md).
Verified the bridge is the single completion producer, that `writer_loop` awaits
it before dequeuing the next frame, and that `RingClientEndpoint::send`'s own
bound is a hardcoded 2 s (`ring_transport.rs:663-667`) that ignores the frame
deadline.
Existing check: `data_saturation_never_starves_a_control_frame` (`:3225`) covers
queue-slot starvation, which is a different mechanism; status `unaudited`.
Impact: Whether the host retires the generation first depends on its probe
interval against 30 seconds. If the probe is shorter, an inbound stall presents
to the operator as a liveness failure rather than as backpressure.
Open questions:
- What is the host's probe interval and deadline? Part 2a owns the liveness
  probe; the comparison against `CLIENT_FRAME_TIMEOUT` needs that number.
  (unresolved, needs the 2a figure)

---

## Group E: the route cache, unbounded and collapsing

Two records on the one `Inner` collection with no capacity predicate. The first
is the missing bound, which the normative document names explicitly. The second
is the identity collapse: because the cache is a `HashSet`, a host that answers
two `route.open` requests with one `(channel, epoch)` merges two callers onto one
entry, and the cleanup path that would normally release a stray bind is the exact
path that returns early. Grouped because both are properties of
`routes.insert(handle)` at `client.rs:507`, one about how many entries it admits
and one about what an entry means.

### client-a-live-route-handles-are-bounded-only-by-the-host

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test opens routes to exhaustion
Guarantee: The client imposes no limit on concurrently live route handles, so the
only bound on its route cache is the host's willingness to keep binding.
Check: `always` - every successful `open_route` inserts into `routes` at
`client.rs:477` with no capacity predicate anywhere on that path, in contrast to
`pending` (`:1118`) and `streams` (`:1009`). `always` because the absence is a
total property of the insert path.
Fault/timing angle: None. The growth is caller-driven, not race-driven.
Required faults and enabling state: A host that binds every `route.open`. Open
routes in a loop without closing and observe `routes` growth against the absent
cap.
Confidence: high - [evidence](evidence/client-a-live-route-handles-are-bounded-only-by-the-host.md).
Verified only two `CLIENT_MAX_*` constants exist (`:53`, `:55`) and that neither
is consulted at `:507`. Contract side at
`docs/host-wire-protocol.md:658`, which names routes in its finite-limits
list.
Existing check: none.
Impact: Unbounded caller-driven growth with no local reaper is the recurring
shape this catalog has found in every part. Here the damage is transitive: each
entry corresponds to a host channel and route permit, so a looping caller
exhausts host resources rather than its own.
Open questions:
- Does the host cap concurrent routes per generation, and does it answer
  `target_unavailable` on exhaustion as `docs/host-wire-protocol.md:658`
  implies? If so the transitive bound is real, though undeclared on this side.
  (unresolved, needs the 2e or 2f route-admission figure)

### client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - `a_duplicate_bind_terminal_never_closes_an_owned_route`
(`client.rs:3587`) covers the unmatched-terminal case, not two successful opens
returning one handle
Guarantee: If the host answers two `route.open` requests with the same
`(channel, epoch)`, the client conflates them into one cache entry, and one
`close_route` settles both callers' work while neither bind is separately
released.
Check: `always` - whenever `parse_route_open` yields a handle already present in `routes`, `routes.insert` (`client.rs:477`) returns `false` and the set is unchanged, so `settle_route` (`:1525`) can remove it at most once and `release_stranded_route` returns early at `:1485-1487`; and with one pending request outstanding per caller, one `close_route` settles both callers' pending requests with a classified outcome and the host observes at most one release. `always` because the set semantics and the settlement hold on every duplicate bind.
Fault/timing angle: None required, but the damage compounds if the two opens
overlap: the second caller receives `Ok(handle)` for a route the first caller can
close underneath it.
Required faults and enabling state: A host, or a fake peer, that answers two
distinct `route.open` correlations with an identical `route_channel` and
`route_epoch`. Assert both callers received `Ok`, that `routes.len() == 1`, and
that one `close_route` settles both callers' pending requests.
Confidence: high - [evidence](evidence/client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle.md).
Verified `routes` is a `HashSet<RouteHandle>` (`:944`), that `parse_route_open`
(`:2167-2206`) validates only shape, and that the early return at `:1576` is the
intended behaviour for the §8.2 case and therefore blocks cleanup here too.
Existing check: `a_duplicate_bind_terminal_never_closes_an_owned_route`
(`:3587`); status `unaudited`.
Impact: A host bug or a hostile peer at the setup path turns into cross-caller
interference inside one client: caller A's `close_route` silently settles caller
B's requests with `route_gone`. Part 2c established that epochs are host-minted
and that the activation token cannot gate mapping, so the client has no
independent basis to reject a repeated handle.
Open questions:
- Should `open_route` retire on a duplicate handle, the way it already retires on
  an unparseable one (`:486`)? Both are host protocol violations the client
  cannot name a remedy for. (needs human input)

---

## Group F: a host answer taken as proof

Two records where the client converts a host message into a belief about host
state it never verifies. `host_shutdown` returns `Ok` on a JSON echo of its own
operation name, and its doc comment declares that `Ok` the stop linearization
point a lifecycle owner waits on. `open_route` retries after four terminal codes
on the premise that each proves no route was bound. Grouped because both are
trust decisions rather than mechanisms, and because in both cases the fact the
client needs lives on the host side. **One of the two host-side facts is now
resolved:** this disposition read `dispatch.rs:1177-1238` and established that no
current host exit both installs a bind and answers with an error, so the retry
record's dependency holds today and the record is restated as a coupling plus a
dead allowlist entry rather than as a suspected defect. The `host_shutdown` echo's
host-side fact remains open.

### client-a-host-shutdown-success-rests-only-on-a-json-echo

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test supplies a well-formed echo from a host that did not
stop
Guarantee: `host_shutdown` returns `Ok` on the strength of a response body
echoing its own operation name, and nothing in the client verifies the host
actually stopped.
Check: `always` - `host_shutdown` (`client.rs:576-615`) returns `Ok(())` if and
only if the response body parses as JSON with `op == "host.shutdown"`
(`:598-606`); no other host state is consulted, and the connection is left open
by design (`:575`). `always` because the acceptance predicate is total over
responses.
Fault/timing angle: None inside the client. The window that matters is between
the host writing the response and the host actually stopping, which the doc's
shutdown ordering places at steps 3 through 9
(`docs/host-wire-protocol.md`, section 12).
Required faults and enabling state: A fake peer that answers
`{"op":"host.shutdown"}` and then continues serving. Assert `host_shutdown`
returns `Ok` and that the caller's next operation still succeeds, which is the
observable form of "the stop was not real".
Confidence: high - [evidence](evidence/client-a-host-shutdown-success-rests-only-on-a-json-echo.md).
Verified the predicate, and verified that the `Ok` is load-bearing for a
downstream owner because the doc comment at `:575` declares it "the stop
linearization point the native lifecycle owner waits on".
Existing check: none found for `host_shutdown` in `client.rs`'s test module.
Impact: This is the shape a sibling part found on a producer that advanced a
durable checkpoint on an acknowledgement truthful about nothing. Here the
acknowledgement gates a lifecycle owner's belief that a daemon stopped, which is
the precondition for starting a replacement. A stale echo could produce two live
daemons.
Open questions:
- Does the host emit the `host.shutdown` response strictly after its stop is
  committed, as `:575` claims? That is a 2a or 2e claim about the host's control
  handler and is not verifiable from `client.rs`. (unresolved, needs the
  host-side handler)
- Does any caller of `host_shutdown` treat `Ok` as authority to launch a
  replacement daemon? `crates/daemon/src/bin/eidnara-host.rs` is the likely site
  and is outside this sub-part's scope. (unresolved, needs 2f or a daemon
  pass)

### client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: not yet - no test counts host-side binds across a retried `open_route`
Guarantee: `open_route` retries after four specific host terminal codes on the
premise that each proves no route was bound, and the client verifies nothing:
today the premise holds only because of an ordering discipline on the host side
that this client neither checks nor is told about, and one of the four codes has
no producer in the tree at all.
Check: `always` - for a sequence of `open_route` attempts ending in success, the
number of routes the host bound for that call is exactly one. Per METHOD's
effect-accounting rule, track attempted and acknowledged separately: attempts
equal loop iterations at `client.rs:435`, acknowledged failures equal the retried
terminals at `:481-490`, and host-side binds must equal one, not the attempt
count. The aggregate bound is the cheap screen; the per-attempt check is the
oracle. `always` because it must hold for every `open_route` call, not merely be
witnessed once.
Fault/timing angle: The retry is gated on `outcome == Terminal` (`:512`), so an
`OutcomeUnknown` never retries. **Restated during disposition: the window the
original record described has no producer in the current host.** The bind path is
`dispatch.rs:1177-1238` and it has no exit that both leaves a route installed and
answers with an error. `BindOutcome::Accept` plus `BindInstall::Installed`
installs the bind and emits the `route.open` success response (`:1178-1193`).
`BindInstall::CloseWins` publishes nothing at all (`:1195-1202`).
`BindOutcome::Reject` calls `shared.registry.take_rejected_bind(handle)`
(`:1219`), which cancels the occupant and marks it `Closing`
(`routing.rs:191-205`), and only then runs route-gone and emits the error terminal
(`:1220-1236`). The stopped-callback arm does the same and emits nothing
(`:1164-1170`). And the three codes a host can actually produce are emitted before
any bind exists or after it is cleaned: `unknown_module` and `target_unavailable`
are pre-bind classification (`control.rs:15-16`, with capacity exhaustion
documented "without any handler bind" at `routing.rs:112`), and
`module_reloading` is a handler bind rejection (`synapse/mod.rs:960-963`) that
takes the `Reject` arm. `module_timeout`, the code the original record's recipe
was built on, appears nowhere in the tree outside this client's own allowlist
(`client.rs:518`), verified by grep.
Required faults and enabling state: Two forms, and the second is the one that
survives. **(a) The verifiable form, no fault:** enumerate the host's bind exits
and assert that every terminal-carrying exit is either pre-bind or preceded by
`take_rejected_bind`, and that every allowlisted code has such an exit or no
producer. **(b) The original form, now known to need a non-conforming peer:** a
fake peer that answers `route.open` with `Error{code:"module_timeout"}` and also
binds a route, then counting host-side binds against client-side handles and
checking `release_stranded_route` (`:1572`) for reclamation. Form (b) tests the
client against a host protocol violation, which is the framing question a human
must settle; see [portfolio-evaluation.md](client-peer/portfolio-evaluation.md).
Confidence: high - [evidence](evidence/client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind.md).
Raised from medium during disposition, because the question the original record
could not resolve is now resolved against the current host: the retry is safe
today, and the reason is `take_rejected_bind` running before
`emit_error_terminal`. The retry predicate and the fresh-correlation-per-attempt
behaviour were already verified. What is *not* resolved is whether the client
should depend on that ordering, which is a contract question rather than a code
one.
Existing check: `an_abandoned_control_open_releases_a_late_bound_route` (`:3503`)
covers the late-bind remedy that would mitigate a violating host; status
`unaudited`. Nothing checks the host-side ordering this record now rests on, and
nothing flags `module_timeout` as an allowlist entry with no producer.
Impact: Reframed and smaller, with one durable finding inside it. The client's
retry safety is a cross-part coupling: it is not derived from anything in
`client.rs`, and a future host that emitted a retried code after installing a bind
would strand a route and channel permit per retry, bounded only by the 30-second
route-open deadline divided by the backoff. The concrete defect that survives on
this side is the dead allowlist entry: `module_timeout` can only ever be produced
by a peer that is not this host, so the client retries on a code its own host
never sends.
Open questions:
- Should the client's retry allowlist be derived from, or checked against, the
  host's emitted code set? They are independent literals today, and one of the
  four has no producer. (needs human input)
- Is `module_timeout` reserved for a host version not in this tree, or is it
  vestigial? `docs/host-wire-protocol.md:658` gives each code "exactly one
  recovery rule in Section 10.2" but does not say which codes a host must be able
  to emit. (unresolved, needs the 2e or 2f control-vocabulary pass)

---

## Group G: inbound strictness and its unreachable twin

Two records on `validate_inbound` (`client.rs:2006-2082`) and the `dispatch`
catch-all behind it. The first is that the validator is stricter than the
document for exactly one frame type, `Cancel`, and retires the whole generation
for it. The second is the consequence: because the validator rejects every type
`dispatch`'s catch-all would handle, that catch-all is unreachable from the
production reader, and the classification is duplicated at two sites that can
drift. Grouped because they are the same `match` residue read from two
directions, and because this synthesis's correction above bears on both.

### client-a-a-host-originated-cancel-retires-the-generation

Type: safety
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - `inbound_validation_enforces_the_direct_profile_table`
(`client.rs:2658`) exercises `validate_inbound` broadly; whether it asserts the
`Cancel` disposition is unverified
Guarantee: The client treats a host-originated `Cancel` as a framing violation
that retires the whole generation, although the protocol's role table does not
list host-originated `Cancel` as role-invalid and assigns `Cancel` an idempotent
no-op disposition.
Check: `always` - for `header.ty == FrameType::Cancel` (`wire.rs:53`),
`validate_inbound` (`client.rs:2073`) has no matching arm and falls to
`_ => return Err(())` at `:2121`, so `ring_reader_loop:2050` retires with
`protocol_violation`. `always` because the classification is total over inbound
frame types.
Fault/timing angle: None.
Required faults and enabling state: A fake peer that sends a well-formed
pure-header `Cancel` on a live route with a pending correlation. Assert the
client retires rather than treating it as a no-op.
Confidence: high - [evidence](evidence/client-a-a-host-originated-cancel-retires-the-generation.md).
Verified `validate_inbound`'s arms are exactly `Response|Error`,
`StreamData|StreamEnd`, `Push`, `Ping`, `Goodbye`, plus the catch-all, and that
`Cancel` is therefore in the residue. Contract side at
`docs/host-wire-protocol.md:269` and `:280`.
Existing check: `inbound_validation_enforces_the_direct_profile_table` (`:2658`);
status `unaudited`, and its coverage of `Cancel` specifically is unverified.
Impact: If a host ever emits `Cancel`, every route on the generation dies. If a
host never does, the strictness is free and the finding is a documentation defect
rather than a code defect. Which of those holds is the open question.
Open questions:
- Is host-originated `Cancel` legal in this profile?
  `docs/host-wire-protocol.md:269` enumerates role-invalid frames and omits
  `Cancel`, while `:280` gives `Cancel` a no-op disposition without naming a
  direction. The doc is ambiguous and the code is strict. (needs human input)

> Synthesis note, resolving this record's `Existing check:` caveat rather than
> editing it. The coverage is now verified and the answer is no.
> `inbound_validation_enforces_the_direct_profile_table` runs `:2658-2751`, and
> grepping that body for `Cancel`, `Request`, `Pong`, `Hello`, and `HelloAck`
> returns exactly one hit, `FrameType::Request` at `:2750`. So the test asserts
> one of the five residue types and says nothing about `Cancel`. The record's
> `Exercised: partial` remains correct as written; what changes is that the
> uncertainty is closed against the pessimistic reading.
>
> A second count correction, carried here rather than edited into the next
> record. Lens A's
> [client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production](#client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production)
> says the test module reaches `dispatch` at 16 sites. Grepping `dispatch(` from
> `:2266` onward returns **15**. The finding is unaffected, since the record's
> claim is that the production reader cannot reach `:1557` while the test module
> can; only the count moves. The 15 sites are listed in that record's evidence
> file.

### client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production

Type: reachability
Reachability: test-only - `Client::connect` (`crates/host-runtime/src/client.rs:306`) has no caller outside `crates/host-runtime` tests and benches in this tree; the daemon and historian consumers the source catalog cited are scheduled for U4 (`docs/properties/README.md:52`). The path carries no `cfg` gate and is reached by every test client, so reclassify to `default-production` in the wave that lands a production caller.
Status: active
Exercised: partial - reached only by the test module's 16 direct `dispatch` calls
Guarantee: `dispatch`'s catch-all retirement arm is unreachable from the
production reader, because `validate_inbound` already rejects every frame type
that would land there.
Check: `unreachable` - the statement at `client.rs:1468` (the `_ => self.retire("protocol_violation")` arm of `dispatch`; `:1557` is `settle_all`'s `cancel_classification` call, which normal retirement with pending work executes) is never executed on the
`ring_reader_loop` path. `unreachable` rather than `always(!X)` because the
subject is a specific code location that must not execute, which is exactly
METHOD's criterion.
Fault/timing angle: None.
Required faults and enabling state: No fault. The check is a marker at `:1468`
that must not fire during any production-path campaign, combined with the
independent observation that `validate_inbound` returned `Err` for the same frame
types.
Confidence: high - [evidence](evidence/client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production.md).
Verified that `dispatch` handles `Ping`, `Goodbye`, `Push`,
`Response|Error|StreamEnd`, and `StreamData`, that its catch-all therefore covers
`Request`, `Cancel`, `Pong`, `Hello`, and `HelloAck` (`wire.rs:52-63`), and that
`validate_inbound:2067` rejects all five. Confirmed `dispatch`'s only non-test
caller is `:1982`.
Existing check: none as a guard. The 16 test call sites listed in the evidence
file reach `dispatch` directly, bypassing validation.
Impact: Low on its own. It matters as a structural fact: the tests exercise a
dispatch surface the production reader cannot reach, so a regression that
loosened `validate_inbound` would be caught by nothing, and the duplicated
classification at `:1557` and `:2067` can drift.
Open questions: None.

---

## Relationship map

Grouped by shared mechanism rather than by the headings above, because the
sharpest relationships cross groups. **Every dominance statement below is a
hypothesis** about which oracle subsumes which, offered to order the work, not a
verified claim. None has been tested, and none can be tested by anything CI runs
today with one partial exception recorded during disposition: this sub-part has
zero CI-executed source-resident checks, its six CI-executed `tests/client.rs`
tests touch none of these records directly, and the thread-count assertions in
`tests/shm_soak.rs` and `tests/shm_failure_modes.rs` reach only the *termination*
half of the close-ordering record, never its ordering.
- **One erased cause, read from four sides.**
  [client-a-a-retired-generation-forgets-why-it-retired](#client-a-a-retired-generation-forgets-why-it-retired),
  [client-a-a-clean-host-close-and-a-transport-failure-share-one-code](#client-a-a-clean-host-close-and-a-transport-failure-share-one-code),
  [client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault](#client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault),
  [client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye](#client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye).
  All four are the same defect at different layers. `retire` keeps no cause
  (`client.rs:1667-1675`); five bridge exits produce one code (`:1987`); a failed
  `Pong` enqueue produces `control_capacity_exhausted`, which names the pool and
  not the probe (`:1341`, `:1356`, discarded at `:1390`); and the departure signal
  the host reads carries no cause either (`:1890-1893`). Hypothesis: storing the
  retirement cause on `Inner` and surfacing it through `CallError`
  *dominates* the first two, because each of their oracles reduces to "a
  post-retirement caller can name the cause". **This disposition strengthened its
  relation to the third**: because a failed `Pong` enqueue now provably retires
  rather than passing silently, storing the cause dominates that record's
  attribution half too, though not its policy question. It dominates the
  setup-socket record not at all, since that signal is read by the host rather
  than by a caller. Fixing the bridge's five exits to carry distinct codes without
  storing the cause dominates nothing, because the distinction would still be
  visible only inside `settle_all`'s loop.
- **One unjoined thread, three claims.**
  [client-a-a-close-completes-before-its-setup-goodbye-is-written](#client-a-a-close-completes-before-its-setup-goodbye-is-written),
  [client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye](#client-a-a-ring-failure-departs-the-setup-socket-as-a-clean-goodbye),
  [client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget](#client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget).
  Every one of these turns on the bridge thread spawned at `:1852` with its
  handle discarded at `:1895`. It owns the ring attach (`:1855`), the sole write
  completion (`:1872`), and the departure write (`:1891`), and nothing observes
  any of the three. Hypothesis: retaining the `JoinHandle` and joining it under
  the existing 5-second shutdown budget *dominates* the close-ordering record
  outright, since the window it describes is exactly what a join closes. It
  dominates the ring-failure record only halfway: joining proves the write was
  attempted but not that its content distinguished the cause. It dominates the
  Pong-egress record not at all, because that record is a bound on time under
  backpressure and a join says nothing about it.
- **The route cache as one entry point.**
  [client-a-live-route-handles-are-bounded-only-by-the-host](#client-a-live-route-handles-are-bounded-only-by-the-host),
  [client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle](#client-a-a-duplicate-host-bind-collapses-two-routes-into-one-handle),
  [client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind](#client-a-route-open-retries-treat-four-host-terminals-as-proof-of-no-bind).
  All three land on `routes.insert(handle)` (`:507`) and on what the host is
  trusted to have done before it. Hypothesis: adding a `CLIENT_MAX_LIVE_ROUTES`
  predicate at `:507` dominates the first record and *nothing else*, which is
  worth stating because the three read as one cluster. A cap does not change
  `HashSet` merge semantics and does not make a retried `route.open` prove
  anything. Conversely, replacing the `HashSet` with a map keyed by the caller's
  identity would dominate the duplicate-bind record and give the retry record its
  missing accounting, because both need to distinguish two binds that currently
  hash equal.
- **Two premises that hold, stated so a regression has something to break.**
  [client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome](#client-a-every-in-flight-request-is-settled-with-a-classified-send-outcome),
  [client-a-no-request-frame-carries-a-non-increasing-correlation](#client-a-no-request-frame-carries-a-non-increasing-correlation).
  Both are proved by the same locking discipline: `admit` holds the
  `correlations`, `admission`, and `pending` guards across allocation, encoding,
  and enqueue (`:1140-1141`, `:1176-1217`), and `settle_all` takes the whole
  pending map under `admission` (`:1651-1652`). Hypothesis: an oracle that
  records the correlation of every frame the writer completes, alongside the
  settlement each pending identity received, *dominates both*, because the
  monotonicity claim and the exactly-once-settlement claim are two readings of
  the same trace. Nothing dominates them separately, which is the argument for
  building that trace once rather than two fixtures.
- **Classification duplicated at two sites.**
  [client-a-a-host-originated-cancel-retires-the-generation](#client-a-a-host-originated-cancel-retires-the-generation),
  [client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production](#client-a-the-unmatched-inbound-frame-arm-is-never-entered-in-production).
  `validate_inbound:2067` and `dispatch:1557` both classify the same five frame
  types, and only the first is reachable from the reader. Hypothesis: a single
  table-driven classifier consulted by both sites would dominate the second
  record by construction, since the arm could not drift out of agreement. It
  dominates the first not at all: whether `Cancel` belongs in the residue is a
  contract question that no refactor answers, and the document is ambiguous
  (`docs/host-wire-protocol.md:269` versus `:280`).


## Sub-part 2e catalog: admission, dispatch, and the response obligation

Scope: what admits a request, what guarantees it gets a response, and what
happens when it does not. Five files, 4,546 lines, all re-derived with `wc -l`
at `HEAD`: `crates/host-runtime/src/dispatch.rs` (1,539), `control.rs` (1,180),
`routing.rs` (833), `handler.rs` (604), `composite.rs` (390).

The production and test halves matter here, because the file that decides every
terminal is almost all production. `dispatch.rs` production occupies `1-1497`
and its `#[cfg(test)] mod tests` occupies `1498-1539`, which is 42 lines,
2.7 percent of the file. `control.rs` production is `1-709` and its tests
`710-1180`. `routing.rs` production is `1-453` and its tests `454-833`.
`handler.rs` and `composite.rs` have no test module at all.

Boundary context, read but not cataloged: `connection.rs` is Part 2a's file and
is cited as the caller boundary only, for `read_loop` (`:373`), the two
dispatch entry points (`:462`, `:467`), and the three rejection bounds.
`ring_transport.rs`, `wire.rs`, and `frame_channel.rs` are Part 2b's and are
cited for the publication contract only. `client.rs` is Part 2d's.
`runtime.rs` and `config.rs` are sub-part 2f's and are cited for pool
construction and the deadline vocabulary.

**This is a post-refactor surface.** The request path survived the refactor with
its comments intact, which is itself a finding: grepping all five files for
`tcp_frame_channel`, `transport_negotiation`, `transport_provider`,
`provider_recovery`, `frame_read`, `shm_provider`, `negotiate`, `Serveable`,
and `fallback` returns zero hits, so **no source comment in this sub-part
describes a deleted mechanism**. The path never named the transport it sat on.
Four commits carry the refactor:

| Commit | Subject |
| --- | --- |
| `0f336d3c` | `refactor(shm): collapse to fixed ring transport` |
| `d8bde128` | `feat(host): add authenticated ring setup socket` |
| `793a973e` | `build(shm): require packaged native transport` |
| `ed487e11` | `refactor(host): make ring transport mandatory` |

The one deleted-mechanism finding here is on the other side of the boundary, in
the normative document, and it is the sharpest disagreement in the sub-part:
see the fifth lead below.

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). System
`the `host` source checkout, branch
`feat/shared-memory-release-gate-audit`, `HEAD` = `e447c927`, confirmed with
`git log -1`. Both lens agents read and verified their line references at that
commit. Scope and CI findings come from
`part-2-rescope/scope-map-and-risk-ranking.md` (a source-tree artifact that was not migrated into this repository).

**Where lens B re-derived a citation lens A made, lens B's line numbers win.**
Four differences, all verified again by this synthesis by printing the lines,
and none changes a finding.

- Busy-reject exhaustion cancels at `dispatch.rs:637` and discards at `:638`.
  Lens A cited `:629` (the bound check) and `:638`. Both halves are real; the
  pair that produces the blast radius is `:637-638`, printed and confirmed as
  `gen.token.cancel();` then `gen.writer.discard();`.
- The `BindInstall::CloseWins` silent exit is `dispatch.rs:1195-1202` as a
  block, and the line that runs instead of a terminal is `:1199`
  (`run_route_gone`). Lens B's `:1199` is the precise site and is used below.
- `dispatch.rs` is 1,539 lines but decides every terminal in 1,497 production
  lines. Lens A cited the file total; lens B the production half. Both stated
  above.
- **Lens A's heading count of CI-named test binaries was wrong, and its
  `Not in CI` conclusion was wrong too.** Lens B enumerated six binaries whose
  subject is the request path and found none named in `ci.yml`; that is true
  and irrelevant, because `ci.yml:118` and `:126` run
  `cargo test --workspace --all-targets` on both toolchains, which builds and
  runs every integration binary in the workspace without naming it. The
  per-record `Existing check:` lines in this group therefore read "runs in CI"
  rather than the source catalog's `Not in CI`, and the `Exercised:` fields
  are classified on that basis.

## What this part is about

Six facts frame every record here. The first three are the reason this sub-part
was cataloged. The fourth is the bound that exists and the deadline that does
not. The fifth is a contract disagreement that a compiler enforces. The sixth is
the coverage position, which is the one place 2e beats both its siblings.

**An admitted routed request gets at most one terminal, not exactly one.** The
arbiter is sound: `Settlement` (`dispatch.rs:34-59`) is three fields, `won` is
mutated only by the `swap(true)` at `:408`, that swap happens under the async
`order` mutex (`:407-410`), and every emission site takes the same lock, so a
stream item can never follow a terminal and exactly one claimant wins among
handler completion, cancellation, route close, and generation teardown. The whole
arbiter is `settle` at `:399-500`, and its first two statements are the lock and
the swap. What is missing is the other half of exactly-once. **Five exits leave
work with no terminal at all.** Each was verified individually, and **this
disposition classified them, because they are not all about the same thing and the
original list read as though they were.** Exactly one concerns an admitted routed
request's settlement; one is pre-dispatch, before any settlement exists; and three
are control-channel `route.open` exits.

1. `dispatch.rs:1058` - the non-panic join-error arm, and **the only one of the
   five about admitted routed settlement**. `:1053` catches
   `join_err.is_panic()` and emits `internal_error`; the `Err(_)` arm at `:1058`
   removes the pending entry and returns *before* the `settle` call at `:1063`,
   so an aborted handler task settles nothing. Printed and confirmed as
   `Err(_) => { remove_pending(&gen_task, key); return; }`. **No record in this
   catalog asserts the silence here.** The pending-entry record covers the
   `remove_pending` at `:1059`, which is the entry's removal, not the missing
   terminal; see the gaps queued in
   [portfolio-evaluation.md](request-path/portfolio-evaluation.md).
2. `dispatch.rs:637-638` - busy-reject exhaustion, which is **pre-dispatch**: the
   rejection never became an admitted request and no `Settlement` exists for it.
   Past the per-generation `MAX_INFLIGHT_BUSY_REJECTS` of 32
   (`connection.rs:42`, used at `:244`) the code cancels the token and calls
   `gen.writer.discard()`, which drops **other correlations' already queued
   terminals**, not just the rejection that could not be emitted. This is the
   worst of the five by blast radius, and its blast radius is precisely the reason
   it is not confined to the pre-dispatch request that triggered it. The comment
   at `:630-636` argues the trade honestly and names the outcome, so it is a
   declared cost; nothing checks that the declared cost is the one that occurs.
3. `dispatch.rs:1164` - **a control `route.open` exit**: a bind callback that
   stopped, by panic, abort, or its own inner deadline. Route-gone still runs
   exactly once; no `Error` is emitted.
4. `dispatch.rs:1174` - **a control `route.open` exit**: a bind callback still
   executing at the lifecycle deadline. The fatal latch is already tripped, so
   the incarnation terminates and a terminal would be pointless.
5. `dispatch.rs:1199` - **a control `route.open` exit**: `BindInstall::CloseWins`,
   a close that raced the bind. Route-gone runs; no terminal.

So the at-most-one guarantee is satisfied by all five, each by emitting zero, but
they do not share a subject. Three of the five are `open_route` exits, out of
seven total (`dispatch.rs:1103-1239`), which makes the control path's worst case
worse than the routed path's. Protocol `:692` covers the retirement cases: "Any
published request lacking an observed terminal at close is `outcome_unknown`". It
does not cover `:1164`, `:1174`, or `:1199`, where the connection stays live and
the `route.open` correlation simply never settles until the caller's own
30-second route deadline expires.

**An empty success is accepted end to end, and nothing below dispatch can reject
it.** The whole success gate for a unary response is `dispatch.rs:1031-1033`,
printed and confirmed:

```
Ok(RequestOutcome::Response { body, binary })
    if body.len() <= crate::wire::MAX_BODY_LEN as usize => {
        Terminal::Response { body, binary }
    }
```

The predicate is an upper bound only, and `0 <= MAX_BODY_LEN` holds. A handler
that reserves **owned** output through `reserve_output` (`handler.rs:466`), fails
partway, and returns the buffer it never wrote into emits a **zero-length
`Response` terminal**. `OutputBuffer::len()` (`handler.rs:361-366`) returns the
*written* `body.len()` for an owned buffer and the *declared* `direct.len` for a
direct one, and `extend_from_slice` and `resize` (`:381-396`) both refuse to grow
past `max_len` and both refuse outright when `direct.is_some()`, so a reserved and
unwritten owned buffer is a supported, silent state. The wire layer accepts the
result: `wire.rs:340` rejects a body only on a pure-header type
(`if ty.is_pure_header() && len != 0`, printed and confirmed), and `Response` is
not pure-header (`:86-88`), so no lower bound on a declared `Response` length
exists anywhere in decode. Neither does the Rust client impose one:
`validate_inbound`'s `Response | Error` arm (`client.rs:2022-2031`) checks
`corr != 0` and the binary-flag-on-channel-0 rule and nothing about length. The
adjacent arms show the author reasoning carefully about other shapes in the same
match - `:1020` catches a unary response after streaming, `:1035` catches an
oversize body. Emptiness is the gap.

**Two scope corrections, both applied during disposition.** First, this is
*empty-response acceptance*, not a handler failure presenting as a success. The
handler that returns `RequestOutcome::Response` has explicitly selected the
variant `handler.rs:220-225` documents as "Unary success"; nothing in the
observable state says a failure occurred, so calling it an error path is not
established, and whether an empty `Response` is a defect at all is an open
question this catalog cannot settle. Second, the *direct*-output form is not part
of this gap: a declared `exact_len` that the serializer never satisfies is caught,
at publication rather than at the gate, with `ProducerError::Underfill` - which is
[req-a-a-response-publication-failure-never-reaches-the-settling-path](#req-a-a-response-publication-failure-never-reaches-the-settling-path)'s
subject. The owned path is the one where declared and written are the same field
and zero is legal.

**Cross-part note, replacing an unverified ordinal.** The original text called
this "the fourth part in this catalog to find an error path presenting to its
caller as a success" and conceded in the same paragraph that the count was
inherited and never re-derived. The ordinal is **removed** rather than confirmed,
for three reasons. METHOD rule 2 forbids clearing an open question by assertion,
and the catalog had already marked this one unverified. After the narrowing above,
the 2e instance is not an error path at all, so it cannot be the fourth of
anything. And the sites the ordinal grouped do not share an oracle: Part 4c's and
4d's are write paths that report success without persisting, whose oracle is to
re-read the store after a successful response; 2d's `host_shutdown` accepts a JSON
echo of its own operation name, whose oracle is to keep serving after answering
and show the caller's next call succeeds; and this one is an empty body that every
layer accepts, whose oracle is a census of the gate. Part 4c's own disposition
made exactly this correction one layer up, removing a third site from an
equivalence for having a different oracle. Three mechanisms with three oracles are
worth a reader's attention as a recurring *shape*; they are not worth a count, and
a count is what made the claim unverifiable.

**Routed terminals carry no delivery acknowledgement, so acknowledged effects
are identically zero and only attempted are observable.** `settle` returns
`true` once `emit_reserved_frame` has *enqueued* the terminal
(`dispatch.rs:447-460`), and an `Err` from that call only cancels the generation
(`:458`). There is no ack frame, no write-completion callback, and no
per-correlation delivery record on the routed path. Only three emissions in the
whole sub-part carry a `written` hook, and all three are control or teardown
frames: `handle_host_shutdown` on both its paths (`:678-680`, `:743-756`),
`emit_authoritative_rejection` (`:814-816`), and `send_connection_goodbye`
(`:1474-1476`). Every routed terminal passes `written: None` (`:358`, and `:300`
when the caller supplies nothing).

The contrast inside the same file is what makes this a finding rather than an
observation: `dispatch.rs:646-651` describes the `CommitOnAck` hook where
"commit and host cancellation run inside the writer task at full-frame write
completion", and every earlier failure drops the hook unrun. The crate knows how
to condition an effect on delivery and does so for exactly one channel-0
operation. Per METHOD's effect-accounting rule the consequence is precise: on
the routed path the **acknowledged** count is identically zero, only
**attempted** is observable, per-identity oracles have no acknowledgement side
to use, and the `observed >= acknowledged` bound is vacuous here. Protocol
§10.1 makes an unobserved terminal `outcome_unknown` on the client side; the
host has no matching classification, so after a close the two ends cannot be
reconciled.

**Handler concurrency is bounded by four host-global semaphores with no
per-connection fairness, and no request deadline exists.** The four pools, with
construction at `runtime.rs:905-912` and defaults at `config.rs:131-132`:

| Bound | Value | Scope | Acquisition |
| --- | --- | --- | --- |
| `task_permits` | `max_handler_tasks` (default 256) minus reservations | host-global, general class | `try_acquire_owned` on the read loop |
| `reserved_task_permits` | 96 (Broca) | host-global, reserved class | same |
| `pending_permits` | `max_pending_requests` (default 1024) minus reservations | host-global, general class | same |
| `reserved_pending_permits` | 96 (Broca) | host-global, reserved class | same |

The acquisition discipline is the part that is unambiguously right and tested:
`try_acquire_owned` never waits, so the request is rejected pre-dispatch with
`server_busy` and zero handler invocation, and the comment at
`dispatch.rs:881-883` states the reason - acquiring inside the spawned task
would let a client pipeline unbounded tasks ahead of the gate
(`tests/dispatch.rs:295`, `:976`, `:1074`). The class comes from the route, not
the body: `route_tracker` returns `(TaskTracker, RouteClass)`
(`routing.rs:326-340`) and `dispatch.rs:873-879` selects the pool pair from it,
so the host never parses an application body to pick a class. The split into two
tasks per request is deliberate and load-bearing: the task permit lives in the
inner callback task (`:990`) so capacity frees the moment the handler returns,
while the pending permit lives in the outer settling task (`:933`) so it is held
across the egress wait.

What is absent is any bound on the handler itself. `HostTiming`
(`config.rs:199-218`) has seven fields and none of them bounds a request's
lifetime: `frame_deadline` bounds frames, `route_close_budget` applies only once
a close begins, and `lifecycle_callback_deadline` applies to `bind`,
`route_gone`, `initialize`, and `health`, never to `handle`. Protocol §11
assigns the 30-second request deadline to the *client*, so a client that dies
without sending `Cancel` leaves the host holding both permits indefinitely. And
because all four pools are host-global rather than per-generation, one
connection can consume every general slot: a module with a missing internal
timeout can hold all 256 general task permits, at which point every other
route's traffic gets `server_busy` while the host reports itself healthy.
Per-connection fairness is not a property of this layer, and lens A's own open
question records that no layer has been shown to supply it.

**A protocol statement names an API a `compile_fail` doctest now forbids.**
Protocol `:673` reads "Handler `Response(Vec<u8>)` becomes `Response`". The
current variant is `RequestOutcome::Response { body: OutputBuffer, binary }`
(`handler.rs:224`), `OutputBuffer`'s fields are all `pub(crate)` (`:332-335`),
and the doctest at `handler.rs:213-219` asserts that constructing
`RequestOutcome::Response { body: Vec::<u8>::new(), binary: false }` must fail
to compile. This is the sharpest disagreement in the sub-part because the two
sides are not merely unsynchronised, they are mechanically opposed: the document
describes a construction, and a check that **runs in CI** (`ci.yml:175`) fails
the build if that construction ever becomes possible. The mechanism the code
enforces is absent from the document - `OutputBuffer`, `reserve_output`, and
`output_from_writer` appear nowhere in `docs/host-wire-protocol.md`, grepped,
zero hits.

Lens B established by history rather than inference that this is a deleted
mechanism and not one that never existed:
`git log -S'body: Vec<u8>,' -- crates/host-runtime/src/handler.rs` returns `cf281ace`
(the commit that added `host-runtime`) and `ef66e349`, and
`git log -S'```compile_fail' -- crates/host-runtime/src/handler.rs` returns
`cf281ace` and `98b7270d`. The document line dates from `d0dbb25a` and has not
moved.

One residual of the opposite shape, recorded so a later pass does not miscount
it: the document names `Handler` at nine sites (`:43`, `:292`, `:596`, `:600`,
`:626`, `:634`, `:685`, `:800`, `:906`) and no such type exists in this crate.
That is a **forward** reference, not a stale one - `handler.rs:3` says
the source module-host work will adapt `Handler` onto this boundary, while the code
carries the boundary that exists, `HostHandler` (`:558`).

**Coverage: 37 in-crate and 84 integration tests, all executed by CI in this tree, plus 4
`compile_fail` doctests do run, so 2e owns 4 of the library's 6 CI-executed
source-resident checks.** The 37 in-crate tests are `control.rs` 23,
`routing.rs` 12, and `dispatch.rs` 2; `handler.rs` and `composite.rs` have
none. The 84 integration tests are spread over six binaries whose subject is
this sub-part - `tests/dispatch.rs` (20), `tests/composite_routing.rs` (16),
`tests/protocol_vectors.rs` (15), `tests/handler_contract.rs` (12),
`tests/routing.rs` (12), `tests/broca_protocol.rs` (9). The source repository's CI
named none of them; in this tree all six run under `ci.yml:118` and `:126`. The
source finding, 121 claim-bearing tests and zero executed by CI, is provenance here.

**CI in this tree.** `.github/workflows/ci.yml:118` and `:126` run
`cargo test --workspace --all-targets --all-features --locked` on the 1.98 and stable
toolchains, so every integration binary and every inline test this section counts
executes in CI. The named-versus-unnamed distinction and the `ci.yml` line numbers
below describe the source repository's workflow at authoring time and are kept as
provenance; they are not coverage gaps here.

**One correction to that framing, applied during disposition, and it is the only
CI-executed check on any record in this catalog.** "Zero executed by CI" is true
of the 121 tests in the five source files and six subject binaries. It is not true
of this sub-part's *record coverage*, because one record is asserted exactly by a
test in a binary CI does name.
`tests/lifecycle.rs:570-651` `shutdown_refuses_new_routes_and_new_routed_work`
drives a `route.open` and a routed request into one draining host and asserts
`target_unavailable` and `server_busy` respectively, which is
[req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes](#req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes)
in full; `lifecycle` runs at `ci.yml:168-169` on Linux. The former macOS run of
the same pair was removed with every other macOS job by PR #131 (merge
`5d638e3e8`); `ci.yml` at HEAD contains only `ubuntu-latest` jobs. The
binary was excluded from the six because its *subject* is the host lifecycle
rather than the request path, which is a defensible scope call and is exactly how
the check went uncredited. Counting by binary subject rather than by assertion is
what produced the error.

The four doctests are the exception and they matter. All four are
`compile_fail`, all four are in `handler.rs`, and all four execute because
`handler.rs` is `pub mod` (`lib.rs:17`) and `ci.yml:175` runs
`cargo test -p host-runtime --doc` under the step name "Rust lease non-escape",
printed and confirmed.

| Site | Forbids |
| --- | --- |
| `handler.rs:213-219` | `RequestOutcome::Response { body: Vec::<u8>::new(), .. }` |
| `handler.rs:425-427` | `ctx.corr` |
| `handler.rs:429-431` | `ctx.socket` |
| `handler.rs:433-435` | `ctx.credentials` |

Six compiled doctests exist in the whole `host-runtime` library, all `compile_fail`:
these four plus `frame_channel.rs:296-301` and `:303-308`, which are 2b's and
are that sub-part's only CI-executed source-resident checks. 2d has none. So 2e
owns four of the six. The three `RequestCtx` doctests are weaker than the first
and worth separating: each asserts that a field name does not resolve, so a
field renamed rather than removed still fails them, which pins absence rather
than privacy. `handler.rs:213-219` is stronger, because `OutputBuffer`'s
`pub(crate)` fields make the failure a type error no rename can satisfy.

**Three quiet areas frame the fault map.** Stated here in full because each is
the gap between what the code decides and what any check proves, and all three
are carried in [existing-checks.md](request-path/existing-checks.md).

1. **`dispatch.rs` decides every terminal on 1,497 production lines and carries
   2 in-crate tests, both about length arithmetic.** Those 1,497 lines own
   `Settlement` (`:34`), `settle` (`:399`), `dispatch_request` (`:828`),
   `open_route` (`:1103`), `close_generation` (`:1394`),
   `force_close_all_routes` (`:1421`), and `handle_cancel` (`:1489`). The two
   tests at `:1502` and `:1524` cover `error_body_len` (`:115`). Both run in CI in
   this tree (`ci.yml:118`, `:126`) and neither touches a terminal. All five silent exits, the emptiness gap
   at `:1031`, and the missing acknowledgement at `:447-460` sit in the same
   file, so the three highest-consequence findings in this catalog all land
   where in-crate coverage is thinnest.
2. **The silent exits emit no terminal, no cause, and no counter.** At `:1058`,
   `:1164`, `:1174`, and `:1199` the code emits no frame, records no cause, and
   increments no metric; `remove_pending` (`:1097`) removes the entry and
   returns nothing. The comments at `:1162-1163` and `:1171-1173` argue each
   case correctly on ordering grounds, and the arguments are sound - running
   route-gone beside a still-executing bind would be worse than leaving the
   correlation unsettled. What is quiet is that the *chosen* outcome has no
   observation point: a caller learns only by its own deadline expiring, which
   is indistinguishable from a slow handler, and an operator learns nothing at
   all. `:637-638` compounds it by discarding unrelated queued terminals with
   the same absence of a counter. Contrast `ring_transport.rs:209-228`, which
   maintains four lifecycle counters for a strictly less consequential set of
   events.
3. **`routing.rs` holds 3 unconditional production panics under a
   process-global mutex with no poison recovery.** `:184`
   (`unreachable!("bind completion found route in {state:?}")`), `:446`
   (`panic!("{op}: registry lost route it owns")`), and the `assert_eq!` at
   `:447-450` all fire in release, all inside code holding the registry mutex,
   and the module doc at `:3-8` makes this registry the single owner of every
   route in the host. A panic there poisons the mutex, and unlike `client.rs`
   there is no `lock_unpoisoned` recovery: the next of 16 `.expect("registry
   lock")` sites converts one bad state transition into a cascade across every
   connection. `expect_occupant` (`:441`) is called on every occupant mutation,
   so it is the most-executed guard in the sub-part and the least
   characterised. Verified here by grep: `:184`, `:446`, and `:447` are the only
   panic-family sites in `routing.rs`'s production half, and `:506-507`'s two
   `panic!` calls are inside the test module.

## Reachability: admission and dispatch

**Fifteen of the sixteen records are `default-production`; one,
[req-a-both-admission-classes-and-the-rejection-bound-saturate](#req-a-both-admission-classes-and-the-rejection-bound-saturate),
is `test-only` in this tree.** The labels rest on three facts, re-verified here,
per METHOD rule 4.

1. **The routed request path is `run`'s default path.** `host_runtime::run`
   (`runtime.rs:541`) accepts every connection through `run_connection`
   (`connection.rs:86`), whose `read_loop` (`:337`) is the ring's only frame
   consumer and calls `dispatch_request` (`dispatch.rs:780`). The source catalog
   cited the daemon binary `crates/daemon/src/bin/eidnara_host/serve.rs` as the
   production caller of `run`; that crate is not in this tree (scheduled for U4,
   `docs/properties/README.md:52`), and `run` is reached here from examples and a
   bench. This catalog labels the path `run` takes by default as
   `default-production` and defers the question of `run`'s own callers to the wave
   that lands them; see bias B1 in
   [discovered-at-u3/portfolio-evaluation.md](discovered-at-u3/portfolio-evaluation.md).
2. **`RouteClass::Reserved` is declared only by a composed component.** The
   comment at `runtime.rs:118-119` says the reserved pools are zero-permit when no
   module declares a reservation. In this tree the only declarer is
   `BrocaComponent::resources` (`broca/mod.rs:151`), and every `BrocaComponent`
   constructor is called only from `crates/host-runtime/tests/`. `RouteClass` is
   read back by dispatch to pick a permit pair (`dispatch.rs:821`), so
   reserved-class dispatch is live code, but the state that saturates it is
   reachable only through a test composition. The one record whose required state
   is reserved saturation is therefore `test-only`, and the permit-pair record
   states which half of its bound the reservation covers.
3. **Nothing in the five files is `cfg`-gated on the production path.** The
   sub-part's only `#[cfg(test)]` markers are module gates for inline tests.
   Sub-part 2f's construction conditionality map establishes independently that
   nothing in the host runtime is feature-gated or `cfg`-gated.

Two code points inside this surface are entered only by a failing host rather
than by a configuration gate, and both are stated at the record rather than
relabelled: `dispatch.rs:1164` and `:1174`, whose enabling state is the fatal
latch inside `lifecycle_join` (`runtime.rs:186-207`).

**The two records carried in later, in
[Group F](#group-f-composite-route-ownership-and-panic-containment), are also
`default-production`.** Fact 1 establishes the routed path; what those two need
in addition is that a composite is on it, and `StaticComposite` is what every
in-tree caller of `run` passes. `composite.rs` contains **zero `#[cfg]` attributes**
of any kind.

## Index

Fourteen records from this sub-part's own lens passes, in the order lens A
proposed them. Lens B proposed none by design; it built the 20-claim register and
the check inventory. **Two further records were carried into this sub-part in a
later pass**, from the superseded pre-refactor `part-2b-wire-and-channels`; they
are the last two rows and they live in
[Group F](#group-f-composite-route-ownership-and-panic-containment). Sixteen
records in total.

| Slug | Type | Confidence |
| --- | --- | --- |
| [req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame](#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame) | safety | high |
| [req-a-a-routed-terminal-carries-no-delivery-acknowledgement](#req-a-a-routed-terminal-carries-no-delivery-acknowledgement) | safety | high |
| [req-a-a-response-publication-failure-never-reaches-the-settling-path](#req-a-a-response-publication-failure-never-reaches-the-settling-path) | safety | high |
| [req-a-a-handler-response-is-length-checked-and-never-content-checked](#req-a-a-handler-response-is-length-checked-and-never-content-checked) | safety | high |
| [req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired](#req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired) | safety | high |
| [req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining](#req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining) | safety | high |
| [req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes](#req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes) | safety | high |
| [req-a-a-handler-outliving-every-host-deadline-is-reached](#req-a-a-handler-outliving-every-host-deadline-is-reached) | reachability | high |
| [req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation](#req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation) | safety | high |
| [req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs](#req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs) | safety | high |
| [req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close](#req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close) | safety | medium |
| [req-a-three-control-rejection-paths-carry-three-different-bounds](#req-a-three-control-rejection-paths-carry-three-different-bounds) | safety | high |
| [req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait](#req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait) | safety | high |
| [req-a-both-admission-classes-and-the-rejection-bound-saturate](#req-a-both-admission-classes-and-the-rejection-bound-saturate) | reachability | high |
| [composite-route-entry-is-removed-by-exactly-one-route-gone](#composite-route-entry-is-removed-by-exactly-one-route-gone) | safety | high |
| [composite-panic-containment-covers-only-optional-health-and-shutdown](#composite-panic-containment-covers-only-optional-health-and-shutdown) | safety | high |

The last two rows are the carried records. They keep their original unprefixed
slugs so the carry stays visible against the fourteen `req-a-` records this
sub-part derived itself.

Semantics distribution: twelve `always`, two `sometimes`. No
`always-or-unreached`, no `reachable`, no `unreachable`. Type distribution:
twelve safety, two reachability, no liveness. Reachability distribution:
fourteen `default-production`. Confidence: thirteen high, one medium.

The two carried records add **2 safety** and semantics **2 `always`**, both
`default-production` and both high confidence, so the sixteen-record totals are
**fourteen safety, two reachability, no liveness**; semantics **fourteen
`always`, two `sometimes`**; reachability **sixteen `default-production`**; and
confidence **fifteen high, one medium**.

**The five group headings below are this synthesis's own**, chosen by shared
mechanism rather than by the order records were proposed. Grouping reorders the
records relative to the index; the index is the record-order artifact. Record
bodies are verbatim from lens A. Two formatting-only changes were applied
uniformly: fields are wrapped to about 80 columns, and evidence links are
rewritten from the lens file's relative form to `evidence/<slug>.md` so they
resolve from this directory. No wording was changed.

A sixth group,
[Group F](#group-f-composite-route-ownership-and-panic-containment), was appended
in a later pass for the two carried records. It sits after the relationship map
rather than in sequence with the five, for the reason given in its preamble.

---

## Group A: the arbiter that holds

Two records that currently hold, and both are premises rather than findings.
The first is the exactly-one-claimant guarantee that `Settlement` provides, and
the second is that no emission reaches a retired generation or an
already-settled correlation. They are grouped because both are proved by the
same discipline: the `won.swap` under the async `order` mutex
(`dispatch.rs:407-410`) plus the unconditional recheck of
`gen.writer.is_retired() || gen.token.is_cancelled()` at each of the four
emission entry points. Both are stated so that a regression has something to
violate.

### req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:358` and `:453` race cancel against
completion and assert one terminal; both binaries run in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`), and neither
races route close or generation teardown into the same settlement.
Guarantee: For one routed correlation, at most one of `Response`, `Error`, or
`StreamEnd` reaches the writer queue, whichever of handler completion,
cancellation, route close, and generation teardown arrives first.
Check: `always` - over every correlation observed on a generation, the count of
terminal-typed frames carrying that `(channel, epoch, corr)` is at most one, and
no `StreamData` for that correlation follows its terminal. `always` because the
arbiter is evaluated on every settlement attempt, not on an optional path.
Fault/timing angle: The window is between `settlement.order.lock()` and the
`won.swap` at `dispatch.rs:408`. A stream `send` holds the same lock while
emitting and stores `streamed` before releasing it (`:583`, `:599`), so the
`has_streamed` read at `:418` cannot observe a torn state.
Required faults and enabling state: A live route with a handler that both
streams and returns a unary `Response`, plus a client `Cancel` and a route
`Goodbye` delivered inside the handler's execution window. Three settlement
claimants must be in flight at once.
Confidence: high - [evidence](evidence/req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame.md).
Verified the `swap` is the only mutator of `won`, that all five emission sites
take the order lock, and that the `streamed` store precedes lock release.
Existing check: `tests/dispatch.rs:358` `cancel_and_completion_settle_exactly_once`,
`:453` `simultaneous_cancel_and_completion_still_emit_one_terminal`,
`:504` `cancelling_a_stream_stops_it_with_one_terminal`. Status unaudited; runs
in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: A duplicate terminal settles a correlation the client has already
retired and, per Part 2d, is dropped as unmatched; a `Response` after
`StreamData` corrupts the client's view of the stream.
Open questions: None.

### req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:835`
`closing_a_route_settles_its_admitted_work` and `tests/routing.rs:435` cover
close-then-request. No test lets a handler complete *after* its generation
retired and asserts nothing is emitted.
Guarantee: A handler result that arrives after its generation was cancelled or
retired, or after its correlation was settled by another claimant, produces no
frame on any generation.
Check: `always` - at every emission entry point, assert that
`gen.writer.is_retired() || gen.token.is_cancelled()` implies no frame is
constructed, and that `settlement.won` already true implies no frame is
constructed. `always` because the recheck runs unconditionally on each of the
four entry points.
Fault/timing angle: The recheck sites are `dispatch.rs:195-197` (charged error
body), `:277-282` (`emit_frame_with_written`), `:323-325`
(`emit_reserved_frame`), and `:519-524` plus `:531-536` (`StreamSink::reserve`,
which rechecks both before and after the budget wait). The request token is a
free-standing root, not a child of the generation token
(`dispatch.rs:911-914`), so route close must cancel entries explicitly
(`:1338-1341`).
Required faults and enabling state: A handler that completes while its
generation is being torn down, with the terminal's byte charge acquired before
the cancellation and consumed after it.
Confidence: high - [evidence](evidence/req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation.md).
Verified all four entry points recheck, and that `StreamSink::reserve` rechecks
on both sides of the await so a charge granted before cancellation cannot be
used after it.
Existing check: `tests/dispatch.rs:835`, `tests/routing.rs:435`
`closed_route_requests_are_unknown_and_cleanup_is_idempotent`. Status
unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: A frame emitted onto a retired generation would be delivered to a
successor connection's peer if the writer were reused, or dropped as unmatched
if not. Part 2a's silent-close rule depends on this holding: a retirement that
fabricated a terminal would contradict protocol §6.3.
Open questions: None.

---

## Group B: exits that answer nothing

Three records on the missing half of exactly-once. The first is the disjunction
the pre-dispatch rejection path actually offers: emitted, or the generation is
retired with every other correlation's queued frames discarded. The second is
the three `open_route` exits that emit nothing on a connection that may stay
live. The third is the pending-entry sweep that `settle_route_work` performs and
`force_close_all_routes` does not. Grouped because all three are consequences of
choosing an ordering-safe silence over a terminal, and because in every case the
chosen outcome has no observation point.

### req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:295` and `:271` assert the terminal on
the healthy path. Nothing exercises the exhaustion path.
Guarantee: Every pre-dispatch rejection either enters the writer queue or the
generation is retired with its queue discarded; no rejection is silently
dropped while the generation stays live.
Check: `always` - on every `emit_rejection` call, assert either that a terminal
frame for the correlation is queued, or that `gen.token.is_cancelled()` and
`gen.writer` is discarding. `always` because the disjunction must hold at every
call, and the second disjunct is the code's actual answer past the bound
(`dispatch.rs:598-602`).
Fault/timing angle: The window is 32 concurrent no-dispatch rejections per
generation, all blocked on contended egress budget, before a 33rd arrives.
`busy_rejects` is `MAX_INFLIGHT_BUSY_REJECTS = 32` (`connection.rs:42`, `:244`).
Required faults and enabling state: A saturated egress byte budget plus a client
pipelining more than 32 requests that fail admission. Both are reachable: the
budget saturates in `tests/dispatch.rs:788`, and admission failure needs only a
closed route or an exhausted permit pool.
Confidence: high - [evidence](evidence/req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired.md).
Verified the permit is acquired before the spawn on both call paths and that the
exhaustion arm cancels and discards rather than awaiting inline.
Existing check: `tests/dispatch.rs:271` `an_unknown_route_is_refused_with_zero_dispatch`,
`:295` `saturated_request_capacity_returns_server_busy_without_dispatch`. Status
unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: `writer.discard()` drops queued frames belonging to *other*
correlations, so one client's rejection flood converts every in-flight peer
request on that generation into `outcome_unknown`. Protocol §10.2 lists
`server_busy` as a terminal that proves no dispatch; past this bound no such
terminal exists, so the proof the client is told to rely on is conditional on
capacity the client cannot observe.
Open questions: None.

### req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/routing.rs:396` and `:570`, and
`tests/handler_contract.rs:229`, cover the answering exits. No test drives a
bind panic, a bind deadline overrun, or a close that races a bind through
`open_route`.
Guarantee: A `route.open` correlation receives exactly one terminal unless the
host has tripped its fatal latch or the route's close already won, in which
case it receives none and the client learns only from its own deadline.
Check: `always` - on every `open_route` return, assert either that one terminal
frame carries the control correlation, or that `shared.fatal` is tripped, or
that the registry reports the handle in `Closing`. `always` because the
disjunction must hold on all seven exits.
Fault/timing angle: Three windows. A bind callback that panics or overruns
`lifecycle_callback_deadline` (30 s default, `config.rs:225`) exits at
`dispatch.rs:1164-1170` or `:1174`. A close marked between `reserve` and
`install_bound` produces `BindInstall::CloseWins` at `:1195-1202`.
Required faults and enabling state: For the first two, a handler `bind` that
panics or blocks. Parts 4c and 4d found handler panics, so this is not
hypothetical. For the third, a generation teardown or forced drain concurrent
with an in-flight bind; `routing.rs:408-413` marks mid-bind routes
close-requested rather than draining them.
Confidence: high - [evidence](evidence/req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining.md).
Enumerated all seven exits, and confirmed via `runtime.rs:186-207` that both
lifecycle-failure variants trip the fatal latch before returning.
Existing check: `tests/routing.rs:396` `rejected_bind_never_publishes_and_still_reports_route_gone`,
`:570` `route_capacity_exhaustion_is_refused_without_binding`,
`tests/handler_contract.rs:229` `a_rejected_bind_carries_the_handler_code_to_the_client`.
Status unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: Protocol §8.2 acknowledges an abandoned `route.open` and gives the
client a remedy keyed on receiving an *unmatched control `Response`*. On these
three exits there is no frame at all, so that remedy never triggers and the
client burns its full 30-second route deadline. Repeated bind panics therefore
cost one route deadline each.
Open questions:
- Is the `CloseWins` silent exit reachable on a generation that stays live
  afterwards, or does every producer of that decision also retire the
  generation? `settle_route` is called from host shutdown, so the host is at
  least draining; a route `Goodbye` cannot reach it because the client does not
  yet know the handle. (unresolved, needs the shutdown-path caller list from
  `harness_closure.rs`, which is sub-part 2f)

### req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test inspects `gen.pending` after a forced close.
Guarantee: Every entry inserted into `gen.pending` is removed either by the
outer dispatch task on each of its exits, or by the route close that aborted
that task.
Check: `always` - for every key inserted into `gen.pending`, its removal originates
either from the outer dispatch task on one of its exits or from the route close
that aborted that task, and no other path removes a live entry; and after the
generation quiesces, `gen.pending` is empty. Owner attribution is asserted per
key, by recording the inserting task and the removing site, because an early
removal by an unrelated path leaves the map empty at quiescence while a later
`Cancel` can no longer find its request. The emptiness postcondition is
`always(!X)` on a forbidden state; the ownership clause is `always` over every
removal.
Fault/timing angle: `remove_pending` is called on all five outer-task exits
(`dispatch.rs:935`, `:958`, `:1059`, `:1066`). The abort case is covered by
`settle_route_work`'s explicit sweep of the keys it collected
(`:1332-1342`, removal at `:1374-1380`), whose comment states "Aborted tasks
never removed their own pending entries". `force_close_all_routes`
(`:1421-1452`) aborts the same tasks and performs **no** equivalent sweep.
Required faults and enabling state: A forced shutdown past the drain deadline
with requests in flight, so `force_close_all_routes` aborts outer tasks whose
keys no `settle_route_work` collected. **Placement constraint, established during
disposition:** the oracle must live in-crate. `mod connection` is private
(`lib.rs:24`), so no integration test can name `GenerationCore`, but `pending` is
`pub` on it (`connection.rs:95`) and therefore directly readable and insertable
from any in-crate test.
Confidence: medium - [evidence](evidence/req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close.md).
The sweep asymmetry is verified by reading both functions. What I could not
verify is whether the forced path is always followed by the whole
`GenerationCore` being dropped, which would make the leak unobservable; that
depends on `runtime.rs:1144-1244`, which is sub-part 2f. **One premise of the
original record was wrong and is corrected here: the map is not unobservable.**
The claim that "the map is private to the crate" and that "no in-crate test
constructs a `GenerationCore`" is false on the second half.
`connection.rs:946-963` (`shutdown_registration_rejection_leaves_no_graceful_drain_work`)
constructs a complete `GenerationCore` today, all eleven fields, using
`frame_sender` for the writer, and asserts against it. So the postcondition is
assertable; what it costs is placing the oracle in an inline unit-test lane, which
CI runs in this tree (`ci.yml:118`, `:126`); that is a trade rather than a block.
Existing check: none for this record's postcondition.
`connection.rs:946-963` is not a check of it - it constructs a `GenerationCore`
for an unrelated claim - but it is the construction proof this record's oracle
needs. Status `unaudited`.
Impact: Bounded by the pending-permit pool in the worst case, so this is not
unbounded growth. The consequence is a stale `PendingEntry` holding a
`CancellationToken` and an `Arc<Settlement>` for the remaining life of the
generation, which makes `handle_cancel` for that key a live no-op against an
already-dead task.
Open questions:
- Does the forced path always drop the `GenerationCore` immediately afterwards?
  `close_generation` removes the connection at `dispatch.rs:1409-1413`, but
  `force_close_all_routes` does not call it. (unresolved, needs sub-part 2f)
- Should the oracle be an in-crate test that reads `pending` directly, or should
  a test-only accessor expose it so the integration binaries CI might one day run
  can assert it? The first is free and runs nowhere; the second is a production
  edit. (needs human input)

> Synthesis note on this record's open question, carried here rather than edited
> into it. Sub-part 2f's construction conditionality map answers the adjacent
> question and not this one. 2f establishes that `shutdown_sequence`
> (`runtime.rs:936`) calls `force_close_all_routes` twice (`:1206`, `:1216`)
> with no enclosing timeout, and that `run` returns after
> `run_handler_shutdown` (`:1240`). It does **not** establish that the
> `GenerationCore` is dropped at either call site, because the connections map
> is cleared elsewhere. So the record's `medium` confidence and its open
> question both stand, and the question is now known to be answerable only from
> 2f's `runtime.rs:1144-1244`, not from `harness_closure.rs` as lens A guessed.

---

## Group C: a terminal that proves nothing

Three records on what a routed terminal does and does not establish. The first
is that it carries no delivery acknowledgement at all, so the host's
acknowledged effect count is identically zero. The second is that a publication
failure lands after the settling path has already returned success, so the host
believes the request was answered while the client believes nothing was. The
third is that the success gate checks only that the body fits, so a zero-length
`Response` is accepted by every layer from the gate to the client. Grouped
because all three are about the distance between "settled" and "answered", and
because together they mean the host's own record of a request's outcome cannot be
reconciled with the client's. The third record's original framing - a handler
*failure* arriving as a success - was narrowed during disposition to
empty-response acceptance; see its `Confidence:` line.

### req-a-a-routed-terminal-carries-no-delivery-acknowledgement

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test distinguishes a queued terminal from a delivered
one for a routed correlation, because the host exposes no signal to distinguish
them.
Guarantee: A routed terminal's logical settlement records only that the frame
entered the writer queue; the host retains no evidence that any routed terminal
reached the peer.
Check: `always` - for every routed terminal emission, the `OutboundFrame`
carries `written: None`, so host-side acknowledged effects are identically zero
while attempted effects equal the settlement count. Per METHOD's effect
accounting, assert observed client terminals at most the host's settlement count
and at least zero, and assert per-correlation that no host state claims delivery.
`always` because it holds on every emission, not on a failure path.
Fault/timing angle: The gap is unbounded in time. `send_before` returning `Ok`
proves queue admission (`frame_channel.rs:715-723`); publication happens later
inside the endpoint thread (`ring_transport.rs:536-578`).
Required faults and enabling state: A generation whose writer queue holds a
settled terminal when the generation is cancelled or the publication fails. The
settlement is already recorded; the frame never leaves.
Confidence: high - [evidence](evidence/req-a-a-routed-terminal-carries-no-delivery-acknowledgement.md).
Enumerated every `written:` construction in the sub-part: three hooks exist, all
on control or teardown frames, none on a routed terminal.
Existing check: none.
Impact: The host cannot report, log, or meter which requests were actually
answered. Protocol §10.1 makes an unobserved terminal `outcome_unknown` on the
client side; the host has no matching classification, so the two ends cannot be
reconciled after a close.
Open questions:
- Should routed terminals carry a `written` hook for metering, given the hook
  is a boxed closure per frame? (needs human input)

### req-a-a-response-publication-failure-never-reaches-the-settling-path

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test drives a serializer that writes the wrong number
of bytes, and none asserts what the client observes when a settled terminal
fails to publish.
Guarantee: When a settled terminal fails to publish, the settling path has
already returned success, the request is recorded as settled, and the client's
only signal is a clean connection close.
Check: `always` - whenever `publish_one` returns `Err` for a frame whose
correlation has `won == true`, assert that no `Error` terminal for that
correlation is emitted afterwards and that the generation's close carries no
distinguishing reason; and, positively, that the client observes the clean
connection close and settles the affected request within the transport's bounded
teardown window, so a failure that leaves the endpoint or the client pending
forever fails the check instead of passing on the absent `Error` alone. `always` rather than `always-or-unreached` because the
settlement half runs on every request; only the failure half is conditional, and
the guarantee is about their relationship.
Fault/timing angle: `settle` completes at `dispatch.rs:460` once `send_before`
returns `Ok`. The failure occurs later in the endpoint thread. The two are not
ordered by anything, so the settling task is typically already gone.
Required faults and enabling state: A handler that calls
`RequestCtx::output_from_writer` with an `exact_len` its serializer does not
match, or a ring reservation that fails under contention.
`reservation.commit(body_len)` then returns `ProducerError::Underfill`
(`shm-transport/src/backend/ring.rs:1363-1367`).
Confidence: high - [evidence](evidence/req-a-a-response-publication-failure-never-reaches-the-settling-path.md).
Traced the direct-output path from `dispatch.rs:332-349` through
`ring_transport.rs:580-593` into `commit`, and confirmed `publish_one` discards
the `written` hook on failure without touching the settlement.
Existing check: none in this sub-part. Part 2b holds
`ring-a-publish-failure-is-reported-as-a-clean-peer-close`, which establishes
the close half but not the settlement half.
Impact: The host believes the request was answered and the client believes
nothing was answered. Combined with Part 2d's finding that a clean host close
and a transport failure share one code, the client cannot attribute the loss,
and any effect the handler already applied is invisible to it.
Open questions:
- Does any production handler use `output_from_writer` with a computed
  `exact_len` that could disagree with its serializer? That is `daemon`'s
  side of the boundary. (unresolved, needs an `daemon` audit)

### req-a-a-handler-response-is-length-checked-and-never-content-checked

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:665`
`oversized_handler_output_cannot_corrupt_framing` covers the upper bound only.
No test constructs a handler that reserves owned output and returns `Response`
without writing.
Guarantee: The dispatch layer validates a handler `Response` against the frame
size ceiling and nothing else, so a handler that reserved owned output and wrote
nothing produces a well-formed zero-length `Response` terminal that every layer
below accepts.
Check: `always` - for every `RequestOutcome::Response` accepted at
`dispatch.rs:963-964`, the only predicate applied is
`body.len() <= MAX_BODY_LEN`; assert there is no lower-bound, emptiness, or
declared-versus-written comparison anywhere on the path to
`emit_reserved_frame`. `always` because the check runs on every unary success.
Fault/timing angle: None. This is a static gap in the guard, not a race.
Required faults and enabling state: A handler that reserves an **owned**
`OutputBuffer` through `reserve_output`, takes an early return without writing,
and still returns `RequestOutcome::Response`. The owned path is the whole record:
`OutputBuffer::len()` (`handler.rs:361-366`) returns the *written* `body.len()`
for an owned buffer and the *declared* `direct.len` for a direct one, so only the
owned shape reaches `:1031` reporting zero.
Confidence: high - [evidence](evidence/req-a-a-handler-response-is-length-checked-and-never-content-checked.md).
**Narrowed during disposition, and the narrowing changed what the record claims.**
What is verified is that an **empty success is accepted end to end**, at five
independent points: an owned reservation starts empty
(`dispatch.rs:537-542`, `Vec::with_capacity` with no writes), `len()` reports the
written length for it (`handler.rs:361-366`), the gate accepts zero
(`dispatch.rs:1031-1034`), `Response` is not a pure-header type so decode rejects
a body only for `Cancel`/`Ping`/`Pong`/`Goodbye` (`wire.rs:48-88`, rule at
`:340-342`), and the Rust client's `validate_inbound` imposes no minimum on a
`Response` (`client.rs:2022-2031`, which checks only `corr != 0` and the
binary-flag-on-channel-0 rule). What is **not** established is that this is a
handler *failure* surfacing as a success: the handler explicitly returned the
variant `handler.rs:220-225` documents as "Unary success", so nothing in the
observable state distinguishes a failed reservation from a deliberate empty
result. That question is upstream of the record and is referred to a human.
Existing check: `tests/dispatch.rs:665` for the ceiling. Status unaudited; runs
in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: An empty `Response` is indistinguishable from a legitimately empty result
at every layer that could reject it, so a handler that abandons its output
mid-request and still reports success is invisible. The severity depends entirely
on whether an empty `Response` is a supported outcome, which nothing states.
Note what this record does **not** cover after narrowing: the *direct*-output
underfill, where a declared `exact_len` is never satisfied, is caught - it fails
at publication with `ProducerError::Underfill` rather than at this gate, which is
[req-a-a-response-publication-failure-never-reaches-the-settling-path](#req-a-a-response-publication-failure-never-reaches-the-settling-path)'s
territory. The gap here is specifically the owned path, where declared and written
are the same field and zero is legal.
Open questions:
- Is a zero-length `Response` a defect or a supported outcome?
  `handler.rs:220-235` does not state the intent, and
  `OutputBuffer::is_empty()` (`:368-370`) exists as public API, which weakly
  suggests emptiness is a state callers are expected to reason about rather than
  an error. Settling this decides whether this record is a missing guard or a
  documentation gap. (needs human input)
- Does any client treat an empty-body `Response` as a protocol violation? The
  Rust client does not (`client.rs:2022-2031`, verified). The TypeScript peer is
  Part 5's surface. (unresolved, needs a Part 5 check)

---

## Group D: admission bounds and the deadline that does not exist

Three records on capacity. The first states the bound that exists and is
correctly acquired before any spawn, along with the fact that all four pools are
host-global. The second is the reachability of a handler outliving every
deadline the host configures, which is what makes the first bound reclaimable
only by handler cooperation. The third is the reachability of all five
saturation states, because a bound asserted only in theory is not a bound.
Grouped because the first is the premise the other two test, and because the
reserved carve-out's whole purpose is to survive the states the third record
must construct.

### req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs

Type: safety
Reachability: default-production for the general-class pair, which every routed request through `host_runtime::run` takes; the reserved-class pair is declared only by `BrocaComponent::resources` (`broca/mod.rs:151`), whose constructors have only test callers in this tree, so the reserved half of the bound is exercised only in tests until a production composition declares a reservation.
Status: active
Exercised: partial - `tests/dispatch.rs:976` and `:1074` prove the two classes
cannot consume each other; `tests/handler_contract.rs:323` and `:636` prove the
startup checked-sum. No test proves the permits are acquired *before* the spawn
rather than inside it.
Guarantee: Concurrent handler callbacks are bounded by the class-scoped
`task_permits` pool, concurrent unsettled requests by the class-scoped
`pending_permits` pool, both acquired non-blockingly on the read loop before any
task is spawned, and each class is unreachable from the other.
Check: `always` - assert that live handler callbacks never exceed the class's task-permit count, that unsettled requests never exceed its pending-permit count, that both acquisitions are `try_acquire_owned` on the reader so exhaustion rejects instead of queueing, and that each route class acquires only from its matching permit pair (`dispatch.rs:821`): with the general pools saturated a reserved-class request is admitted and with the reserved pools saturated a general-class request is admitted, and neither class ever holds a permit from the other pair (`saturated_broca_reserve_cannot_consume_a_general_slot` and `saturated_general_capacity_cannot_consume_the_broca_reserve` in `tests/dispatch.rs` are the existing forms). `always` because all four bounds must hold at every instant.
Fault/timing angle: The task permit is released when the handler returns
(`dispatch.rs:990`, inside the inner task) while the pending permit is held
across the egress wait (`:933`, in the outer task). Under a slow peer the two
counts diverge, which is intended: a blocked terminal must not occupy handler
capacity.
Required faults and enabling state: A client pipelining more requests than
`max_handler_tasks` on one route, plus a slow-reading peer so terminals queue
and the pending count exceeds the task count.
Confidence: high - [evidence](evidence/req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs.md).
Verified pool construction at `runtime.rs:905-912`, class selection at
`dispatch.rs:873-879` from `route_tracker`'s stored class, and Broca's live
96/96 reserved declaration.
Existing check: `tests/dispatch.rs:976` `saturated_broca_reserve_cannot_consume_a_general_slot`,
`:1074` `saturated_general_capacity_cannot_consume_the_broca_reserve`,
`tests/handler_contract.rs:323` `reservations_must_leave_one_general_slot_in_each_pool`,
`:636` `zero_reservation_handlers_keep_single_pool_admission`. Status unaudited.
All run in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: All four pools are host-global, so one connection can hold every general
permit. Per-connection fairness is not provided at this layer; if it is
required, it is required somewhere else and nothing here supplies it.
Open questions:
- Is per-connection handler-capacity fairness owned anywhere? `connection_permits`
  bounds connection count but not per-connection dispatch share. (unresolved,
  needs sub-part 2f's `runtime.rs` and `config.rs` pass)

> Synthesis note resolving this record's open question, carried here rather than
> edited into it. **No layer supplies per-connection handler-capacity
> fairness.** Sub-part 2f enumerated every field of `HostLimits`, `HostTiming`,
> `LivenessPolicy`, `HostInit`, and `HostConfig` against its consumers, 21 keys
> in total, and the only per-connection capacity key is `writer_queue_frames`
> (`config.rs:141`, consumed at `connection.rs:145`), which bounds one
> generation's writer queue depth and not its dispatch share. `max_connections`
> (`config.rs:129`) bounds connection count at `runtime.rs:872` and `:914`. All
> four admission pools are constructed once at `runtime.rs:905-912` and stored
> on `HostShared`, which 2f establishes is frozen for the incarnation. So the
> record's `Impact:` sentence "if it is required, it is required somewhere else
> and nothing here supplies it" is now confirmed for the whole host, not merely
> for this layer.

### req-a-a-handler-outliving-every-host-deadline-is-reached

Type: reachability
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:295` parks a handler in a "hang" mode to
occupy a permit, so the state is constructed; nothing asserts the absence of a
host-side bound or measures how long the permits stay held.
Guarantee: A campaign reaches the state where an admitted request's handler has
been executing longer than every deadline the host configures, with its route
and generation still live and no `Cancel` outstanding.
Check: `sometimes` - at least once per campaign, observe a request whose
handler has held its task and pending permits for longer than
`max(frame_deadline, lifecycle_callback_deadline, route_close_budget)` while
`route_tracker` still reports the route live and the pending entry is unsettled.
`sometimes` and not `reachable`: the branch lines are trivially executed by any
slow handler, but the operational state that matters is a handler outliving the
host's whole deadline vocabulary, which is a situation, not a location.
Fault/timing angle: `HostTiming` (`config.rs:199-218`) has no request field.
`route_close_budget` (5 s) applies only once a close begins;
`lifecycle_callback_deadline` (30 s) applies to `bind`, `route_gone`,
`initialize`, and `health`, never to `handle`.
Required faults and enabling state: A handler whose `handle` blocks on an
external dependency with no internal timeout. Protocol §11 assigns the
30-second request deadline to the *client*, so a client that dies without
sending `Cancel` leaves the host holding the permits indefinitely.
Confidence: high - [evidence](evidence/req-a-a-handler-outliving-every-host-deadline-is-reached.md).
Read all seven `HostTiming` fields and every `timeout`/`timeout_at` call in
`dispatch.rs`; none wraps the handler callback.
Existing check: `tests/dispatch.rs:295` constructs the state incidentally.
Status unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: Handler-task capacity is reclaimed only by handler cooperation, client
`Cancel`, route close, or generation teardown. A module with a missing internal
timeout can hold all 256 general task permits, at which point every other
route's traffic gets `server_busy` while the host reports itself healthy.
Open questions:
- Should the host own a request deadline at all, given protocol §11's rule that
  each operation owns exactly one absolute deadline and it assigns the request
  deadline to the client? Adding one would create the multiplied timer §11
  forbids. (needs human input)

### req-a-both-admission-classes-and-the-rejection-bound-saturate

Type: reachability
Reachability: test-only - the reserved class exists only when a composed component declares it, and the only declarer in this tree is `BrocaComponent::resources` (`crates/host-runtime/src/broca/mod.rs:151`), whose constructors are called only from `crates/host-runtime/tests/`. Reserved-pending and reserved-task saturation are therefore reachable only in tests here; reclassify when a production composition declares a reservation.
Status: active
Exercised: partial - `tests/dispatch.rs:295`, `:976`, and `:1074` saturate
pending capacity in both classes. Task-permit saturation and `busy_rejects`
saturation are constructed by no test.
Guarantee: A campaign reaches each of the five distinct saturation states this
layer can enter, so no admission bound is asserted only in theory.
Check: `sometimes` - at least once per campaign, observe each of: general
pending exhaustion, general task exhaustion, reserved pending exhaustion,
reserved task exhaustion, and per-generation `busy_rejects` exhaustion.
`sometimes` and not `reachable` because a campaign can execute the
`try_acquire_owned` error arm of one pool while never producing the operational
state of a saturated *task* pool or a saturated rejection counter, and those
are the states whose consequences differ.
Fault/timing angle: Task-permit exhaustion requires more than 256 concurrent
*executing* handlers, which is harder to reach than pending exhaustion because
the task permit releases on handler return.
`busy_rejects` exhaustion additionally requires contended egress so the 32
in-flight rejections do not drain.
Required faults and enabling state: A shrunk configuration (`max_handler_tasks`
and `max_pending_requests` lowered, as `tests/dispatch.rs:295` already does for
pending), a parked handler, a saturated egress budget, and a client pipelining
past each bound.
Confidence: high - [evidence](evidence/req-a-both-admission-classes-and-the-rejection-bound-saturate.md).
Enumerated the five `try_acquire_owned` sites and confirmed which existing tests
reach which.
Existing check: `tests/dispatch.rs:295`, `:976`, `:1074` for pending in both
classes. Status unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: The reserved class exists specifically to survive general-load
saturation. If reserved *task* exhaustion is never constructed, the carve-out's
second half is unverified, and `runtime.rs:118-119`'s claim that the reserved
pools may be "unreachable" would go unchallenged even though Broca makes them
live.
Open questions: None.

---

## Group E: rejection, three shapes and three bounds

Three records on the fact that rejection is not uniform. The first is that one
shutdown condition evaluated at two call sites answers with two codes carrying
two different client retry rules. The second is that a channel-0 rejection is
bounded by one of three different counters depending on why it was rejected, and
only one of the three can prove its terminal reached the peer. The third is the
one policy in this area that is applied correctly and is applied twice by hand.
Grouped because all three are about the vocabulary and the accounting of saying
no, which the routed and control chains do differently at every level.

### req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `tests/lifecycle.rs:570` (re-located at HEAD)
`shutdown_refuses_new_routes_and_new_routed_work` asserts both codes against one
draining host, and `lifecycle` is CI-executed on Linux (`ci.yml:168-169`);
`ci.yml` has no macOS jobs after PR #131 (merge `5d638e3e8`)
Guarantee: The shutdown admission fence is one condition evaluated at two call
sites, and the two sites answer with different error codes carrying different
client retry rules.
Check: `always` - whenever `shared.draining` is set or `shared.shutdown` is
cancelled, assert that every routed request receives `server_busy` and every
`route.open` receives `target_unavailable`, and record that both are attributed
to the same cause. `always` because the fence is evaluated on every admission.
Fault/timing angle: The fence is checked twice per request kind: advisorily at
`dispatch.rs:844` and `:1112`, then authoritatively under the registry lock at
`routing.rs:305`. `handle_host_shutdown`'s write hook sets both `draining` and
`freeze_admission` inside the writer task (`dispatch.rs:752-753`), so the
commit point and the fence coincide.
Required faults and enabling state: An authenticated `host.shutdown`, or an
external shutdown signal, with a client pipelining both a routed request and a
`route.open` behind it.
Confidence: high - [evidence](evidence/req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes.md).
Both call sites read; protocol §10.2's two retry rows compared.
Existing check: **corrected during disposition from "none".**
`tests/lifecycle.rs:570-651` `shutdown_refuses_new_routes_and_new_routed_work`
asserts this property exactly and in the record's own shape: it holds a drain open
with a parked handler (`:584-600`), spawns the shutdown (`:605`), waits for the
publication to be unlinked (`:608-615`), then sends a `route.open` and asserts
`open_error.error_code() == "target_unavailable"` (`:620-632`) and sends a routed
request on the still-live route and asserts
`request_error.error_code() == "server_busy"` (`:634-651`). Both codes, one
draining host, one test. Status `unaudited`. **In CI**, unlike every other check
this catalog cites: `ci.yml:168-169` runs `--test client --test lifecycle` on
Linux. The former macOS run of the same pair was removed by PR #131 (merge
`5d638e3e8`), which left `ci.yml` Linux-only.
Impact: Protocol §10.2 tells a client to retry `target_unavailable` "with new
correlation under bounded route deadline" and `server_busy` "with backoff". A
draining host therefore invites un-backed-off `route.open` retries from exactly
the clients it is trying to shed, while backing off their routed traffic. The
divergence is not merely unchecked, it is **pinned by a CI-executed test**, so it
is current intended behaviour unless someone changes both the code and that test.
Open questions:
- Which code does the protocol intend for a `route.open` during shutdown? §12
  step 1 names `server_busy` for routed requests and is silent on `route.open`;
  §8.3 reserves `target_unavailable` for route admission failures such as
  channel exhaustion, which shutdown is not. (needs human input)

### req-a-three-control-rejection-paths-carry-three-different-bounds

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/routing.rs:212` and `:98` exercise the semantic
path. Part 2a holds `oversize-control-drain-work-is-bounded-without-ingress-budget`
for the oversize path. Nothing exercises all three under one saturation.
Guarantee: A channel-0 rejection is bounded by exactly one of three different
counters depending on why it was rejected, and only one of the three can prove
its terminal reached the peer.
Check: `always` - classify every channel-0 rejection emission by path and
assert the matching bound: semantic rejections by `pending_permits`, capacity
rejections and oversize rejections by the per-generation `busy_rejects` count of
32, and assert that only the oversize path attaches a `written` hook.
`always` because the classification holds on every rejection.
Fault/timing angle: The three paths are `emit_error_terminal` inside a
pending-permit-holding task (`connection.rs:638-655`), `emit_rejection`
(`connection.rs:625-635` and `dispatch.rs:613-641`), and
`emit_authoritative_rejection` (`connection.rs:430-450`,
`dispatch.rs:786-821`). Only the third carries `written_tx`, which the read
loop uses to fence exactly one authoritative frame past an otherwise silent
close (`connection.rs:391-400`).
Required faults and enabling state: Concurrent floods of malformed control
bodies, oversize control bodies, and requests past the pending-permit bound on
one generation.
Confidence: high - [evidence](evidence/req-a-three-control-rejection-paths-carry-three-different-bounds.md).
All three call sites and both bounding counters read; `MAX_INFLIGHT_BUSY_REJECTS`
confirmed as 32 at `connection.rs:42` and used at `:244`.
Existing check: `tests/routing.rs:98` `unsupported_operations_leave_the_generation_usable`,
`:212` `malformed_control_bodies_are_refused_before_handler_work`. Status
unaudited; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`).
Impact: A semantic-rejection flood consumes the same global pool that funds real
requests, so malformed control traffic degrades application throughput on every
connection, while a capacity-rejection flood is contained per generation. The
two attack surfaces have different blast radii for the same client behaviour.
Open questions:
- Protocol §8.3 says a control request is "one consumer request against the
  global unsettled bound", which the semantic path honours. Is charging
  malformed traffic to the *global* pool rather than a per-generation one the
  intended reading? (needs human input)

### req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/dispatch.rs:1524` `diagnostic_limit_substitution_drops_retry_hint`
is an inline unit test covering `bounded_terminal_error` only, and inline
`host-runtime` tests run in CI in this tree (`ci.yml:118`, `:126`). The `BindOutcome::Reject` copy of the same
policy has no test.
Guarantee: Handler-authored error codes and messages are truncated-by-
substitution to at most 128 and 4,096 bytes before the terminal is held across
any await, on both the request-error and bind-rejection paths.
Check: `always` - assert that no `Terminal::Error` or bind rejection retained
across an await carries a code above 128 bytes or a message above 4,096, and
assert the two capping sites use identical limits. `always` because the cap is
applied on every handler-authored diagnostic.
Fault/timing angle: The window is the egress wait. `dispatch.rs:1045-1049`
states it: the handler task permit is already released when the outer task
holds the terminal, so an uncapped string would accumulate uncharged across up
to `max_pending_requests` settlements. `dispatch.rs:1206-1209` states the same
for up to `max_routes` concurrent binds.
Required faults and enabling state: A handler returning a multi-megabyte error
message, at pending-pool or route-pool saturation, with a slow-reading peer so
the terminals queue.
Confidence: high - [evidence](evidence/req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait.md).
Both capping sites read and their limits compared: same constants, different
substitute messages, and the bind path re-implements the comparison by hand
instead of calling `bounded_terminal_error`.
Existing check: `tests/dispatch.rs:1524` (inline; runs in CI through `cargo test --workspace --all-targets` (`ci.yml:118`, `:126`)). Status unaudited.
Impact: Without the cap, `max_pending_requests` (1024) times an arbitrary
message is unbounded uncharged residency. With two hand-written copies of one
policy, a future limit change applied to one and missed in the other silently
reopens half of it.
Open questions: None.

---

## Relationship map

Grouped by shared mechanism rather than by the headings above, because the
sharpest relationships cross groups. **Every dominance statement below is a
hypothesis** about which oracle subsumes which, offered to order the work, not a
verified claim. None has been tested. **Corrected during disposition: one record
is CI-tested, though no dominance statement is.** The four `compile_fail` doctests
bear on the handler API surface rather than on any record here, but
`tests/lifecycle.rs:570-651` asserts the divergent-codes record exactly and runs
at `ci.yml:168-169` on Linux. That record appears in the fourth cluster below,
and its presence there is the only place a hypothesis could be checked against
something CI executes today.

- **One settlement primitive, read from four sides.**
  [req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame](#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame),
  [req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation](#req-a-no-emission-reaches-a-retired-generation-or-a-settled-correlation),
  [req-a-a-routed-terminal-carries-no-delivery-acknowledgement](#req-a-a-routed-terminal-carries-no-delivery-acknowledgement),
  [req-a-a-response-publication-failure-never-reaches-the-settling-path](#req-a-a-response-publication-failure-never-reaches-the-settling-path).
  All four turn on what `won` means. The first two say it is a sound arbiter of
  *who* emits; the last two say it is silent about *whether the bytes left*.
  Hypothesis: one trace that records, per correlation, the `won` transition, the
  frame the winner enqueued, and the writer's own publication result
  *dominates all four*, because each of their oracles is a projection of that
  same trace. Nothing dominates them pairwise: adding a `written` hook to routed
  terminals would dominate the acknowledgement record and give the
  publication-failure record its missing evidence, but it says nothing about
  arbitration, and strengthening the arbiter says nothing about delivery. This
  is the argument for building the trace once rather than four fixtures.
- **Five silent exits, one missing observation point.**
  [req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired](#req-a-a-pre-dispatch-rejection-is-emitted-or-the-generation-is-retired),
  [req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining](#req-a-a-route-open-is-answered-unless-the-host-is-failing-or-draining),
  [req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close](#req-a-every-pending-entry-is-removed-by-its-owner-or-its-route-close).
  These three cover four of the five exits between them: `:637-638` for the
  first, and `:1164`, `:1174`, and `:1199` for the second. **The third does not
  cover `:1058`, and the original text said it did.** It cites `:1059`, the
  `remove_pending` on the same match arm, which is the pending entry's removal
  rather than the absent terminal; `:1058` returns before `settle` at `:1063`, and
  it is the only one of the five silent exits that concerns an admitted routed
  request's settlement. Nothing here asserts that silence, which is a queued gap.
  Hypothesis: a per-exit counter or marker, incremented at each of the five sites,
  *dominates* the reachability half of all three, because every one of their
  oracles begins with "observe that this exit was taken". It dominates none of
  their safety halves: a counter at `:637-638` does not tell you which other
  correlations' frames the `discard()` dropped, a counter at `:1199` does not tell
  you whether the connection stayed live afterwards, and a counter at `:1058` does
  not tell you whether the pending entry was swept. Those three questions need
  three different oracles, which is why the pending-entry record is the only
  `medium` in the catalog.
- **Two hand-written copies of one policy.**
  [req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait](#req-a-handler-authored-diagnostics-are-capped-before-any-egress-wait),
  [req-a-a-handler-response-is-length-checked-and-never-content-checked](#req-a-a-handler-response-is-length-checked-and-never-content-checked).
  Both are about what the dispatch layer checks in a handler's own output.
  `bounded_terminal_error` (`dispatch.rs:82`) is applied to error diagnostics at
  `:1045-1049` and re-implemented by hand at `:1206-1218`, while the success
  path at `:1031` applies one upper bound and nothing else. Hypothesis: a single
  `validate_handler_outcome` funnel consulted by all three sites *dominates the
  diagnostics record by construction*, since the two copies could not drift, and
  *dominates the emptiness record not at all*, because whether an empty body is
  a failure is a contract question no refactor answers. Lens B's open question
  says the same thing from the other direction: `handler.rs:220-235` does not
  state the intent.
- **One shutdown condition, two vocabularies, three bounds.**
  [req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes](#req-a-shutdown-rejects-routed-and-control-work-under-divergent-codes),
  [req-a-three-control-rejection-paths-carry-three-different-bounds](#req-a-three-control-rejection-paths-carry-three-different-bounds).
  The first is about which code a rejection carries, the second about which
  counter bounds its emission and whether delivery can be proved. Hypothesis:
  an oracle that classifies every rejection by (cause, code, bounding counter,
  `written` hook present) *dominates both*, because both records are readings of
  the same four-tuple. Neither dominates the other: unifying the two codes would
  leave the three bounds untouched, and unifying the three bounds would leave
  the code divergence untouched. Note that only the oversize path can be proved
  delivered, which ties this cluster back to the acknowledgement record above.
- **The bound and the two states that test it.**
  [req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs](#req-a-handler-concurrency-is-bounded-by-two-class-scoped-permit-pairs),
  [req-a-a-handler-outliving-every-host-deadline-is-reached](#req-a-a-handler-outliving-every-host-deadline-is-reached),
  [req-a-both-admission-classes-and-the-rejection-bound-saturate](#req-a-both-admission-classes-and-the-rejection-bound-saturate).
  Hypothesis: the saturation record's five-state campaign *dominates* the
  permit-pair record's oracle, because a campaign that reaches all five
  saturation states has necessarily observed both bounds binding in both
  classes. It does **not** dominate the parked-handler record, which is a claim
  about *duration* rather than about *count*: a campaign can saturate every pool
  with fast handlers and never produce a handler that outlives
  `lifecycle_callback_deadline`. Conversely the parked-handler state is the
  cheapest way to reach task saturation, since `tests/dispatch.rs:295` already
  parks a handler, so the two are cheapest to build together even though neither
  dominates the other.

---

## Group F: composite route ownership and panic containment

Two records on `crates/host-runtime/src/composite.rs`, the static three-child
composition every production host runs. **Both were carried into this sub-part
from the superseded pre-refactor sub-part `part-2b-wire-and-channels`**, where
they were records 10 and 11 of `_lenses/lens-c-negotiation-provider.md`. See
`part-2b-wire-and-channels/README.md` (source-tree only, not migrated)
for that directory's disposition.

They were orphaned rather than retired, and the mechanism was a route that was
recorded and then not walked. The re-scope retired the `wire-and-channels` label,
moved `composite.rs` into this sub-part's scope, and named carrying these two
forward as one of this sub-part's three attention focuses. That did not happen.
This sub-part's two lens passes went to dispatch, control decode, routing and
handler concurrency: all fourteen records above carry the `req-a-` prefix, and
neither composite property appears among them. `composite.rs` appears in the rest
of this catalog only in the scope sentence and in two test-inventory notes
recording that the file has no test module of its own. So the scope moved, the
absorbing sub-part's lenses did not re-derive these properties, and the two sat
uncovered.

**This group sits after the relationship map because it was carried in a later
pass, and the relationship map above does not cover it.** No dominance relation
is claimed between these two and the fourteen. One relationship is worth stating
and is not a dominance claim: the first record's subject is the route map that
`handle` consults, and `handle` is the composite's leg of the same dispatch path
[req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame](#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame)
governs one layer up. A leaked map entry does not break that record's
at-most-one guarantee; it routes a *reused* handle to a stale child, which is a
different failure with the same input.

**Why these two were the cheapest salvage in that directory.** `composite.rs` is
byte-identical between the lens-era commit and `HEAD`: `git rev-parse` returns
blob `6858246d` for `crates/host-runtime/src/composite.rs` at `1c193ae0`, `793a973e`
and `e447c927` alike, and `wc -l` gives 390 at all three. Their existing check,
`tests/composite_routing.rs`, is likewise blob `2201b830` at all three commits at
1,049 lines. Both records' `composite.rs` citations were re-verified line by line
at carry time and **every one holds**.

**Citations repaired at carry time, per METHOD rule 1.** One, in the second
record. Its `Existing check:` cited
`tests/composite_routing.rs:1028-1060` for the optional-child health panic on the
tertiary child. The file is 1,049 lines, so `:1060` overruns the end of the file
by eleven lines; the test is
`a_panicking_synapse_health_reports_failing_without_unwinding`, whose
`#[tokio::test]` is at `:1028`, whose `fn` is at `:1029`, and which ends at
`:1049`, the last line of the file. The corrected span is `:1028-1049`. This is
the one drift the earlier triage did not predict: it recorded that both records'
subjects and both existing checks were byte-identical and concluded that "neither
needs a citation refresh", which is true of the file contents and false of this
one span, because the span was already wrong when the lens wrote it rather than
made wrong by a change. Everything else in both records verified unchanged.

**Reachability for both rests on one chain, re-verified at carry time rather
than inherited.** The production binary is
`crates/daemon/src/bin/eidnara_host/serve.rs`, which constructs
`StaticComposite::new(...)` at `:575` and passes that value to `host_runtime::run` at
`:632`; both lines were re-printed here. `composite.rs` contains **zero `#[cfg]`
attributes**, verified by grep, so no part of the file is gated. That is a
stronger statement than the equivalent for most files in this sub-part, and it is
consistent with the check inventory's note that `composite.rs` has no test module.
Fact 1 of the [Reachability](#reachability-admission-and-dispatch) section establishes the routed path
that reaches `handle` and `route_gone`. The one asymmetry worth naming is inside
the surface rather than at its edge, and it is the second record's subject: the
primary child's `health` at `:312` is *not* wrapped, while the two optional
children's are at `:318` and `:321`.

### composite-route-entry-is-removed-by-exactly-one-route-gone

Type: safety
Reachability: test-only - the composite is composition-dependent state, and the
binary that composes it in production, `crates/daemon/src/bin/eidnara_host/serve.rs`,
is not in this tree (the daemon is scheduled for U4, `docs/properties/README.md:52`);
a repo-wide search for `StaticComposite::new` finds only `crates/host-runtime/tests/`
and the two examples. Once composed, its `bind`, `handle` and `route_gone` are the
composite's leg of the routed path Fact 1 of
[Reachability](#reachability-admission-and-dispatch) establishes, and the route map they share
(`composite.rs:112`, initialized at `:134`) is plain `Mutex<HashMap<..>>` state
with no gate. `composite.rs` has zero `#[cfg]` attributes. The `serve.rs` line
citations elsewhere in this record are source-repository evidence.
Status: active
Exercised: partial - one rejected-bind case is covered; panic and
close-wins-bind are not.
Guarantee: Every route-map entry the composite inserts is removed exactly once,
so the map's size is bounded by the set of live plus closing routes.
Check: `always` - for every `RouteHandle` the composite inserts, the number of
removals is exactly one, and no removal precedes the owning child's `route_gone`
returning. Per-handle accounting is the primary oracle; total map size is a
cheap screen, since an insert and an unrelated remove cancel in the total.
Fault/timing angle: the removal is deliberately after the child callback
[composite.rs:297-303], so `handle` for a handle mid-`route_gone` still resolves
to the correct child [277-287]. That window is intentional and already covered.
Required faults and enabling state: the three non-success bind outcomes the
comment at composite.rs:262-265 names - a `BindOutcome::Reject`, a panicking
`bind`, and close-wins-bind - each of which must still produce exactly one
`route_gone`. The insert at composite.rs:266-269 happens before the `await` at
271-273, so a panicking `bind` leaves the entry behind and the host's route-gone
obligation is the only thing that reclaims it.
Confidence: high - [evidence](evidence/composite-route-entry-is-removed-by-exactly-one-route-gone.md).
Read the insert, the removal, and the unmapped arms of `handle` [282-285] and
`route_gone` [295]. The unmapped `route_gone` returns without touching the map,
so a spurious callback cannot remove another handle's entry. Every citation in
this record was re-verified line by line at carry time and none needed repair.
Two additions from that pass, both strengthening the record rather than changing
it. The **at-most-one** half now has a named enforcer on the runtime side:
`run_route_gone` short-circuits at `dispatch.rs:1256-1258` when
`registry.mark_gone_started` (`routing.rs:377-390`) reports the flag already set,
returning without invoking the child callback at all, so the composite's removal
returning without invoking the child callback at all, so the composite's removal
statement at `:299-302` cannot run twice for one handle. **The at-least-one half
has three
exceptions, all fatal-latched, listed in the open questions below.
Existing check: `tests/composite_routing.rs:485-531` pins exactly one
`route_gone` for a rejected bind;
`tests/composite_routing.rs:532-600` pins that a closed handle cannot dispatch
to stale child ownership. Both run in CI in this tree (`ci.yml:118`, `:126`); the
unnamed-binary status in [existing-checks.md](request-path/existing-checks.md) is the
source repository's. Status unaudited. Both spans
re-verified at carry time: `rejected_broca_bind_gets_exactly_one_broca_route_gone`
has its attribute at `:485` and its `fn` at `:486`, and
`a_closed_route_handle_cannot_dispatch_to_stale_child_ownership` has its attribute
at `:532` and its `fn` at `:533`.
Impact: a bind path that never yields `route_gone` leaks one map entry per
connection for the host's lifetime, and the leaked entry keeps routing a reused
handle to a stale child.
Open questions:
- Does the host guarantee `route_gone` after a panicking `bind`, or only after
  `Reject` and close? The comment claims all three; the runtime side is outside
  this lens. **Resolved at carry time, and the answer is yes.** The runtime side
  is `dispatch.rs`, which is inside *this* sub-part's scope rather than outside
  it, so the question was answerable here and was not asked. A panic in `bind`
  propagates out of the spawned task, because `panic_boundary::redact_sync`
  (`panic_boundary.rs:52-55`) only marks the panic-hook depth and does not
  `catch_unwind`; `lifecycle_join` observes `is_panic()` at `runtime.rs:187`,
  trips the fatal latch at `:192-193`, and returns
  `Err(LifecycleFailure { stopped: true })` at `:194`; and `dispatch.rs:1164`
  matches that arm and calls `run_route_gone` at `:1166`. All three outcomes the
  comment at `composite.rs:262-265` names do produce exactly one `route_gone`.
- **A new question, opened by that resolution.** There are three further bind or
  close outcomes the composite's comment does not name, and on each the map entry
  is never removed: `dispatch.rs:1174`, where the bind is still executing past
  `lifecycle_callback_deadline` and the comment at `:1171-1173` deliberately
  declines to run `route_gone`; `dispatch.rs:1440-1444`, where a dispatch task did
  not stop before route-gone and the function returns before the `run_route_gone`
  at `:1446`; and `run_route_gone` returning `false` at `:1276`, where the child's
  own callback did not return. All three trip the fatal latch, so the leak is
  bounded by a terminating incarnation rather than by the host's lifetime, which
  is a weaker bound than this record's `Impact:` assumes but not an unbounded one.
  Is that bound intended as the answer, or should the composite's map be dropped
  wholesale on a fatal latch? (needs human input)

### composite-panic-containment-covers-only-optional-health-and-shutdown

Type: safety
Reachability: test-only - same composition dependency as the record above: no
in-tree production caller constructs `StaticComposite`, and `serve.rs` is not in
this tree. Every one of the eleven child call positions enumerated below is an
unconditional statement in `composite.rs`, which has zero `#[cfg]` attributes, so
once composed none of the contained or uncontained sites is gated.
Status: active
Exercised: partial - both contained categories have dedicated tests; no test
pins that the other categories deliberately escalate.
Guarantee: A child panic is contained exactly where the composite can still
serve the host without that child, and escalates to the runtime's fatal cell
everywhere else; the set of contained call sites is closed.
Check: `always` - a panic in an optional child's `health` yields a `Failing`
report for that child and the primary's report still decides the aggregate; a
panic in any child's `shutdown` still drains every remaining child; and a panic
in any other child callback reaches the runtime.
Fault/timing angle: `catch_child_panic` wraps each individual poll
[composite.rs:160-171], so a child that panics after an `await` is still caught.
`shutdown` collects notes and re-raises one aggregate panic only after all three
drains [composite.rs:370-388], which is what keeps the instance fence held until
every child's background work has stopped.
Required faults and enabling state: a panicking child in each of the nine
uncontained positions listed in O17, plus the two contained ones. The primary's
`health` at composite.rs:312 is the one asymmetry a test should pin explicitly,
because the surrounding comment [306-311] only discusses optional children.
Confidence: high - [evidence](evidence/composite-panic-containment-covers-only-optional-health-and-shutdown.md).
Enumerated every child call in the file and checked each for a
`catch_child_panic` wrapper. This is deliberately a *containment* property and
does not restate part 2a's
`every-callback-invocation-is-inside-the-redaction-guard`, which is about the
redaction hook rather than about unwinding. The enumeration was re-derived
independently at carry time and O17's count of nine uncontained positions is
confirmed exactly: `install_connection_key` (`:194-196`), `manifest`
(`:201-203`), `resources` (`:211-213`), `initialize` (`:223-225`), `activate`
(`:235-237`), `bind` (`:271-273`), `handle` (`:279-281`), `route_gone`
(`:292-294`), and the primary's `health` (`:312`). The two contained positions are
the optional children's `health` (`:318`, `:321`) and all three `shutdown` calls
(`:374`, `:378`, `:382`).
Existing check: `tests/composite_routing.rs:851-885` and `:886-917` cover
shutdown panic and error; `tests/composite_routing.rs:986-1027` and `:1028-1049`
cover optional-child health panics;
`tests/composite_routing.rs:918-985` covers the non-graceful incarnation. All
run in CI in this tree (`ci.yml:118`, `:126`); the unnamed-binary status in
[existing-checks.md](request-path/existing-checks.md) is the source repository's. Status unaudited. **One citation
repaired at carry time:** the last of the health-panic spans is `:1028-1049`, not
`:1028-1060`. The file is 1,049 lines, so the lens's end bound overran it by
eleven; the test is `a_panicking_synapse_health_reports_failing_without_unwinding`
(attribute `:1028`, `fn` `:1029`) and it ends on the file's final line. The other
four spans verified exactly.
Impact: adding a `catch_child_panic` to a callback the runtime treats as fatal
would silently convert a host-fatal invariant break into a degraded mode;
removing one from `shutdown` would release the instance fence with a child's
work still live.
Open questions: None.


## Sub-part 2f catalog: runtime assembly and the configuration contract

Scope: what is constructed, in what order, with what defaults, and what a
misconfiguration does. Five files, 3,246 lines, all re-derived with `wc -l` at
`HEAD`: `crates/host-runtime/src/runtime.rs` (1,344), `harness_closure.rs` (1,122),
`config.rs` (674), `lib.rs` (87), `file_mode.rs` (19).

Production and test halves: `runtime.rs` production is `1-1298` with a 46-line
test module at `1299-1344`; `config.rs` production is `1-462` with its tests at
`463-674`. `harness_closure.rs`, `lib.rs`, and `file_mode.rs` have **no test
module at all**, which for a 1,122-line security-relevant filesystem module is a
finding in its own right and is carried in
[existing-checks.md](runtime-config/existing-checks.md).

Boundary context, read but not mined: `connection.rs` is Part 2a's file and is
cited only as the consumer of four configured deadlines (`:125`, `:145`, `:158`,
`:177`, `:279`). `dispatch.rs`, `routing.rs`, and `handler.rs` are sub-part 2e's.
`crates/daemon/src/bin/eidnara_host/serve.rs` is outside the crate and is cited
because it is the **sole production `HostConfig` construction site and the only
non-test caller of `runtime::run`**; without it no reachability label in this
sub-part could be justified.

**This is a post-refactor surface, and unusually for this catalog it is a clean
one.** Grepping all five files for `tcp_frame_channel`,
`transport_negotiation`, `transport_provider`, `provider_recovery`,
`frame_read`, `shm_provider`, `negotiate`, `Serveable`, `transport selection`,
and `fallback` returns **zero hits**, so **no documentation or comment in this
sub-part describes a deleted mechanism**. `lib.rs` is the file most likely to
hold a stale reference, since it is the module manifest the refactor edited, and
it is clean: it declares `ring_transport` and `setup_socket` as `#[doc(hidden)]
pub mod` (`:20-21`, `:34-35`) and names no deleted module.
`config.rs:213`'s `transport_setup_deadline` survives and still names a live
mechanism, the mandatory ring setup of protocol Section 7.7. Four commits carry
the refactor:

| Commit | Subject |
| --- | --- |
| `0f336d3c` | `refactor(shm): collapse to fixed ring transport` |
| `d8bde128` | `feat(host): add authenticated ring setup socket` |
| `793a973e` | `build(shm): require packaged native transport` |
| `ed487e11` | `refactor(host): make ring transport mandatory` |

Two residuals of the opposite shape, recorded so a later pass does not miscount
them as stale. Neither describes deleted work. **One of the two was described
here as a forward reference to unbuilt work and that was wrong**, and the
correction is applied rather than footnoted, because the construction
conditionality map below leaned on it.

1. `runtime.rs:3-5` says signal acquisition stays outside the crate and that
   "future production wiring in the source module-host work will map SIGINT/SIGTERM".
   The first clause is true and the second is **stale**: the production wiring
   already exists, outside this crate. `serve.rs:617-622` installs a `SIGTERM`
   stream and a `SIGINT` stream, failing startup if either installation fails,
   and `:623-631` spawns a task that cancels the caller-supplied
   `CancellationToken` on the first of the two to arrive. All of that is
   **before** `host_runtime::run` is entered at `:632`, and the comment at
   `serve.rs:604-616` states the ordering requirement explicitly: creating the
   stream inside the spawned task would race `run`, and a signal arriving before
   registration would take the default disposition and kill the daemon outright.
   So the correct statement is that the crate does not acquire signals and its
   sole production caller does, for both `SIGINT` and `SIGTERM`. What this
   changes for the catalog is not a label but a *producer*: shutdown-token
   cancellation at an arbitrary point in `run` is an operator-reachable
   production event, not a test-only injection. Consequences are worked through
   at the end of the construction conditionality map.
2. `config.rs:5-6` says CLI and config-file exposure belongs to
   the source daemon-lifecycle work, which is the reason the configuration contract is doc
   comments. This one is a genuine forward reference and stands.

Provenance: source catalogs at `host@39e823037`; see [../README.md](../README.md). System
`the `host` source checkout, branch
`feat/shared-memory-release-gate-audit`, `HEAD` = `e447c927`, confirmed with
`git log -1`. Both lens agents read and verified their line references at that
commit. Scope and CI findings come from
`part-2-rescope/scope-map-and-risk-ranking.md` (a source-tree artifact that was not migrated into this repository).

**Where lens B re-derived a citation lens A made, lens B's line numbers and
figures win.** Three differences, all verified again by this synthesis by
printing the lines, plus one cross-part correction of its own.

- **The forced-shutdown total is a per-branch ceiling of about 100 seconds, not a
  floor, and the word "floor" was wrong wherever this catalog used it.** Lens A's
  observation O3 states the mechanism correctly - `runtime.rs:1148` computes one
  absolute deadline, `:1214`'s `timeout_at` is entered only when it has already
  expired, and `:1223` then arms a *fresh* `lifecycle_callback_deadline
  .saturating_mul(2)` awaited at `:1224` - and then reads the total as 60 s
  added after 10 s. Lens B composed the whole sequence and reported about
  **100 seconds** counting the drain (`:1200`, 10 s), the doubled chain
  (`:1223-1224`, 60 s), and `run_handler_shutdown` (`:1240`, 30 s at `:1276`),
  and about **160 seconds** counting one of the two `force_close_all_routes`
  calls (`:1206`, `:1216`) that no timeout wraps. **Lens B's arithmetic is
  right and its label is wrong.** Each term is that stage's *maximum*, and each
  stage returns as soon as its awaited future resolves: `timeout(lifecycle_chain,
  tracker.wait())` at `:1224` returns the instant the tracker drains, and
  `run_handler_shutdown` returns the instant the callback task joins. A sum of
  selected per-stage maxima is a **ceiling on the branch that visits every one of
  those stages**, and the true floor of the forced path is the drain deadline
  plus however long the surviving work actually takes, which can be
  milliseconds. Calling 100 s a floor claims every forced shutdown takes at least
  that long, which is false and is the opposite of the defect: the defect is that
  a *ceiling* the operator was told is 10 s can reach roughly ten times that. The
  corrected figures and terminology are used below, per branch, and the record's
  `Check:` line is rewritten to match rather than left verbatim.
- **`activation_in_progress` is `runtime.rs:1051-1071`, not `:1051-1074`.**
  Lens A cited the wider span in two places.
- **`config.rs` carries 10 in-crate tests and `runtime.rs` 1, for 11 in the
  sub-part.** Lens A did not count them; lens B enumerated the sites. Used
  throughout.
- **This synthesis corrects one cross-part citation of its own.** Part 2a's
  catalog cites `config.rs:296` for `liveness: None` in two places (its
  reachability section and its Group K preamble). The line is **`config.rs:294`**
  at `HEAD`, printed and confirmed as `liveness: None,` inside
  `HostConfig::default`. Lens A of this sub-part cites `:294` and is right. The
  correction changes nothing about 2a's conclusion; it is recorded because the
  cross-part settlement below leans on that exact line.

## What this part is about

Six facts frame every record here. The first is the artifact siblings depend on.
The second settles a question three parts have left open. The third is the
recurring shape this catalog has now found twice. The fourth is why the
configuration contract cannot be checked. The fifth is the one-sentence verdict
on the defaults. The sixth is the coverage position, which is the weakest of the
three sub-parts.

### The construction conditionality map

Reproduced in full, because sibling sub-parts depend on it for their reachability
labels. **This map was rebuilt after an independent evaluation refuted two of its
rows, and the headline it previously carried - "only three things are conditional"
- was one of the casualties.** The corrected answer is that four things are
conditional on a config key, a `#[doc(hidden)]` entry point, or a cancelled
token, **and one whole tail of the sequence is conditional on something none of
those categories covers: whether the `run` future is still being polled.**
Nothing is `cfg`-gated, which was the map's other headline and which survives.
`run` (`runtime.rs:630`) delegates to `run_with_publish_hook` (`:641`).
"Unconditional" means reached on every path that gets that far, with no config
key, feature flag, or `cfg` gating it, **and, for every row from 19 onward, on
the additional condition that the caller keeps polling `run`.**

| # | Component or step | Site | Condition |
| --- | --- | --- | --- |
| 1 | Process panic hook | `:647` | **Unconditional, and before config validation.** `Once`-guarded inside `panic_boundary::install` (`panic_boundary.rs:39`), so idempotent across repeated `run` calls |
| 2 | `HostConfig::validate` | `:648` | Unconditional. First rejection point |
| 3 | `InstanceGuard::acquire` retry loop | `:656-679` | Unconditional. 4 attempts, 25 ms apart (`instance.rs:674-675`), so a 75 ms budget; not configurable |
| 4 | `Starting` lifecycle record | `:680-682` | Unconditional |
| 5 | `Arc::new(handler)` | `:684` | Unconditional |
| 6 | `install_connection_key` | `:685` | Unconditional. Handler learns the auth key before any listener exists |
| 7 | `manifests()` / `resource_declarations()` | `:686-687` | Unconditional, both inside `redact_sync` |
| 8 | `TargetIndex` + `Reservations` | `:688` via `build_target_index` (`:496`) | Unconditional. Refuses 0 or >3 manifests (`:500`), mismatched declaration count (`:506`), duplicate module id (`:526`), class/reservation disagreement (`:535-554`), unsupported or duplicate role (`:588`, `:592`), no routable role (`:599`), and a manifest set with no `tool_provider` (`:610-617`) |
| 9 | Reservation feasibility gates | `:693`, `:698`, `:708` | Unconditional |
| 10 | `CatalogCache::new_bounded` | `:718` | Unconditional, bounded during serialization at `MAX_BODY_LEN` |
| 11 | Resident-byte floor gate | `:733-740` | Unconditional. **Load-bearing for step 18** |
| 12 | `initialize` callback | `:752` | Unconditional, `AbortOnDropHandle`, raced against `shutdown` (`:756-764`) and `lifecycle_callback_deadline` (`:761`) |
| 13 | `PrePublicationCleanup` | `:826` | Unconditional once initialization returned `Ok` |
| 14 | Setup socket bind | `:836` | **Skipped if `shutdown` already cancelled** (`:831` returns `Ok(None)`) |
| 15 | Publication + `Running` record | `:842`, `:847-849` | Same condition as 14. The `Running` write is best-effort; its failure is discarded |
| 16 | `process_limits(max_connections)` | `:872` | Unconditional. Checked multiplication; overflow is `InitFailed` |
| 17 | `RingTransport` | `:876` | **Unconditional.** Confirms Part 2b's finding at the same line, unchanged at this commit |
| 17a | `ring.set_publish_hook` | `:879-881` | **Conditional and test-only.** Reachable only through `run_with_publish_hook`, which is `#[doc(hidden)]` (`:640`); `run` passes `None` (`:635`) |
| 18 | `HostShared` | `:882-927` | Unconditional. Contains the unchecked ingress subtraction (`:896-902`) |
| 18a | `ingress_budget` | `:896-902` | Unconditional. `max_resident_bytes − EGRESS − SCRATCH − catalog − retained`, **unchecked**; step 11 is its only guard |
| 18b | `scratch_budget`, `egress_budget` | `:903-904` | Unconditional, fixed constants, **never derived from config** |
| 18c | `pending_permits`, `task_permits` | `:905-910` | Unconditional. Configured limit minus the reserved carve-out |
| 18d | `reserved_pending_permits`, `reserved_task_permits` | `:911-912` | Unconditional **construction**, zero-permit when no module declared a reservation, and then never entered |
| 18e | `health_snapshot` | `:889-893` | Unconditional. Seeded `Degraded` with `components: {}` |
| 18f | `liveness` | `:886` | Unconditional **copy**; the subsystem it feeds is conditional (see 22) |
| 18g | `shutdown_latch`, `tracker`, `AbortRegistry` | `:915-919` | Unconditional |
| 19 | `AbandonGuard` | `:929-931` | Unconditional **construction**, and it is the switch every row below depends on. Armed with `Arc::clone(&shared)` and the `InstanceGuard`; `disarm` at `:937` is reached only if `:936` returned |
| 20 | Activation task | `:932` | **Unconditional.** Tracked, abort-exempt, not awaited by startup |
| 21 | Health task | `:933` | **Unconditional.** No config key disables it. `liveness: None` does not suppress it |
| 22 | Liveness loop | `connection.rs:279-284` | **Conditional on `shared.liveness.is_some()`**, which is `None` by default (`config.rs:294`) and `None` in production (`serve.rs:593`) |
| 23 | Accept loop | `:934` | Unconditional. 100 ms fixed `ACCEPT_ERROR_BACKOFF` (`:965`), not configurable |
| 24 | Setup-socket unlink | `:935` | **Conditional on `accept_loop` returning to a live poller**, result discarded |
| 25 | `shutdown_sequence` | `:936` | **Conditional, and this row previously said "Unconditional", which was wrong.** It is reached only if the caller polls `run` past `:934`. If the `run` future is *dropped* at any point after `:929` - a supervisor aborting the task, or a `select!` arm losing - the guard's `Drop` (`:419-476`) runs the teardown instead and `shutdown_sequence` never executes. The two paths are not equivalent: the drop path cancels the token and every generation's three tokens (`:424-434`), calls `abort_all` (`:435`), demotes the phase (`:442`), and then spawns a task that runs `force_close_all_routes`, `tracker.close()`, an **explicitly unbounded** `tracker.wait()` (`:457`, comment at `:452-456`), `run_handler_shutdown` (`:467`), and a **second** unbounded `tracker.wait()` (`:471`) before dropping the lock. So it performs no graceful drain, sends no connection Goodbyes, honours no `shutdown_deadline`, and is bounded by nothing at all. `run_handler_shutdown`'s once-latch (`:1265-1270`) is what keeps the two paths from double-running the handler callback, and its own comment at `:1260-1264` names this exact interleaving, which is direct evidence the drop path is a live concern rather than a theoretical one |
| 26 | `abandon_guard.disarm` and the ordered handler-then-lock drop | `:937-951` | Same condition as 25. On the graceful branch `shared` and `guard` drop in order (`:944-945`); on the non-graceful branch `retain_lock_until_drained` (`:951`) takes both |
| - | `HarnessClosureStore` | - | **Never constructed by this crate.** Zero Rust references to `HarnessClosureStore`, `ClosureCandidate`, or `HarnessClosureStore::open` anywhere under `crates/host-runtime/src`. Its production constructor is `serve.rs:162` and `:349`, in `daemon`, and both discard the error with `.ok()` |

Four conclusions siblings can rely on. The first three are the map's original
three, two of which survive verification unchanged and one of which is narrowed.
The fourth is new and is the reason this map was rebuilt.

1. **Nothing in the host runtime is feature-gated or `cfg`-gated.** Verified
   again and unchanged. The conditional construction in the sequence is
   `set_publish_hook` (test-only), the setup-socket bind and publish pair
   (skipped on an already-cancelled token), the per-connection liveness loop, and
   the polled-future tail described in conclusion 4. None of those is a `cfg`.
2. **The activation task and the health task are unconditional.** Unchanged. A
   record about either is `default-production` regardless of configuration. Both
   are spawned at `:932-933`, above the abandon window's practical effect, and
   neither is gated.
3. **The liveness loop is not reached in production.** Unchanged. Not merely
   `explicit-config-only`: the sole non-test caller of `run` never sets the
   policy.
4. **The graceful shutdown sequence is conditional on the `run` future being
   polled to the end, and the alternative path is unbounded.** New. Rows 24
   through 26 are reachable only if nothing drops the future, and
   `AbandonGuard::drop` is a second, structurally different teardown that no
   record in this catalog describes. This is queued as a gap rather than mined
   here.

**Which dependent labels this rebuild puts back in question, and whether any
actually changes.** Stated explicitly because METHOD rule 4 exists precisely
because a blanket reachability claim in a preamble has already cost one revision,
and this map *is* that preamble for four sub-parts. Each dependent claim was
re-derived rather than assumed:

- **Part 2a's three `explicit-config-only` liveness records: unchanged.** They
  rest on conclusion 3, which is untouched by either error. Re-verified:
  `config.rs:294`, `connection.rs:279`, `serve.rs:593`.
- **2b's and 2d's citation of `RingTransport` at `:876` as unconditional:
  unchanged.** Row 17 is above the abandon window and carries no condition.
- **2e's citation of the map for "nothing is `cfg`-gated"
  (`../part-2e-request-path/catalog.md:406-407`): unchanged.** That is
  conclusion 1, which survives.
- **2e's citation of the shutdown-sequence composition
  (`../part-2e-request-path/catalog.md:690-698`): still true, now incomplete.**
  It cites `shutdown_sequence` calling `force_close_all_routes` twice and `run`
  returning after `run_handler_shutdown`, to argue its own open question is
  answerable only from `runtime.rs:1144-1244`. Every fact it cites is correct.
  What it did not know is that there is a *second* call site of
  `force_close_all_routes`, at `:450` on the drop path, so the census that
  question needs is three sites rather than two. Its `medium` confidence and its
  open question both stand, which is why this is an addition and not a
  correction.
- **This sub-part's own fourteen labels: all unchanged, and two are strengthened
  rather than moved.** No record in this catalog is labelled on the ground that
  signals do not exist or that shutdown is unconditional, which is why the
  thirteen `default-production` and one `explicit-config-only` survive intact.
  The strengthening is in the justification, not the label:
  [rt-a-an-initialized-handler-drains-without-publishing](#rt-a-an-initialized-handler-drains-without-publishing)
  gives its cheapest entry as "cancel the shutdown token between the return of
  `initialize` and the `is_cancelled` check at `:831`", and the corrected residual
  above supplies a production producer for exactly that - an operator `SIGINT`
  during startup - where before it read as a test-only injection. The same
  applies to
  [rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline](#rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline),
  whose enabling state now has a production trigger rather than only
  `tests/lifecycle.rs`. Both were already `default-production`, so **the net
  effect on the reachability distribution is zero.**

**So the answer to "did rebuilding the map move a label" is no.** Two of the
map's rows were wrong, one of its three headline conclusions was wrong, and a
whole teardown path was invisible; and none of that reaches a
`Reachability:` line, in this sub-part or in the three that cite the map. That is
worth stating rather than leaving implicit, because the natural assumption on
being told the map is unreliable is that the labels resting on it must move. They
do not, and the reason is structural: the labels depend on the map's `cfg` and
config-key conclusions, and both errors were about *control flow* - who calls,
and who keeps polling - which no reachability class in METHOD's three-way
vocabulary encodes.

`RingTransport` at `:876` deserves separate emphasis because three sibling
sub-parts cite it: it is built **unconditionally**, printed and confirmed here as
`let ring = Arc::new(crate::ring_transport::RingTransport::for_ring_profile(`.
The ring is not optional, not selected, and not gated, which is the same verdict
2b and 2d reached against the `RING_PROFILE = "host-test-ring-v1"` name and
the "Thread-confined peer endpoint for integration tests" doc comment.

### This sub-part settles a cross-part question

**Liveness is `None` by default and production never overrides it, so no ping is
ever sent and Part 2a's liveness records are unreachable in production.** Stated
plainly because three parts have carried it as an open question and the answer is
now determined by a line outside the crate.

The chain is three facts, each verified. `HostConfig::default` sets
`liveness: None` at `config.rs:294`, printed and confirmed. The liveness loop is
spawned only under `shared.liveness.is_some()` at `connection.rs:279`. And the
sole production `HostConfig` construction, `serve.rs:582-593`, overrides only
`max_resident_bytes` and then falls through to `..HostConfig::default()` at
`:593`, printed and confirmed, so the production host inherits `None`. No other
non-test caller of `runtime::run` exists.

Part 2a labelled its three liveness-dependent records `explicit-config-only` on
the narrower ground that nothing *in this crate* opts in
(`part-2a-host-lifecycle/catalog.md:46-51`). This sub-part strengthens that to
the production claim, and the three records it applies to are exactly the three
`explicit-config-only` records in 2a's catalog, confirmed by enumerating that
catalog's `Reachability:` lines:

- [a-timely-pong-sustains-the-generation-within-a-bounded-round](#a-timely-pong-sustains-the-generation-within-a-bounded-round),
  2a's liveness record proper.
- [slow-egress-alone-does-not-retire-a-probed-generation](#slow-egress-alone-does-not-retire-a-probed-generation).
- [a-setup-pong-is-required-and-forbidden-in-the-same-window](#a-setup-pong-is-required-and-forbidden-in-the-same-window),
  **the pong pre-answer record**, whose enabling state its own
  `Required faults and enabling state:` line gives as `liveness: Some(..)` in
  `HostConfig`.

So all three are reachable only from `tests/lifecycle.rs:402` and
`tests/client.rs:64`, and a production incarnation cannot enter any of them. Two
consequences follow and both belong on the record. First, the host has **no
application-level liveness detection at all** by default: a silently wedged peer
is discovered only by the ring's own path, which Part 2d established shares one
code (`eof`) with a clean host exit. Second, `invalidate_on_missed` is the one
flag whose only `true` value in the repository is in a test
(`tests/client.rs:67`), against a comment at `config.rs:236-238` saying it must
stay `false` until the source module-host work, so the code path the stated policy
forbids is the only one exercised.

### Two fixed bounds judge configurable ones

Both are the shape Part 2a found with its 60-second freshness window, where a
hardcoded value governs an operator-settable one and the fixed value wins. **Two
recurrences in one sub-part, in the same direction, makes this the catalog's
second-most repeated finding after the success-shaped error path.** Part 2a's is
[phase-evidence-outlives-a-long-phase](#phase-evidence-outlives-a-long-phase),
where the record is written once per phase transition and compared against a
fixed, non-configurable 60-second window while the frame and lifecycle deadlines
are settable to 365 days.

**First, a hardcoded 50 millisecond probe interval replaces the configured health
interval whenever a handler-authored string reports a starting state, so the
handler sets the host's probe rate, unbounded.** `runtime.rs:1129-1133`, printed
and confirmed:

```
let interval = if activation_in_progress {
    Duration::from_millis(50)
} else {
    shared.timing.health_interval
};
```

The predicate `activation_in_progress` (`:1051-1071`) walks the report's own
metrics and returns true when any component's `metrics.storage_state` or
`metrics.synapse_state` equals the string `"starting"`. That report is handler
output: `HostHandler::health` returns a `HealthReport` (`handler.rs:591`) whose
`metrics` field is `Option<serde_json::Value>` (`:194`), entirely
handler-authored. So a handler that keeps reporting `starting` moves the host
from its configured `health_interval` - 30 s by default (`config.rs:229`),
settable to 365 days (`config.rs:81`, `:360`) - to a hardcoded 50 ms, a
600-fold increase, for as long as it keeps reporting that string. Nothing caps
the duration, counts the fast probes, or re-reads the configured value while the
fast path is active. Two aggravating details: the 50 ms is a bare literal rather
than a named constant, so it is invisible to anyone reading `HostTiming`; and
each probe invokes the handler's `health` callback under `lifecycle_join`
(`:1117`), where an overrun is host-fatal (`handler.rs:554-556`), so the fast
path raises callback frequency 600-fold and keeps every invocation on a
fatal-if-slow path. The design intent is legible - fast polling during
activation is how the host notices storage becoming ready, and protocol `:596`
requires the host to stay usable while storage opens - but the trigger is
untrusted input from the component being probed, and the knob is silently
overridden rather than clamped.

**Second, a doubled callback deadline is armed after the shutdown deadline
already expired, so 10 seconds configured admits a ceiling of about 100 seconds
on one branch, and no finite ceiling at all against a callback that never
yields.** `runtime.rs:1223`, printed and confirmed as
`let lifecycle_chain = shared.timing.lifecycle_callback_deadline.saturating_mul(2);`,
armed at `:1224` with a fresh `timeout(...)` rather than a
`timeout_at(deadline, ...)`. The ordering is what makes it a finding: `deadline`
is computed once at `:1148` as `Instant::now() + shutdown_deadline`, the drain at
`:1200` consumes it, and the `timeout_at` at `:1214` is entered only when the
deadline is already in the past.

`shutdown_sequence` has **three exits, not one**, and they carry three different
bounds. Reading them separately is what the earlier single-figure account missed,
and it is what the record's `Check:` line now asserts. At defaults
(`shutdown_deadline` 10 s, `config.rs:228`; `lifecycle_callback_deadline` 30 s,
`:225`):

| Stage | Site | Maximum |
| --- | --- | --- |
| Graceful drain | `:1200` | 10 s (`shutdown_deadline`) |
| `abort_all` + `force_close_all_routes` | `:1205-1206` | **unbounded by `deadline`**; internally 30 s (`dispatch.rs:1434`) then 30 s in `run_route_gone`. Entered only when the drain timed out |
| `timeout_at(deadline, tracker.wait())` | `:1214` | the remainder of `deadline`, so about 0 whenever the drain already consumed it |
| `abort_all` + `force_close_all_routes` again | `:1215-1216` | same shape |
| Doubled lifecycle chain | `:1223-1224` | 60 s (`2 x lifecycle_callback_deadline`) |
| `run_handler_shutdown` | `:1240` or `:1243` | 30 s (`lifecycle_callback_deadline`, applied at `:1276`) |

| Exit | Reached when | Ceiling in configured units | At defaults |
| --- | --- | --- | --- |
| `:1243`, graceful | drain finished inside `deadline`, or the tracker drained inside the remainder | `shutdown_deadline + lifecycle_callback_deadline` | 40 s |
| `:1238`, fatal latch | tracker still busy after the doubled chain. **`run_handler_shutdown` is never called on this exit** | `shutdown_deadline + 2 x lifecycle_callback_deadline`, plus two untimed `force_close_all_routes` | 70 s |
| `:1241`, forced with callback | tracker drained inside the doubled chain | `shutdown_deadline + 3 x lifecycle_callback_deadline`, plus two untimed `force_close_all_routes` | 100 s |

Three things follow, and the first is the correction. **The check's previous bound,
`shutdown_deadline + 2 * lifecycle_callback_deadline`, is the bound of the exit
that does the *least* work.** It omits `run_handler_shutdown` entirely, and
`run_handler_shutdown` runs on two of the three exits, at `:1240` on the forced
path and at `:1243` on the graceful one, each time under its own fresh
`timeout(lifecycle_callback_deadline, ...)` at `:1276`. So an oracle written to
the old bound fails on a correct build the moment the handler callback takes any
appreciable time, and it fails for a reason that has nothing to do with the
defect. The bound is now stated per exit.

**Second, no finite ceiling in that table is a real guarantee, and the reason is
mechanical rather than a matter of degree.** Every bound above is a
`tokio::time::timeout` over a future, and a timeout cannot preempt a future that
does not yield. `run_handler_shutdown` calls the handler's `shutdown()` through
`redact_sync` at `:1273` and then awaits the result at `:1274`; a callback that
blocks its worker thread rather than awaiting is never interrupted by the
`timeout` at `:1276`, which is exactly what the function's own doc comment at
`:1256-1258` says - "The callback is never aborted: a deadline overrun trips the
fatal latch and returns non-graceful while the still-tracked task keeps running".
The same applies to the two `tracker.wait()` calls. So the honest statement is
that the configured deadlines bound the *host's own waiting*, not the host's
lifetime, and the 40/70/100 second figures are ceilings **conditional on every
awaited future being cooperatively cancellable**. That condition is unstated
anywhere in `config.rs`.

**Third, the word "floor" is wrong for all of these and was used three times in
this catalog.** Each figure is a sum of stage maxima on one branch. Every stage
returns as soon as its future resolves, so a forced shutdown whose surviving task
drains immediately after the abort exits in milliseconds past `shutdown_deadline`.
The defect is a ceiling ten times the configured knob, not a floor.

Two secondary consequences, both unchanged by the correction.
`HostError::ShutdownDeadlineExpired`'s own doc comment (`runtime.rs:42-44`) says
"Host tasks could not be reaped within the shutdown deadline even after aborts",
and it is returned from an exit whose ceiling is ten times that deadline. And the
client gives up long before: `CLIENT_SHUTDOWN_TIMEOUT` is 5 s (`client.rs:51`,
protocol `:741`), so a correct graceful shutdown presents to a conforming client
as a timeout. The comments at `:1217-1222` and `:1228-1233` argue the trade
explicitly and well - releasing the instance fence while a lifecycle callback
still owns the handler would let a successor start against the predecessor's
in-flight cleanup - and the finding is not that the choice is wrong. It is that
the choice is unbounded by the knob the operator was told bounds it, and the rule
it breaks is stated as `MUST NOT` at protocol `:731`: "Every operation owns one
absolute deadline; per-stage timers MUST NOT multiply it."

A third site multiplies in the same way and is a refinement of Part 2c's finding
rather than a new one: `transport_setup_deadline` is armed twice, serially, at
`connection.rs:158` for `ring.prepare` and again at `:177` for `activate_server`,
so with `auth_deadline` consumed first at `:125` the host's serial pre-service
budget at defaults is 2 + 2 + 2 = **6 seconds** against a documented client
whole-handshake deadline of 2 s (protocol `:737`). Part 2c's
`existing-checks.md:569-575` recorded 4 s, which counts
`transport_setup_deadline` once.

### The configuration contract is doc comments only

**There is no configuration reference document.** Both lenses searched
independently and agree. Every key name in `HostLimits`, `HostTiming`,
`LivenessPolicy`, `HostInit`, and `HostConfig` was grepped across `docs/`, and
**no file names any of them except `max_resident_bytes`**
(`docs/host-wire-protocol.md:423`, and there only to say the cap covers
Synapse parse scratch as a named logical payload). Every other hit is inside
`docs/properties/`, which is this catalog's own working material and is not a
contract. `the historical host performance baseline document:36-38` restates a handful of default
values as a description of one perf run and `:48` explicitly tells a reader to
record the current `HostConfig::default()` rather than copying the old value, so
it is a self-dating snapshot rather than a specification. `config.rs:5-6` says
why this is intentional: CLI or config-file exposure "belongs to the spawn/doctor
integration (the source daemon-lifecycle work), not this crate."

That raises the evidentiary weight of the doc comments in `config.rs` and makes
each contradiction a contradiction with the only available authority. It also
means the protocol specification is being used as a configuration contract it was
not written to be, which is why the "Documented" column below is answered from
the specification's normative statements about the *behaviour* each key controls.

**And all 10 `config.rs` tests prove rejection rather than use.** The ten sites
are `:467`, `:472`, `:502`, `:520`, `:550`, `:564`, `:576`, `:603`, `:636`,
`:646`, and lens B's verdict is the load-bearing one: none proves that an
*accepted* configuration is then used as configured. That is exactly the class
both fixed-bound findings above fall into, and it is not a class a rejection test
can catch.

The table below is lens A's, reproduced in full. Columns: code default; what
`docs/host-wire-protocol.md` says; the bound `validate` enforces; and whether
the key changes host behaviour.

| Key | Code default | Documented | Enforced bound | Takes effect |
| --- | --- | --- | --- | --- |
| `max_handshakes` | 32 (`config.rs:128`) | Value deliberately unspecified: "Slot count is finite deployment policy, not a wire constant" (`:161`) | nonzero, ≤ `Semaphore::MAX_PERMITS` (`config.rs:156-167`) | Yes - `runtime.rs:913` |
| `max_connections` | 64 (`:129`) | "A deployment MAY cap concurrent connections" (`:290`), no value | same | Yes - `:872` and `:914`. **Two consumers**: it also scales every shared-memory resource limit |
| `max_routes` | 1024 (`:130`) | "MAY cap ... routes" (`:290`), no value | same, plus ≤ `u16::MAX` (`config.rs:168-174`) | Yes - `:895` |
| `max_pending_requests` | 1024 (`:131`) | "MAY cap ... pending correlations" (`:290`), no value | same | Yes - `:693`, `:906` |
| `max_handler_tasks` | 256 (`:132`) | "MAY cap ... handler tasks" (`:290`), no value | same | Yes - `:698`, `:707`, `:909` |
| `max_resident_bytes` | 385,942,805 (`:140`, computed) | Described at `:423` as an accounting boundary over named payloads, not an exact RSS claim; no value given | ≥ `MIN_RESIDENT_BYTES` = 318,833,941 (`config.rs:175`), ≤ `min(Semaphore::MAX_PERMITS, u32::MAX)` (`:185-191`), plus the startup floor at `runtime.rs:736` | Yes - `:897`. **Raises only the admission pool**; the egress and scratch slices are constants |
| `writer_queue_frames` | 64 (`:141`) | Not documented for the host. `:742-743` gives 256 data + 32 reserved as *managed client* defaults | nonzero, ≤ `Semaphore::MAX_PERMITS` | Yes - `connection.rs:145` |
| `auth_deadline` | 2 s (`:223`) | "Recommended host default is 2 seconds" (`:159`) - **the only host default the specification states** | nonzero, ≤ 365 days (`config.rs:356-363`) | Yes - `connection.rs:125` |
| `frame_deadline` | 30 s (`:224`) | 30 s appears at `:738` but that table is scoped "Managed Rust and TypeScript client defaults" (`:733`) | same | Yes - `connection.rs:146`, then `ring_transport.rs` |
| `lifecycle_callback_deadline` | 30 s (`:225`) | Not documented | same | Yes - `runtime.rs:184`, `:761`, `:1084`, `:1276`, `dispatch.rs:1140`. **Also doubled at `runtime.rs:1223`** |
| `route_close_budget` | 5 s (`:226`) | "finite close budget" (`:296`, `:691`), no value | same | Yes - `dispatch.rs:1348`, `:1358` |
| `transport_setup_deadline` | 2 s (`:227`) | Not documented as a host key. `:737` gives the client one 2 s deadline covering descriptor transfer and ring attachment | same | Yes - `connection.rs:158` **and** `:177`, armed twice serially |
| `shutdown_deadline` | 10 s (`:228`) | Not documented for the host. `:741` gives 5 s as the *client* shutdown deadline | same | Yes - `runtime.rs:1148`. **Exceeded on the forced path** by `:1223` |
| `health_interval` | 30 s (`:229`) | §9.3 (`:679-685`) describes the probe; no interval | same | Yes - `runtime.rs:1132`, **but only in the `else` branch**; `:1130` substitutes a fixed 50 ms |
| `liveness` | `None` (`:294`) | "A missed Pong invalidates the connection only under host's bounded liveness policy" (`:681`) - absence permitted | if `Some`, both periods nonzero and ≤ 365 days (`config.rs:364-377`) | Yes when `Some` (`connection.rs:279`). **Never `Some` in production** |
| `liveness.invalidate_on_missed` | n/a; `false` at both call sites | Not documented | none | Yes - `connection.rs:830`. `config.rs:236-238` states it must stay `false` until the source module-host work |
| `data_dir` | `None`, so XDG (`:288`) | `${dataDir}/eidnara/...` layout is normative (`:143-147`) | **none** | Yes - `runtime.rs:660` |
| `daemon_ver` | `host-runtime/{CARGO_PKG_VERSION}` (`:289`) | Published as `daemon_ver`, echoed in the proof; `auth.rs:132-133` says it is not an authentication input | nonempty (`config.rs:302`); worst-case auth and connection-file size (`:314-340`) | Yes - `:842`, `:926` |
| `payload_manifest_digest` | SHA-256 of zero bytes (`:84-85`, `:290`) | Required persisted identity; `lifecycle.rs:378` treats an *empty* digest as legacy | 64 lowercase hex (`config.rs:305`) | Yes - `:661` |
| `init.storage` | `None` (`HostInit::default`) | "The handler deserializes it; the host never reads it" (`config.rs:252-253`) | none | Pass-through - moved out at `:751`, handed to `initialize` |
| `init.host_capabilities` | `Vec::new()` (`:250`) | Not documented | none | **No.** Zero readers repo-wide. Only written (`serve.rs:487`, two test sites, both `Vec::new()`) and `Debug`-formatted (`config.rs:262`) |

**Totals in Part 4f's categories.** 21 keys in scope.

- **Documented and matching: 1.** `auth_deadline` at 2 s (`:159` versus
  `config.rs:223`).
- **Documented as policy with the value deliberately left open: 6.**
  `max_handshakes`, `max_connections`, `max_routes`, `max_pending_requests`,
  `max_handler_tasks`, `route_close_budget`. These are not divergences; the
  specification says the host owns the number.
- **Documented behaviour, undocumented value: 3.** `max_resident_bytes`,
  `liveness`, `payload_manifest_digest`.
- **Undocumented but effective: 10.** `frame_deadline`,
  `lifecycle_callback_deadline`, `transport_setup_deadline`,
  `shutdown_deadline`, `health_interval`, `writer_queue_frames`,
  `invalidate_on_missed`, `data_dir`, `daemon_ver`, `init.storage`.
- **Inert: 1.** `init.host_capabilities`.
- **Absent everywhere: 0.** No key is documented that does not exist.
- **Divergent: 0 strictly, 3 by adjacency.** No documented host default
  contradicts its code default. But three host keys sit next to a *client*
  default of a different value in the same specification section, with nothing
  relating them.

The three adjacency cases are the ones that bite, and lens B gave each a verdict:

| Domain | Host default | Client default | Verdict |
| --- | --- | --- | --- |
| Shutdown | `shutdown_deadline` 10 s (`config.rs:228`) | `CLIENT_SHUTDOWN_TIMEOUT` 5 s (`client.rs:51`, protocol `:741`) | **Worst.** The client abandons its close at 5 s while the host is still legitimately draining to 10 s, so a correct graceful shutdown presents to the client as a timeout |
| Authentication | `auth_deadline` 2 s (`:223`) | `CLIENT_HANDSHAKE_TIMEOUT` 2 s (`client.rs:43`), spanning discovery, authentication, descriptor transfer, and ring attach | The host's authentication **stage alone** can consume the client's whole budget. Protocol `:747` names this: the two values "are not independent" and a deployment needing the full host window "MUST raise the client handshake deadline above it" |
| Transport setup | `transport_setup_deadline` 2 s (`:227`) | the same 2 s client handshake | A **second** host stage individually equal to the client's entire budget |

`frame_deadline` is the control case: 30 s on both sides (`config.rs:224`,
`client.rs:45`), the one domain where the two defaults agree, which shows the
divergences are not a systematic offset. `HostConfig::validate` (`:300-379`)
checks each duration for zero and for `MAX_CONFIG_DURATION` and **never compares
a host key against a client key**, and it performs no cross-field check of any
kind. Two further unenforced bounds belong with it: `data_dir` has no length or
shape validation at all, its practical bound being `AF_UNIX` `sun_path` enforced
only by `bind_owner_only` failing at `runtime.rs:836` *after* validation passed
and *after* the instance lock was taken; and only two of the eight startup gates
live in `validate`, because the reservation feasibility gates (`:693`, `:698`,
`:708`) and the resident floor (`:736`) are handler-dependent. So "the config
validated" never implies "this host can start."

### The default configuration is safe for capacity and not for detection

One sentence, because it is the operational summary of everything above.

On the **capacity** side the defaults are conservative and internally consistent,
and the arithmetic was verified independently by lens A from `wire.rs:28`
(`HEADER_LEN = 21`) and `wire.rs:371`/`:35` (`MAX_BODY_LEN = 67,108,864`):
`EGRESS_RESERVED_BYTES` = 67,108,885, `SCRATCH_RESERVED_BYTES` = 184,616,192,
`MIN_RESIDENT_BYTES` = 318,833,941 (304 MiB), default `max_resident_bytes` =
385,942,805 (368 MiB), and the admission pool at defaults before catalog and
retained subtraction is 134,217,728, exactly 2 × `MAX_BODY_LEN`. Nothing silently
clamps: every out-of-range value returns a `ConfigError` naming the offending key
(`config.rs:158`, `:161`, `:169`, `:176`, `:187`, `:358`, `:361`) and every
`Display` arm prints the configured and maximum values (`:420-457`).

On the **detection** side the same defaults arm nothing. No liveness probe is
started, so a wedged peer is invisible at the application layer. The health
snapshot is seeded `Degraded` with an empty `components` map (`runtime.rs:889-893`)
and the health task is spawned one line before `accept_loop` (`:933` versus
`:934`), so a client can be served, and can read `host.status`
(`connection.rs:691-695`), before the first probe returns - for up to
`lifecycle_callback_deadline`, 30 s at defaults, under a slow `handler.health`.
The distinguishing signal exists, because `build_target_index` requires one to
three manifests (`:500`) and `composite.rs:334-348` emits one entry per
component so a real report is never empty, but it is incidental and unasserted,
and no field marks "not yet probed". And a closure-store open failure is
swallowed into `None` at `serve.rs:162` and `:349`, so a permissions or symlink
problem on the closure root presents as "no harness available" rather than as
"the closure store is insecure".

### Coverage: the weakest source-resident position of the three sub-parts

**CI in this tree.** `.github/workflows/ci.yml:118` and `:126` run
`cargo test --workspace --all-targets --all-features --locked` on the 1.98 and stable
toolchains, so every integration binary and every inline test this section counts
executes in CI. The named-versus-unnamed distinction and the `ci.yml` line numbers
below describe the source repository's workflow at authoring time and are kept as
provenance; they are not coverage gaps here.

**11 in-crate tests reach 3,246 lines, all run in CI in this tree, and there are zero
doctests.** The 11 are 10 in `config.rs` (`:467`, `:472`, `:502`, `:520`,
`:550`, `:564`, `:576`, `:603`, `:636`, `:646`) and 1 in `runtime.rs` (`:1326`,
`stalled_generations_share_one_shutdown_goodbye_deadline`).
`harness_closure.rs`, `lib.rs`, and `file_mode.rs` have none. Four integration
binaries carry this sub-part's claims - `tests/synapse_bundle.rs` (24 tests),
`tests/harness_closure.rs` (15), `tests/ipc_budget_topology.rs` (9),
`tests/activation.rs` (4). The source repository's CI named none of them; in this tree all four run under `ci.yml:118` and `:126`.

2e owns four CI-executed `compile_fail` doctests and 2b owns two; **2f owns
none**, and that is the largest structural gap in its inventory, because
`ci.yml:190` runs `cargo test -p host-runtime --doc` and `config.rs`,
`harness_closure.rs`, and `lib.rs` are all `pub mod` (`lib.rs:14`, `:17`,
`:18`), so a doctest added to any 2f file would execute in CI today. For a
sub-part whose entire contract is doc comments, the one CI lane it could reach is
the one it does not use.

`tests/lifecycle.rs` (35 tests, 1,846 lines) is CI-named and does reach
`runtime.rs` transitively, since `run` is how any host starts. It is Part 2a
scope and its subject is lifecycle records and publication rather than the
configuration contract. Recorded so a later pass does not credit it as coverage
for this sub-part's claims, and does not overlook that it is the one CI-executed
path that touches these files at all.

**Four quiet areas frame the fault map**, three synthesized here and a fourth
added by a disposition pass once the construction conditionality map's
shutdown-is-unconditional row was refuted. Carried in full in
[existing-checks.md](runtime-config/existing-checks.md).

1. **`harness_closure.rs` is 1,122 lines of untrusted-manifest filesystem code
   with zero in-crate tests.** It validates untrusted manifests
   (`validate_manifest`, `:231`), materializes content-addressed trees through
   `openat`/`renameat_with`/`unlinkat` with explicit modes (`:14-16`), verifies
   file hashes and modes (`:826`, `:859`), checks directory ownership (`:919`),
   prunes a store (`:554`), enforces five hard caps (`MAX_MANIFEST_BYTES` 16 MiB,
   `MAX_NODES` 65,536, `MAX_PATH_BYTES` 4096, `MAX_STRING_BYTES` 1024,
   `:25-28`), and guards against sticky bits and non-regular files (`:29-32`).
   None of that is exercised by its one test binary, which CI does run in this
   tree (`ci.yml:118`, `:126`), and `:400`'s `.expect` makes a validation gap a panic rather than a
   rejection.
2. **The configuration contract is proven only by rejection.** `config.rs` is the
   only authority for twenty of the twenty-one keys, its ten tests all prove
   rejection, none runs in CI, and the file has no doctest even though one would.
   `HostTiming`'s seven keys are validated for zero and overflow at `:341-363`
   and for nothing else, so `shutdown_deadline` is proven nonzero and never
   proven to bound shutdown.
3. **The forced shutdown path makes five unbounded or re-armed decisions and is
   tested nowhere, and there is a sixth teardown path that is not this one.**
   `runtime.rs:1144-1244` calls `force_close_all_routes` twice (`:1206`, `:1216`)
   with no enclosing timeout, re-arms a doubled deadline after the original
   expired (`:1223`), trips the fatal latch on one branch (`:1234`), and runs the
   handler callback on another (`:1240`), returning `false` from three separate
   places. The comments are unusually careful and each argues its own ordering
   correctly. What is quiet is that the *composition* of those stages, which is
   what an operator experiences as a shutdown that can take up to ten times the
   configured deadline on its longest exit, is argued nowhere and tested nowhere.
   Quieter still is `AbandonGuard::drop` (`:419-476`), which replaces the whole
   sequence when the `run` future is dropped, performs two **explicitly
   unbounded** `tracker.wait()` calls (`:457`, `:471`), and honours no configured
   deadline at all. Nothing in this catalog covers it; it is queued as a gap.

4. **`AbandonGuard::drop` is a second teardown path that no record and no test
   reaches.** Split out of area 3 rather than left as its aside, because the two
   have different shapes: the forced path is bounded badly, and this one is not
   bounded at all. `runtime.rs:419-476` is entered on a dropped `run` future
   rather than on a cancelled token, performs no graceful drain and sends no
   connection Goodbyes, and its own comment at `:452-456` states the unbounded
   wait deliberately. The proof that the interleaving is real is in the crate:
   `run_handler_shutdown`'s once-latch comment (`:1260-1264`) exists to stop this
   path and `shutdown_sequence` from both running the handler callback. Full
   entry in [existing-checks.md](runtime-config/existing-checks.md).

## Reachability: runtime and configuration

**Thirteen records are `default-production` under the convention below, and one is `explicit-config-only`.**
No record here is `test-only`. The labels rest on the construction conditionality
map above plus three facts, per METHOD rule 4. **The map was rebuilt after an
independent evaluation refuted two of its rows and one of its conclusions, and
every one of these fourteen labels was re-derived against the corrected map
rather than carried forward. None moved.** The reasoning is recorded in full at
the end of the map, under "Which dependent labels this rebuild puts back in
question".

1. **`runtime::run` is the library's entry, and the path it takes by default is
   what `default-production` names here.** The source catalog cited
   `crates/daemon/src/bin/eidnara_host/serve.rs` as `run`'s one non-test caller,
   with `SIGINT`/`SIGTERM` handlers cancelling the token `run` receives. That crate
   is not in this tree (scheduled for U4, `docs/properties/README.md:52`); here
   `run` (`runtime.rs:541`) is reached from `examples/` and a bench. These thirteen
   records describe what `run` does on every start with no composition-dependent
   state, so they keep the label under the convention stated in the provenance
   section, and the question of `run`'s own callers is bias B1 in
   [discovered-at-u3/portfolio-evaluation.md](discovered-at-u3/portfolio-evaluation.md).
2. **Nothing in the sequence is `cfg`-gated.** The map's conditional steps are
   `set_publish_hook` (test-only, reachable only through the `#[doc(hidden)]`
   `run_with_publish_hook`), the setup-socket bind and publish pair (skipped on an
   already-cancelled token, which is itself a `default-production` state), the
   per-connection liveness loop, and the shutdown tail from map row 24 onward,
   which is conditional on the `run` future still being polled. None of the four
   is a `cfg`, which is the property these labels actually need.
3. **The one `explicit-config-only` label is
   [rt-a-every-published-configuration-field-changes-host-behaviour](#rt-a-every-published-configuration-field-changes-host-behaviour),
   and lens A's reasoning is that the property is about what an *embedder* can
   set rather than about what the production binary does set.** Its subject is the
   published surface, and its one violator, `HostInit::host_capabilities`, is
   written as `Vec::new()` at all four construction sites, so a default
   production host never populates it.

One asymmetry to state explicitly, because it is the opposite of what the
`runtime.rs:118-119` comment implies. The reserved admission pools are
**`default-production` reachable**, not dormant: `broca/mod.rs:164-177` returns a
`ResourceDeclaration` with `route_class: RouteClass::Reserved` and 96/96 counts
(`broca/config.rs:185`, `:188`), the comment at `broca/mod.rs:169-170` makes it
deliberate and unconditional, `composite.rs:10-13` fixes the direct profile's
tertiary as `broca/management_surface`, and `serve.rs:575` composes it. So the
comment's second clause is false and its first clause is true only of a
composition that excludes Broca. Sub-part 2e reached the same verdict
independently. The record this bears on,
[rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration](#rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration),
is worded conditionally ("When no linked module declares a reserved
allocation") and is therefore unaffected as written; the correction is carried
here rather than edited into it.

## Index

Fourteen records, in the order lens A proposed them. Lens B proposed none by
design; it built the 20-claim register and the check inventory.

| Slug | Type | Confidence |
| --- | --- | --- |
| [rt-a-startup-refuses-every-configuration-it-cannot-fund](#rt-a-startup-refuses-every-configuration-it-cannot-fund) | safety | high |
| [rt-a-the-ingress-pool-derivation-cannot-underflow](#rt-a-the-ingress-pool-derivation-cannot-underflow) | safety | high |
| [rt-a-no-configured-limit-is-silently-clamped](#rt-a-no-configured-limit-is-silently-clamped) | safety | high |
| [rt-a-the-default-configuration-arms-no-liveness-probe](#rt-a-the-default-configuration-arms-no-liveness-probe) | safety | high |
| [rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval](#rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval) | safety | high |
| [rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline](#rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline) | safety | high |
| [rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline](#rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline) | safety | high |
| [rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one](#rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one) | safety | high |
| [rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration](#rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration) | safety | high |
| [rt-a-every-published-configuration-field-changes-host-behaviour](#rt-a-every-published-configuration-field-changes-host-behaviour) | safety | high |
| [rt-a-configuration-is-frozen-for-the-incarnation](#rt-a-configuration-is-frozen-for-the-incarnation) | safety | high |
| [rt-a-a-closure-store-open-failure-is-classified-not-swallowed](#rt-a-a-closure-store-open-failure-is-classified-not-swallowed) | safety | medium |
| [rt-a-the-activation-fast-probe-interval-is-entered](#rt-a-the-activation-fast-probe-interval-is-entered) | reachability | high |
| [rt-a-an-initialized-handler-drains-without-publishing](#rt-a-an-initialized-handler-drains-without-publishing) | reachability | high |

Semantics distribution: eleven `always`, one `always-or-unreached`, two
`sometimes`. No `reachable`, no `unreachable`. Type distribution: twelve safety,
two reachability, no liveness. Reachability distribution: thirteen
`default-production`, one `explicit-config-only`. Confidence: thirteen high, one
medium.

**The five group headings below are this synthesis's own**, chosen by shared
mechanism rather than by the order records were proposed. Grouping reorders the
records relative to the index; the index is the record-order artifact. Record
bodies were verbatim from lens A at synthesis. Two formatting-only changes were
applied uniformly: fields are wrapped to about 80 columns, since lens A's 2f
records were written on single long lines, and evidence links are rewritten from
the lens file's `../evidence/` form to `evidence/<slug>.md` so they resolve from
this directory. **Two records are no longer verbatim, because a disposition pass
edited them:**
[rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval](#rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval)
and
[rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline](#rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline),
both in Group B, whose `Check:` lines asserted conditions that could not fail and
could not pass respectively. The changes and their justification are in
[portfolio-evaluation.md](runtime-config/portfolio-evaluation.md). Where a record's prose says
"per the map above", the map it means is the construction conditionality map in
the leading section of this file, which the same pass rebuilt.

---

## Group A: what startup refuses to fund

Three records on the eight startup gates and the one piece of arithmetic they
protect. The first is the joint postcondition at `HostShared` construction, which
four existing tests cover one gate at a time and none asserts together. The
second is the unchecked subtraction that derives `ingress_budget`, whose only
guard sits 160 lines earlier. The third is that no out-of-range value is ever
clamped, which is the premise the other two rely on. Grouped because all three
are about the boundary between "the configuration validated" and "this host can
start", and that boundary is not where `validate` is.

### rt-a-startup-refuses-every-configuration-it-cannot-fund

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `handler_contract.rs:323`
`reservations_must_leave_one_general_slot_in_each_pool`, `:375`
`class_and_reservation_mismatches_fail_startup`, `:408`
`parked_general_task_bound_must_leave_one_free_slot`, and `:437`
`retained_declaration_raises_the_resident_floor_exactly` each cover one gate; the
joint postcondition at the construction site is unasserted
Guarantee: If `run` reaches `HostShared` construction, every permit count and
byte quantity it computes is non-negative, within `Semaphore::MAX_PERMITS`, and
leaves at least one maximum request body of ingress headroom.
Check: `always` - at `HostShared` construction (`runtime.rs:748`, re-verified; the source catalog cited `:882`; the `Semaphore::new` calls span `:771-780`, and `connection_permits` at `:780` from `max_connections` is in the enumeration alongside the five counts derived from reservations), assert `max_pending_requests > reservations.pending`, `max_handler_tasks > reservations.tasks`, `general_task_holds < max_handler_tasks - reservations.tasks`, `max_resident_bytes >= MIN_RESIDENT_BYTES + catalog_resident + retained_bytes`, and, for every count passed to `Semaphore::new` at `:771-780` (pending, task, reserved pending, reserved task, handshake, and connection from `max_connections`), that the count is at most `Semaphore::MAX_PERMITS`, and for every quantity passed to `ByteBudget::new` at `:762-770` that it is within the budget type's range. `always` rather than `always-or-unreached` because this construction is on every successful startup path with no condition, per the map above.
Fault/timing angle: none. Startup is single-threaded here and the inputs are
fixed by the time the gates run.
Required faults and enabling state: a handler whose `resource_declarations` sum
approaches or exceeds a configured limit. `handler_contract.rs:302-320` already
builds one.
Confidence: high - [evidence](evidence/rt-a-startup-refuses-every-configuration-it-cannot-fund.md).
I traced all eight gates and verified each line, and computed the byte arithmetic
independently.
Existing check: four tests, each covering one gate; none asserts the conjunction
at the use site. Status `unaudited`.
Impact: a wrapped subtraction at `runtime.rs:762-767` reaching `Semaphore::new`
or `ByteBudget::new` panics during `HostShared` construction, after the transport
is published, so a client can discover a dead endpoint.
Open questions: None.

### rt-a-the-ingress-pool-derivation-cannot-underflow

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test relates `config.rs:23-24` to `runtime.rs:762-767`;
`config.rs:520-548` asserts the floor decomposition but never the runtime
subtraction
Guarantee: The unchecked subtraction that derives `ingress_budget` never
underflows, and its result is never below one `MAX_BODY_LEN`.
Check: `always` - immediately before the `ByteBudget::new` subtraction at `runtime.rs:762-767`, where `config.limits.max_resident_bytes`, `catalog_resident`, and `reservations.retained_bytes` are all in scope, assert
`max_resident_bytes >= EGRESS_RESERVED_BYTES + SCRATCH_RESERVED_BYTES +
catalog_resident + retained_bytes + MAX_BODY_LEN`. `always` because the
subtraction is unconditional; the guard living in `HostConfig::validate` (`config.rs:122`) rather than at the subtraction is exactly why
the assertion belongs at the consumer.
Fault/timing angle: none, but the coupling is a maintenance window rather than a
runtime one: any independent edit to `MIN_RESIDENT_BYTES` or to the subtrahend
list breaks it silently in release builds.
Required faults and enabling state: a `max_resident_bytes` exactly at the
handler-dependent floor, plus a non-zero `retained_resident_bytes` declaration
and a non-trivial catalog. `handler_contract.rs:437` constructs the floor case.
Confidence: high - [evidence](evidence/rt-a-the-ingress-pool-derivation-cannot-underflow.md).
Verified the gate, the constant's definition, and the arithmetic; confirmed
`ByteBudget::new` casts and would panic.
Existing check: `config.rs:520-548`
`the_resident_cap_splits_into_three_non_overlapping_pools` covers the constant
decomposition only. Status `unaudited`.
Impact: release-mode `u64` wrap producing a near-`u64::MAX` budget, then a panic
inside `Semaphore::new` after publication.
Open questions: None.

### rt-a-no-configured-limit-is-silently-clamped

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `config.rs:503`, `:551`, `:565`, `:577`, `:604`, `:637`,
`:647` cover rejection for individual keys; no test asserts that no path clamps
Guarantee: An out-of-range limit or duration is rejected with an error naming the
offending key, never clamped to a bound the caller cannot observe.
Check: `always` - for every numeric limit and duration field of `HostLimits`,
`HostTiming`, and `LivenessPolicy`, set it one step outside its bound and assert
`validate` returns `Err` whose `Display` names that field; and for every field
including the boolean `invalidate_on_missed`, which has no out-of-range value,
assert that no accepted `HostConfig` differs from the submitted one in any field.
`always` because it must hold on every validation.
Fault/timing angle: none.
Required faults and enabling state: none. Pure function of a constructed
`HostConfig`.
Confidence: high - [evidence](evidence/rt-a-no-configured-limit-is-silently-clamped.md).
Read every branch of both validators and every `Display` arm. The one silent
narrowing found is `file_mode.rs:18`, outside `HostConfig`.
Existing check: seven unit tests in `config.rs`, per key, not exhaustive over
fields. Status `unaudited`.
Impact: an operator who sets a value and gets a different one silently loses the
ability to reason about the host's capacity, which is the premise of
`config.rs:87-88`.
Open questions:
- `file_mode::raw_mode` is `pub(crate)` and shared with `generation.rs`, which is
  Part 2a's file. Whether that caller upholds the "already within `0o7777`"
  precondition is unverified from here. (needs Part 2a)

---

## Group B: fixed bounds that outrank configured ones

Three records on the same shape, in the same direction: a value the operator
cannot set governs a value the operator can. The first is the 50 millisecond
probe interval, whose switch is handler-controlled and unbounded. The second is
`transport_setup_deadline` armed twice serially, so one configured duration
bounds two stages. The third is the doubled callback deadline armed after the
shutdown deadline already expired. Grouped because all three violate the same
normative rule - protocol `:731`'s "per-stage timers MUST NOT multiply it" - and
because in all three the code carries a written justification that is sound while
the consequence for the knob is unstated.

### rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - `tests/lifecycle.rs:165` sets `health_interval` to 50 ms,
which coincides with the hardcoded value and therefore cannot distinguish the two
branches
Guarantee: The health probe cadence is either the configured `health_interval` or
the fixed 50 ms activation cadence, and which one applies is a stated function of
the component-reported activation state. The code places no bound on how long a
component may hold the fast cadence by reporting `starting`; that bound is the
open product decision below, not a promise this record makes.
Check: `always` - over two assertable conjuncts and one measurement, and the split is the point. **Conjunct 0:** at `runtime.rs:972-973` (re-verified), whenever `activation_in_progress` is true the selected interval equals `Duration::from_millis(50)` exactly, and whenever it is false the selected interval equals `shared.timing.health_interval`; both branches are unconditional within the loop, so `always` holds on every iteration. The remaining text names the source catalog's conjuncts.
**Conjunct 1, which carries the `always` semantics:** at `runtime.rs:972-976`,
whenever `activation_in_progress` is false the selected interval equals
`shared.timing.health_interval` exactly. That is a pass/fail bound, it holds on
every loop iteration, and `always` is correct because the selection is
unconditional within the loop. **Conjunct 2 has no assertable semantics and is
`partial` pending a product decision, having previously masqueraded as a check.**
The fast path has *no pass/fail bound in the code to assert*: the earlier text
asked an oracle to "record the number of consecutive iterations that selected
50 ms so a campaign can bound it", which measures without deciding - every
observation passes, so it cannot fail and is therefore not a check. The two
candidate bounds a campaign could assert instead are stated here so the decision
is a choice between named options rather than an open-ended design question, and
neither can be adopted without the open question below being answered, because
both invent a limit the code does not contain:
- **A count bound**, `consecutive_fast_probes <= K`, which needs a `K`. Nothing
  in `HostTiming` supplies one and no constant in `runtime.rs` is a candidate.
- **A duration bound**, `time_in_fast_cadence <= lifecycle_callback_deadline` or
  some other configured span, which needs a decision about which existing knob
  ought to govern activation, and there is no reading of `config.rs:144-162`
  under which any of them does.
Until one is chosen, the honest oracle is conjunct 1 plus an instrumented count
reported as a **measurement**, not asserted. Recording that distinction is what
keeps this record from shipping a check that can only pass.
Fault/timing angle: the window is unbounded and that is the finding rather than
an accident of the fixture. The predicate at `:1051-1071` is driven entirely by
handler-authored strings in the previous report's metrics, so nothing in the host
limits how long the fixed cadence persists. A handler that never leaves
`starting` holds it forever.
Required faults and enabling state: a handler whose `health` report carries
`metrics.components.<id>.metrics.storage_state == "starting"` or
`synapse_state == "starting"`, plus a `health_interval` distinguishable from
50 ms. `tests/lifecycle.rs:165` must change its value to make the two branches
separable. Conjunct 1 needs only the second half, since it asserts the `else`
branch; conjunct 2 needs both.
Confidence: high - [evidence](evidence/rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval.md).
Verified the branch, the predicate, the single `health_interval` consumer, and
that `MAX_CONFIG_DURATION` admits 365 days. Confidence is about the mechanism,
which is fully verified; it is not a claim that the record's second conjunct is
implementable today, and the `Check:` line says so.
Existing check: none that separates the branches. Status `unaudited`.
Impact: an operator who raises `health_interval` to reduce probe load gets no
relief while any component reports `starting`, and 20 handler callbacks per
second continue. This is Part 2a's hardcoded-60-second shape in the same
direction.

> Synthesis note on one citation inside this record, carried here rather than
> edited into it. The predicate's span is `runtime.rs:1051-1071`, which lens B
> re-derived and this synthesis confirmed. The record's `Fault/timing angle:`
> said `:1051-1074` before this disposition and now says `:1051-1071`. The
> finding is unaffected; only the span moved.

Open questions:
- Should the fast cadence carry its own bound, and if so which of the two forms
  above? Until this is answered the record has one assertable conjunct and one
  measured one, which is why its `Exercised:` line cannot reach `yes` by fixture
  work alone. (needs human input)

### rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - 2c's `fault-map.md:180` reaches one of the two
`transport_setup_deadline` sites; nothing measures the serial sum
Guarantee: The host's total pre-service budget for one accepted socket is a
stated function of the configured deadlines, and the specification's coupling
warning accounts for every stage that consumes one.
Check: `always` - measure wall-clock from `run_connection` entry
(`connection.rs:86`) to the return of `activate_server` (`:155-165`) on a peer that
stalls maximally at each stage, and assert the total is at most
`auth_deadline + 2 * transport_setup_deadline`. `always` because the bound must
hold on every accepted socket.
Fault/timing angle: three serial windows: `auth_deadline` at `:125`,
`transport_setup_deadline` at `:158` for `prepare`, and
`transport_setup_deadline` again at `:177` for `activate_server`. At defaults
that is 6 s against a documented client budget of 2 s.
Required faults and enabling state: a peer that stalls inside authentication,
then inside descriptor transfer. 2c's `fault-map.md:52` describes the fixture and
notes it does not exist.
Confidence: high - [evidence](evidence/rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline.md).
Verified all three sites and confirmed `HostConfig::validate` performs no
cross-field check.
Existing check: none. Part 2c's `existing-checks.md:569-575` records the coupling
as a documentation gap with the figure 4 s. Status `unaudited`.
Impact: a client conforming to the documented 2 s handshake deadline abandons a
host that is still inside a budget the host considers valid, producing an
`outcome_unknown` class the specification's coupling note was written to prevent.
Open questions: None.

### rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `tests/lifecycle.rs:714-715` sets both
`lifecycle_callback_deadline` and `shutdown_deadline`, so the forced path is
reachable; no assertion bounds the total
Guarantee: Given handler callbacks that yield to the runtime (cooperatively cancellable), `run` returns within a stated function of the configured deadlines; that function is stated per exit rather than as one figure, and it is documented wherever `shutdown_deadline` is described. A callback that blocks its worker thread without yielding is outside this guarantee, because no `tokio::time::timeout` can preempt it; the bound for that case is external to the host.
Check: `always`, per exit, because `shutdown_sequence` has three and they do
different amounts of work. From the shutdown token's cancellation to `run`'s
return, assert elapsed time is at most:
writing `L` for `lifecycle_callback_deadline` and `R = 2 * L` for one
`force_close_all_routes` call (the tracker wait at `dispatch.rs:1299` plus
`run_route_gone` at `:1162-1166`, each under `L`, and no `timeout` wraps the call):
- `shutdown_deadline + L` on the graceful exit at `runtime.rs:1069` when the drain
  finished inside `deadline`, and `shutdown_deadline + R + L` when it did not but
  the tracker wait at `:1048` still succeeded, since the first
  `force_close_all_routes` (`:1042`) runs before that wait; `run_handler_shutdown`
  then runs under its own budget (`:1098`);
- `shutdown_deadline + 3R = shutdown_deadline + 6L` on the fatal-latch exit at
  `:1064`: the first route close (`:1042`, only when the drain missed), the second
  (`:1050`), and the doubled chain (`:1053-1054`, `2L`); it is the *only* exit
  that never calls `run_handler_shutdown` (`:1060-1064` returns before `:1066`);
- `shutdown_deadline + 3R + L = shutdown_deadline + 7L` on the forced exit at
  `:1067`, which pays both route closes, the doubled chain, **and** the handler
  callback (`:1066`, bounded at `:1098`).
The route-close terms are inside the formulas, not measured separately, so a
compliant forced path that spends them does not fail the assertion; the
`shutdown_deadline` term is absolute (`:1037` and `:1048` share one `deadline`),
so the first route close does not extend the tracker wait.
`always` because each bound must hold on every shutdown that takes its exit, and
the bounds are in the units the code bounds.
**Every bound above is conditional, and the condition is not a detail.** These
are `tokio::time::timeout` and `timeout_at` budgets over awaited futures, and a
timeout cannot preempt a future that never yields. A handler whose `shutdown()`
blocks its worker thread instead of awaiting is not interrupted by `:1098`; the
function's own doc comment at `:1082-1084` states that the callback "is never
aborted" and that an overrun "trips the fatal latch and returns non-graceful
while the still-tracked task keeps running". The same holds for both
`tracker.wait()` calls. So the check is `elapsed <= bound` **given cooperatively
cancellable callbacks**, and an oracle must construct a yielding slow callback,
not a blocking one, or it will time out rather than fail an assertion. A
non-yielding callback defeats every finite ceiling here, which is why the previous
single-figure framing was not merely imprecise but the wrong shape.
Fault/timing angle: `:1214` fails, then `:1224` awaits a second, fresh budget
computed at `:1223` as `lifecycle_callback_deadline.saturating_mul(2)`. At
defaults, 60 s armed after a 10 s deadline expired, and then either the fatal
latch at `:1234` or 30 s more at `:1240`. `saturating_mul` means a
`lifecycle_callback_deadline` above half of `MAX_CONFIG_DURATION` yields a budget
the validator would itself reject.
Required faults and enabling state: a tracked task that survives the shutdown
deadline and then *does* drain, for the `:1241` exit; one that never drains, for
the `:1238` exit. `tests/lifecycle.rs:678` and `:714` build the non-yielding
callback shape, which reaches the forced path but which, being non-yielding, is
the shape that cannot bound anything. Distinguishing the two forced exits is
fixture work nothing currently does.
Confidence: high - [evidence](evidence/rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline.md).
Verified both deadline sites, read the justifying comment at `:1217-1222`, and
re-read `:1234-1243` and `run_handler_shutdown` (`:1259-1297`) end to end for
this disposition to separate the three exits.
Existing check: none bounding any of the three totals. Status `unaudited`.
Impact: a supervisor that budgets `shutdown_deadline` for a stop, plus the
documented client 5 s, kills the host during a cleanup phase the host considers
in-budget, which is precisely the window `:1217-1222` says must not be
interrupted. The exit that omits `run_handler_shutdown` has a second consequence:
on that path the handler's `shutdown()` never runs at all from
`shutdown_sequence`, so component-owned drain work is left to whichever of
`retain_lock_until_drained` (`:951`) or `AbandonGuard`'s drop path (`:467`) gets
there, mediated by the once-latch at `:1265-1270`.
Open questions:
- `saturating_mul(2)` can produce a duration the validator rejects as an input.
  Whether the derived budget should be clamped to `MAX_CONFIG_DURATION` is
  unresolved. It cannot overflow, so this is a coherence question rather than a
  defect.
- Should the fatal-latch exit at `:1238` run the handler shutdown callback? The
  comment at `:1228-1233` argues it must not, to avoid overlapping two handler
  callbacks, and that argument is sound. The consequence it does not state is
  that a host taking this exit returns `false` having never invoked `shutdown()`
  on this path, which is a different contract from the other two exits.
  (needs human input)

> Disposition note replacing the synthesis note that stood here. The earlier note
> said the check's single bound,
> `shutdown_deadline + 2 * lifecycle_callback_deadline`, was "the right bound to
> assert", observed that lens B's composed total of about 100 s exceeds it, and
> concluded that "an oracle written to the check as stated would **fail** on a
> correct build", then declined to fix the check because the record text was
> preserved verbatim. An independent evaluation refuted the framing rather than
> the arithmetic: a check known to fail on a correct build is not a bound with a
> caveat, it is a wrong bound, and the reason it was wrong is that
> `shutdown_sequence` has three exits and the figure describes one of them.
> `run_handler_shutdown` at `:1240` was omitted, and it runs on two of the three.
> The check is now stated per exit, at 40 s, 70 s, and 100 s of configured units,
> the `:1238` exit is identified as the one that omits the handler callback, and
> the ceiling-versus-floor confusion in the earlier figures is corrected in the
> "Two fixed bounds" section above. The verbatim-preservation rule that blocked
> the earlier fix is a synthesis convention, not a METHOD rule, and a disposition
> pass is where it yields.

---

## Group C: the detection the default configuration does not arm

Three records on what a default host cannot see. The first is that no liveness
probe is armed at all, which is the reachability label for every liveness
property in the catalog. The second is that a `host.status` served before the
first health probe is distinguishable from a genuinely degraded one only by an
incidental empty map. The third is the reachability of the activation fast-probe
situation, without which the override in Group B cannot be measured. Grouped
because all three are about the host's own view of itself, and because in all
three the signal either does not exist or exists by accident.

### rt-a-the-default-configuration-arms-no-liveness-probe

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `tests/lifecycle.rs:496` `liveness_is_disabled_by_default`
asserts no Ping arrives within 500 ms on a default host
Guarantee: With `liveness` unset, the host arms no Ping timer, sends no Ping, and
never invalidates a connection for a missing Pong.
Check: `always` - whenever `shared.liveness.is_none()`, assert no `liveness_loop`
task was spawned for any generation, and no frame of type `Ping` was ever
enqueued. `always` because the absence must hold for the whole incarnation, not
merely at one observation.
Fault/timing angle: the window is the whole incarnation. A default host cannot
detect a silently wedged peer through Ping at all; peer death is discovered only
by the ring's own path.
Required faults and enabling state: a default `HostConfig`. That is the
production configuration.
Confidence: high - [evidence](evidence/rt-a-the-default-configuration-arms-no-liveness-probe.md).
Verified `config.rs:294`, the single spawn condition at `connection.rs:279`, and
that `serve.rs:582-593` reaches `HostConfig::default` for this field.
Existing check: `tests/lifecycle.rs:496`. Status `unaudited`.
Impact: this is the reachability label for every liveness property in the
catalog. Any record whose enabling state is a `LivenessPolicy` is reachable only
from `tests/lifecycle.rs:402` or `tests/client.rs:64`, never from production.
Open questions:
- `config.rs:236-238` says `invalidate_on_missed` stays `false` until
  the source module-host work. `tests/client.rs:67` sets it `true`. So the only code
  path that ever invalidates on a missed Pong is a test. Whether that is intended
  coverage of a future default or an accidental divergence from the stated policy
  is a design question. (needs human input)

### rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test reads `host.status` before the first probe completes
Guarantee: An authenticated `host.status` served before any health probe has
completed is distinguishable from one reporting a genuinely degraded component.
Check: `always` - whenever the `host.status` response reports `degraded`, assert
that either `metrics.components` is non-empty or an explicit not-yet-probed
marker is present. `always` because a client may read the snapshot at any moment,
including the first.
Fault/timing angle: the window opens at `runtime.rs:933`, when the health task is
spawned, and closes when the first probe stores a report at `:1120-1123`.
`accept_loop` starts one line later at `:934`, so the window is genuinely
client-visible, and it lasts up to `lifecycle_callback_deadline` (30 s at
defaults) under a slow `handler.health`.
Required faults and enabling state: a handler whose first `health` call blocks,
plus a client that authenticates and issues `host.status` inside that window.
`tests/lifecycle.rs:579` already builds a slow-callback handler.
Confidence: high - [evidence](evidence/rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one.md).
Verified the seed, the reader, the spawn ordering, and that a real report always
carries at least one component.
Existing check: none. Status `unaudited`.
Impact: a supervisor gating traffic on `host.status` reads `degraded` from a
healthy host and may withhold traffic or restart it. The distinguishing signal
exists but is incidental and unasserted, so a change to either the seed or the
composite's report shape removes it silently.
Open questions: None.

### rt-a-the-activation-fast-probe-interval-is-entered

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - no test constructs a component report carrying
`storage_state` or `synapse_state` equal to `starting` and observes the branch
Guarantee: The activation-in-progress fast probe cadence is entered at least once
per campaign, so its handler-controlled predicate and its 50 ms interval are
exercised rather than assumed.
Check: `sometimes` - a marker on the `Duration::from_millis(50)` branch at
`runtime.rs:972-973`, fired when the fixed interval is selected (`:1129-1130` is
the test-only `stalled_generation` helper and is unrelated to activation). `sometimes` and not `reachable` because this is situation coverage: a
campaign can execute the health loop thousands of times, and even execute the
`if` at `:972`, while never producing the operational state the branch
represents, which is a component that has published `starting` in its health
metrics. Line coverage of the conditional does not witness that state.
Fault/timing angle: the situation requires a real post-publication activation
window. `spawn_activation_task` (`:932`) runs `handler.activate()` with
deliberately no lifecycle deadline (`:981-983`), so the window's length is
component-determined.
Required faults and enabling state: a handler or composite whose `health` reports
a component with `metrics.storage_state == "starting"`, observed by the probe at
`:1092` before activation completes. `tests/activation.rs` is the natural host for
the fixture.
Confidence: high - [evidence](evidence/rt-a-the-activation-fast-probe-interval-is-entered.md).
Verified the predicate, both metric keys, the branch, and that a real composite
report populates `components`.
Existing check: none. Status `unaudited`.
Impact: if the situation is never produced, the 50 ms path and the predicate that
selects it ship unexercised, and the override in
`rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval` cannot be
measured at all.
Open questions: None.

---

## Group D: the configuration surface, frozen and mostly effective

Three records on the surface itself rather than on any one key. The first is that
every published field reaches a consumer, with one violator. The second is that
nothing changes value after `HostShared` construction, which is the assumption 55
or more records across the catalog rest on. The third is that the reserved pools
are correctly gated when nothing declares a reservation. Grouped because all
three are properties of the wiring rather than of any value, and because two of
the three are discharged by enumeration rather than by a fault.

### rt-a-every-published-configuration-field-changes-host-behaviour

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - nothing enumerates the fields against their consumers
Guarantee: Every field an embedder can set on `HostConfig`, `HostLimits`,
`HostTiming`, `LivenessPolicy`, or `HostInit` reaches at least one consumer, so
setting it changes some observable host behaviour.
Check: `always` - for each public configuration field, two host executions that differ only in that field produce the documented observable difference for its family: a limit field moves the admission boundary at which a request or connection is rejected, a timing field moves the instant at which the corresponding deadline fires under paused time, a liveness field changes the probe cadence or the retirement decision, and an init field arrives unchanged in the `HostInit` passed to `HostHandler::initialize` (`handler.rs:532`), asserted at the handler rather than through client-visible behaviour, because the host does not publish `host_capabilities` or `storage` and a conforming handler may ignore them. A read site outside `config.rs` and outside a `Debug` implementation is a necessary screen, not the check: a field that is read and ignored fails. `always` because it is a property of the surface, evaluated once per field.
Fault/timing angle: none. This is a static property of the wiring.
Required faults and enabling state: none. The check is an enumeration, best
expressed as a test that names each field and its consumer, or as a review gate.
Confidence: high - [evidence](evidence/rt-a-every-published-configuration-field-changes-host-behaviour.md).
Grepped each of the 21 fields across the whole repository. One violator:
`HostInit::host_capabilities` (`config.rs:250`), read nowhere, written as
`Vec::new()` at all four construction sites.
Existing check: none. Status `unaudited`.
Impact: an embedder who populates `host_capabilities` believes it advertises
capabilities and it does nothing. Its `Debug` appearance at `config.rs:262` makes
it look load-bearing in diagnostics.
Open questions:
- Is `host_capabilities` a placeholder for the source module-host work work, in which
  case the record documents an accepted gap, or a wiring omission?
  `config.rs:246-247` says `HostInit` is "handed to the linked handler", so a
  handler outside this repository could read it. (needs human input)

> Synthesis note sharpening this record's `Impact:` with a fact lens B added,
> carried here rather than edited into it. `host_capabilities` is not merely
> inert; it is **the one field the redaction impl does not redact**.
> `HostInit`'s hand-written `Debug` exists specifically to redact, because the
> comment at `config.rs:258-260` says the storage descriptor "can carry
> credentials or deployment secrets" so diagnostics get "presence and bounded
> structure only", and `:263` accordingly renders `storage` as `.is_some()`.
> Directly above it, `:262` renders `host_capabilities` in full. Today that
> prints `[]` and leaks nothing, and `HostConfig` derives `Debug` (`:268`) so the
> render reaches any diagnostic that formats a `HostConfig`. So the first
> population of the field lands on the wrong side of conformance vector V24 by
> default, which is a stronger reason to record it than inertness alone.

### rt-a-configuration-is-frozen-for-the-incarnation

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test mutates a config after startup, because no API
permits it
Guarantee: No configured limit, deadline, or policy changes value between
`HostShared` construction and `run`'s return.
Check: `always` - capture `shared.limits`, `shared.timing`, and
`shared.liveness` immediately after `runtime.rs:748` and assert equality at
`run`'s return, and assert no interior mutability exists on those fields.
`always` because every config-dependent property in the catalog depends on it
holding continuously.
Fault/timing angle: none by construction. `limits`, `timing`, and `liveness` are
plain owned values on `HostShared` (`runtime.rs:96-98`), not behind a lock or
atomic, and `HostShared` is shared as `Arc` without interior mutability on those
fields.
Required faults and enabling state: none. The property is structural; the check
is a compile-time or review-level assertion rather than a runtime one.
Confidence: high - [evidence](evidence/rt-a-configuration-is-frozen-for-the-incarnation.md).
Verified the clone sites, the moved `init`, and that no read of `config` follows
`:926`.
Existing check: none. Status `unaudited`.
Impact: if a reload path were ever added, every sibling record that treats a
limit as constant would need re-verification. Recording it now fixes the
assumption explicitly instead of leaving it implicit in 55-plus records across
the catalog.
Open questions: None.

### rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `handler_contract.rs:636`
`zero_reservation_handlers_keep_single_pool_admission` and `:375`
`class_and_reservation_mismatches_fail_startup` cover the pair from the admission
side
Guarantee: When no linked module declares a reserved allocation, the reserved
admission pools hold zero permits and no route ever attempts to acquire from
them.
Check: `always-or-unreached` - assert that an acquisition against
`reserved_pending_permits` or `reserved_task_permits` occurs only for a route
whose class is `Reserved`, and that no such route exists when
`reservations.pending == 0`; and, after `HostShared` construction, that
`reserved_pending_permits.available_permits() == reservations.pending` and
`reserved_task_permits.available_permits() == reservations.tasks` (constructed at
`runtime.rs:777-778`), so the undeclared case requires both pools to hold zero
permits rather than only that no route acquires from them. `always-or-unreached` rather than `unreachable`,
because the pools are legitimately entered on a host that does declare a
reservation; the obligation is that entry is safe and correctly gated, not that
the code is dead.
Fault/timing angle: an acquisition against a zero-permit `Semaphore` blocks
forever rather than failing, so the failure mode is a permanently parked dispatch
task rather than an error. The gate is `build_target_index`'s class/reservation
agreement check at `runtime.rs:535-554`.
Required faults and enabling state: a manifest set whose declared `route_class`
disagrees with its reserved counts. `handler_contract.rs:378-388` constructs both
directions.
Confidence: high - [evidence](evidence/rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration.md).
Verified both construction sites, the agreement gate, and the doc comment at
`runtime.rs:117-121` that states the claim.
Existing check: two tests from the admission side; neither asserts the no-entry
half directly. Status `unaudited`.
Impact: a route that reaches a zero-permit pool parks indefinitely with no error
frame, which presents as a hung request rather than a refusal.
Open questions: None.

> Synthesis note on the doc comment this record cites, carried here rather than
> edited into it. The record's guarantee is conditional and therefore correct as
> written, but the comment at `runtime.rs:117-119` that it verifies against is
> **false in the composed production host**. The comment says the reserved pools
> are "Zero-permit when no module declared a reservation, and then unreachable
> because every route is general-class". `broca/mod.rs:164-177` declares
> `route_class: RouteClass::Reserved` with 96/96 counts (`broca/config.rs:185`,
> `:188`), `composite.rs:10-13` fixes the direct profile's tertiary as
> `broca/management_surface`, and `serve.rs:575` composes it. So the second
> clause is false and the first is true only of a composition that excludes
> Broca. Sub-part 2e's lens B reached the same verdict independently and both
> lenses report it as the fourth misleading comment in this crate; neither
> verified the three prior instances, so **the ordinal is inherited and
> unconfirmed** while the contradiction itself is verified.

---

## Group E: paths nobody owns

Two records on code that runs at startup and belongs to no test. The first is the
harness closure store, 1,122 lines whose only two production constructions
discard their error with `.ok()` in a file outside this crate. The second is the
pre-publication drain, the path a handler takes when it initialized successfully
and the host then never published a transport. Grouped because both are startup
paths whose failure is invisible, and because in both cases lens A's own
confidence or open question records that the answer lives outside this sub-part's
footprint.

### rt-a-a-closure-store-open-failure-is-classified-not-swallowed

Type: safety
Reachability: test-only - `HarnessClosureStore::open` is called only from
`crates/host-runtime/tests/harness_closure.rs` in this tree, and `manifest_digest`
is reached only through the store; the daemon that opens the store in production
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
in the wave that lands it.
Status: active
Exercised: not yet - `tests/harness_closure.rs` covers `open` succeeding and
`validate`/`materialize` failing; no test exercises `open` failing on the
production path
Guarantee: A failure to open the harness closure store is reported with its
distinct cause, not collapsed into an absent store that silently selects a
different execution backend.
Check: `always` - whenever `HarnessClosureStore::open` returns `Err`, assert that
the resulting host startup carries a classified unavailability reason naming that
cause. `always` because every open failure must be classified; the store's
absence is a legitimate state, but an indistinguishable one is not.
Fault/timing angle: none timing-related. The window is startup. `open` fails on a
symlinked or non-owner-only ancestor, a wrong mode, a non-directory, or a
creation failure, each with a distinct `&'static str`
(`harness_closure.rs:1044`, `:1052`, `:1067`, `:1074`, `:1076`, and
`verify_owned_directory` at `:923`).
Required faults and enabling state: a
`${dataDir}/eidnara/harness-closures` path that is a symlink,
group-writable, or owned by another uid. `tests/instance_security.rs` already
builds hostile-path fixtures for the sibling walk in `instance.rs`.
Confidence: medium - [evidence](evidence/rt-a-a-closure-store-open-failure-is-classified-not-swallowed.md).
The `.ok()` at both sites and the distinct error strings are verified. Medium
because the two call sites are in
`crates/daemon/src/bin/eidnara_host/serve.rs`, outside this sub-part's
footprint, so I read only their immediate context and did not trace what the
downstream backend selection ultimately reports to an operator.
Existing check: none on the failure path. Status `unaudited`.
Impact: a permissions or symlink problem on the closure root presents as "no
harness available" rather than "the closure store is insecure", so an operator
investigates the wrong subsystem. This is Part 4f's silent-degradation shape.
Open questions:
- Does `harness_backend` (`serve.rs:344`) ultimately surface any distinguishable
  reason to an operator, or does the `None` terminate in a generic
  unavailability? Unresolved; needs the `daemon` binary pass, which is outside
  this footprint.

### rt-a-an-initialized-handler-drains-without-publishing

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - the bind and publish failure paths at `runtime.rs:836` and
`:842` have no fixture
Guarantee: The state in which a handler completed initialization and then drained
without the host ever publishing a transport occurs at least once per campaign,
so `PrePublicationCleanup::finish` runs against a fully initialized handler.
Check: `sometimes` - a marker inside `PrePublicationCleanup::finish`
(`runtime.rs:308`), fired only when initialization had returned `Ok`.
`sometimes` and not `reachable` because `finish` is also reached from the
initialization-failure arms at `:666` and `:677`, so a campaign can cover the
function's lines while never producing the operational state that matters: a
*successfully* initialized handler being drained with nothing published. That
distinction is exactly what `:695-696` says the grouping exists to protect.
Fault/timing angle: three entries. `bind_owner_only` failing at `:836`; `publish`
failing at `:843`; and the shutdown token already cancelled at `:831`, which
returns `Ok(None)` and drains through `:856`. The third is the cheapest to
construct.
Required faults and enabling state: for the cheapest form, cancel the shutdown
token between the return of `initialize` and the `is_cancelled` check at `:831`.
For the bind form, occupy or make unwritable the `setup.sock` path inside the
guard's directory. For the publish form, a connection-file write failure.
Confidence: high - [evidence](evidence/rt-a-an-initialized-handler-drains-without-publishing.md).
Verified all three entries, the shared `finish` path, and that `finish` demotes
the phase at `:355-357` before the drain.
Existing check: none. Status `unaudited`.
Impact: this path runs the handler shutdown callback for a handler that never
served a request, while the instance lock is still held. If the callback assumes
publication occurred, or assumes at least one connection existed, the failure
surfaces only here. It is also the path that decides whether a failed startup
leaves a lock behind.
Open questions: None.

---

## Relationship map

Grouped by shared mechanism rather than by the headings above, because the
sharpest relationships cross groups. **Every dominance statement below is a
hypothesis** about which oracle subsumes which, offered to order the work, not a
verified claim. None has been tested, and none can be tested by anything CI runs
today: this sub-part has zero CI-executed source-resident checks and zero
CI-named integration binaries, and the one CI-named binary that reaches these
files at all, `tests/lifecycle.rs`, is Part 2a's and tests lifecycle records
rather than the configuration contract.

- **One validator, three things it does not check.**
  [rt-a-no-configured-limit-is-silently-clamped](#rt-a-no-configured-limit-is-silently-clamped),
  [rt-a-startup-refuses-every-configuration-it-cannot-fund](#rt-a-startup-refuses-every-configuration-it-cannot-fund),
  [rt-a-the-ingress-pool-derivation-cannot-underflow](#rt-a-the-ingress-pool-derivation-cannot-underflow).
  `HostConfig::validate` checks each field in isolation and nothing else: no
  cross-field relationship, no handler-dependent feasibility, no arithmetic at
  the consumer. Hypothesis: an assertion battery placed at `runtime.rs:882`,
  immediately before `HostShared` construction, *dominates the second and third
  outright*, because both of their checks are stated at that exact site and the
  third's whole point is that its guard belongs at the consumer rather than 160
  lines earlier. It dominates the first **not at all**: no-silent-clamping is a
  property of `validate`'s return value, and a battery at the construction site
  runs only on configurations that already passed. The two need two oracles in
  two places, which is worth saying because they read as one cluster.
- **Two fixed bounds and the situation that measures one of them.**
  [rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval](#rt-a-a-fixed-probe-interval-preempts-the-configured-health-interval),
  [rt-a-the-activation-fast-probe-interval-is-entered](#rt-a-the-activation-fast-probe-interval-is-entered),
  [rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline](#rt-a-forced-shutdown-outlives-the-configured-shutdown-deadline),
  [rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline](#rt-a-the-serial-setup-budget-triples-the-configured-transport-deadline).
  All four are the same shape at different sites, and protocol `:731` is the one
  rule all four bear on. Hypothesis: the fast-probe reachability record is a
  **strict prerequisite** of the fixed-probe-interval record rather than
  dominated by it, because the override cannot be measured until the situation is
  produced, and a `health_interval` distinguishable from 50 ms is the one fixture
  change both need (`tests/lifecycle.rs:165` currently sets exactly 50 ms, which
  makes the two branches inseparable). Hypothesis: nothing dominates across the
  three sites. A clamp on the 50 ms path says nothing about the shutdown chain, a
  `timeout_at` at `:1224` says nothing about `connection.rs:158`, and a
  cross-field check in `validate` cannot see any of the three, because all three
  multiply at the *consumer* rather than at the configuration boundary. That is
  the argument for a single review-level census of every `timeout` and
  `timeout_at` in the crate against the key each names, which would dominate all
  three as a static check while proving none of their runtime bounds.
- **The default configuration read from two directions.**
  [rt-a-the-default-configuration-arms-no-liveness-probe](#rt-a-the-default-configuration-arms-no-liveness-probe),
  [rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one](#rt-a-an-unprobed-health-snapshot-is-distinguishable-from-a-degraded-one).
  Both are about what a default host can observe about its own health. The first
  is already `Exercised: yes`, uniquely in this catalog, because
  `tests/lifecycle.rs:496` asserts no Ping arrives within 500 ms. Hypothesis: it
  dominates **nothing**, and that is the interesting part: proving the probe is
  absent says nothing about whether the *other* health signal, the snapshot, is
  interpretable. Conversely the snapshot record needs a slow first `health`
  callback (`tests/lifecycle.rs:579` already builds one) and does not care about
  liveness at all. Two adjacent detection gaps with no shared oracle.
- **Two enumerations that fix assumptions the rest of the catalog rests on.**
  [rt-a-every-published-configuration-field-changes-host-behaviour](#rt-a-every-published-configuration-field-changes-host-behaviour),
  [rt-a-configuration-is-frozen-for-the-incarnation](#rt-a-configuration-is-frozen-for-the-incarnation).
  Neither needs a fault and both are discharged by reading the tree. Hypothesis:
  the frozen-configuration record *dominates every config-dependent record in
  every sibling sub-part* in the weak sense that it licenses their treatment of a
  limit as constant, and it is dominated by nothing, because no runtime oracle
  can observe the absence of a reload path that does not exist. The
  every-field record is the complement: it proves the surface is wired, and its
  one violator is the only field where an embedder's action has no effect. Worth
  building as one census pass, since both walk the same 21 fields.
- **Two startup paths whose failure is invisible.**
  [rt-a-a-closure-store-open-failure-is-classified-not-swallowed](#rt-a-a-closure-store-open-failure-is-classified-not-swallowed),
  [rt-a-an-initialized-handler-drains-without-publishing](#rt-a-an-initialized-handler-drains-without-publishing).
  Both run before any request is served and both are unobserved. Hypothesis: they
  dominate each other not at all, and they fail for opposite reasons. The closure
  record's oracle is blocked *outside* this sub-part, at `serve.rs:162` and
  `:349` where the `.ok()` discards a well-built closed error vocabulary, which
  is why it is the catalog's only `medium`. The drain record's oracle is
  constructible *inside* it, by cancelling the shutdown token between
  `initialize`'s return and the `is_cancelled` check at `:831`, which is the
  cheapest of its three entries. So one is cheap and unbuilt, the other is
  expensive and needs a pass nobody has scheduled.
- **The reserved pools, cited by three sub-parts.**
  [rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration](#rt-a-reserved-pools-are-zero-permit-and-unentered-without-a-declaration).
  Standing alone because its relationship is across parts rather than within
  this one. Its guarantee is conditional on no module declaring a reservation;
  Broca declares one, so 2e's
  [req-a-both-admission-classes-and-the-rejection-bound-saturate](#req-a-both-admission-classes-and-the-rejection-bound-saturate)
  owns the live half, namely that reserved *task* exhaustion is constructed by no
  test. Hypothesis: 2e's five-state saturation campaign *dominates this record's
  entry half*, because a campaign that saturates the reserved task pool has
  necessarily observed that only `Reserved`-class routes acquire from it. It does
  not dominate the zero-permit half, which is about a composition that excludes
  Broca and which no in-tree production configuration produces.

## Discovered at U3

Records added when the crate entered this tree. The first seven cover renamed identities (proof vectors, data root,
coordination locks, route-open body, closure digest, credential fingerprint, bundle fingerprint); the Broca and
Synapse records cover code the source catalogs did not reach and enter at the status observed at discovery, with
their existing checks named and unaudited. This set has its own per-part artifacts under
[`discovered-at-u3/`](discovered-at-u3/): the check inventory
[existing-checks.md](discovered-at-u3/existing-checks.md), the fault map
[fault-map.md](discovered-at-u3/fault-map.md), and the independent
[portfolio-evaluation.md](discovered-at-u3/portfolio-evaluation.md), whose refinements R1, R2, R3 and R7 are
applied below and whose remaining findings are queued there.

**Index.** 16 records, in discovery order.

| Slug | Type | Confidence |
| --- | --- | --- |
| [host-proof-construction-matches-the-committed-vectors](#host-proof-construction-matches-the-committed-vectors) | safety | high |
| [data-root-resolves-under-the-managed-directory](#data-root-resolves-under-the-managed-directory) | safety | high |
| [coordination-locks-live-beside-the-managed-subtree](#coordination-locks-live-beside-the-managed-subtree) | safety | high |
| [canonical-route-open-declares-its-exact-body-length](#canonical-route-open-declares-its-exact-body-length) | safety | high |
| [harness-closure-manifest-digest-is-canonical](#harness-closure-manifest-digest-is-canonical) | safety | high |
| [credential-fingerprint-derives-from-the-product-domain](#credential-fingerprint-derives-from-the-product-domain) | safety | high |
| [synapse-bundle-fingerprint-covers-every-artifact](#synapse-bundle-fingerprint-covers-every-artifact) | safety | high |
| [broca-identical-resends-converge-on-one-run](#broca-identical-resends-converge-on-one-run) | safety | medium |
| [broca-permits-and-charges-return-to-baseline](#broca-permits-and-charges-return-to-baseline) | safety | medium |
| [broca-children-are-reaped-as-a-process-group](#broca-children-are-reaped-as-a-process-group) | safety | medium |
| [broca-child-environment-carries-only-the-provider-row](#broca-child-environment-carries-only-the-provider-row) | safety | medium |
| [broca-protocol-shapes-are-closed](#broca-protocol-shapes-are-closed) | safety | medium |
| [synapse-admission-boundaries-are-exact](#synapse-admission-boundaries-are-exact) | safety | medium |
| [synapse-degrades-to-disabled-and-keeps-the-context-routable](#synapse-degrades-to-disabled-and-keeps-the-context-routable) | liveness | medium |
| [synapse-requests-are-validated-before-any-inference](#synapse-requests-are-validated-before-any-inference) | safety | medium |
| [synapse-inference-runs-through-a-sealed-runtime-image](#synapse-inference-runs-through-a-sealed-runtime-image) | safety | medium |

### host-proof-construction-matches-the-committed-vectors

Type: safety
Reachability: default-production - every client and server handshake computes this proof.
Status: active
Exercised: yes - the crate-internal vector test and the independent `raw_client` oracle each pin their own side to the same committed literal, and `production_proof_matches_the_oracle_across_perturbed_tuples` calls `compute_proof` and `raw_client::proof` on the same tuple for both domains over the committed inputs, each input perturbed alone, daemon versions of several lengths, and short and long keys, asserting equality and distinctness.
Guarantee: The host's `compute_proof` is the shared `shm_transport::setup_auth` transcript with domains `eidnara-server-v1` and `eidnara-client-v1`, and its output over the committed inputs equals the vectors an implementation outside the crate produces.
Check: `always` - `compute_proof(...) == raw_client::proof(...)` for the committed inputs and for every generated or single-field-perturbed input tuple, where `raw_client::proof` is the test-local HMAC implementation of the documented transcript; the equality over arbitrary inputs, not the change under perturbation, is the oracle, and distinct inputs must give distinct proofs. `always` because the transcript is a pure function evaluated on every handshake.
Fault/timing angle: Only an external oracle detects a transcript change both sides apply.
Required faults and enabling state: The committed inputs and the test-local HMAC oracle.
Confidence: high - [evidence](evidence/host-proof-construction-matches-the-committed-vectors.md).
Existing check: `committed_wire_vectors_pin_the_proof_construction` (`crates/host-runtime/src/auth.rs`), `committed_auth_proof_vectors_pin_the_construction`, `proof_folds_every_input`, and `production_proof_matches_the_oracle_across_perturbed_tuples` (`crates/host-runtime/tests/protocol_vectors.rs`); audited at U3.
Impact: A client that cannot authenticate, or a rogue listener that can.
Open questions: None. The section 5.2 examples in `docs/host-wire-protocol.md` were regenerated at U3 to the committed vectors, and the prose there names `eidnara-host/0.1.0`; the evidence record's investigation log carries the reproduction.

### data-root-resolves-under-the-managed-directory

Type: safety
Reachability: default-production - every host start without a data-directory override resolves the root this way.
Status: active
Exercised: yes - every branch of the resolver is exercised without touching process environment.
Guarantee: The default data root is an absolute `XDG_DATA_HOME`, else `$HOME/.local/share` for an absolute `HOME`, else `NoDataDir`; relative or empty values are ignored rather than joined to the working directory; the runtime directory is `<root>/eidnara/run`.
Check: `always` - `default_data_root(xdg, home)` returns the documented root for absolute values and `Err(NoDataDir)` when neither is absolute; `runtime_dir_path` appends `eidnara/run`.
Fault/timing angle: A relative `XDG_DATA_HOME` joined to the working directory would scatter data roots by launch directory.
Required faults and enabling state: Relative, empty, and absent values for both variables.
Confidence: high - [evidence](evidence/data-root-resolves-under-the-managed-directory.md). At U3 the resolver was split so the environment values are arguments; the test no longer calls `set_var`, which Rust 2024 makes unsafe.
Existing check: `default_root_follows_xdg_then_home` and `explicit_override_resolves_canonical_layout` (`crates/host-runtime/src/instance.rs`); audited at U3.
Impact: Two hosts on one machine open different roots for the same user, or one host opens a root under an attacker-chosen directory.
Open questions: None.

### coordination-locks-live-beside-the-managed-subtree

Type: safety
Reachability: default-production - every incarnation materialises both lock files (`crates/host-runtime/src/lifecycle.rs:78-83`, re-verified) and takes `lifetime.lock` through `LifetimeLock::acquire` (`:181`, reached from `InstanceGuard::acquire` at `instance.rs:231` and `runtime.rs:565`). `LifecycleTransactionLock::acquire_exclusive` (`:456`) has only test callers in this tree; the probe takes it shared (`:873`) and the daemon that takes it exclusively is scheduled for U4. The path guarantee therefore reaches production through the lifetime lock and the directory, not through an exclusive transaction lock.
Status: active
Exercised: yes - the lock path literal and the inode identity across a managed-subtree replacement are asserted.
Guarantee: The lifetime and transaction locks live at `<root>/.eidnara-coordination/{lifetime,transaction}.lock`, outside `<root>/eidnara`, so replacing the managed subtree neither moves nor splits the fence, and independent openers see one inode identity.
Check: `always` - both lock files are created under the literal `<root>/.eidnara-coordination/` path on every incarnation, and the `(dev, ino)` of each is identical before and after the managed subtree is renamed away; the named test asserts this for `transaction.lock`, and `successive_incarnations_lock_the_same_coordination_inodes` for both.
Fault/timing angle: A lock inside the replaceable subtree would let a replaced subtree admit a second incarnation.
Required faults and enabling state: Managed-subtree replacement while a lock exists; two independent openers.
Confidence: high - [evidence](evidence/coordination-locks-live-beside-the-managed-subtree.md). The directory name is a renamed identity at U3.
Existing check: `independent_openers_see_one_stable_coordination_identity` (`crates/host-runtime/src/lifecycle.rs`), plus the replaced-subtree tests in the same module; audited at U3.
Impact: Two live hosts, each believing it holds the fence.
Open questions:
- How does the cutover isolation probe treat `.eidnara-coordination`, which sits beside rather than inside the managed subtree it digests? See the evidence file. (needs human input)

### canonical-route-open-declares-its-exact-body-length

Type: safety
Reachability: default-production - every `route.open` carries a header whose declared length the reader trusts for framing.
Status: active
Exercised: yes - the canonical body's byte length and the committed header bytes are asserted against literals.
Guarantee: The canonical compact `route.open` request targeting module `context` is 167 UTF-8 bytes, and the committed control header `a7 00 00 00 02 00 02 00 00 00 00 00 00 01 00 ...` declares exactly that length with version 2, request type, interactive flags, channel 0, epoch 0, correlation 1.
Check: `always` - `canonical.len() == 167`, `raw_client::decode_header(committed).len == canonical.len()`, and `raw_client::header(167, ...) == committed bytes`, decoded by the test-local decoder.
Fault/timing angle: A header that declares the wrong length desynchronizes framing on the first request.
Required faults and enabling state: None; literal comparison against the documented vector.
Confidence: high - [evidence](evidence/canonical-route-open-declares-its-exact-body-length.md). The module id `context` is a renamed identity, so the canonical body shrank from the predecessor's length and the vector was regenerated once; `docs/host-wire-protocol.md` section 6.4 carries the same bytes.
Existing check: `canonical_route_open_body_is_167_bytes` and `committed_header_vectors_decode_to_their_documented_fields` (`crates/host-runtime/tests/protocol_vectors.rs`); audited at U3.
Impact: The first request on every connection is misframed.
Open questions: None.

### harness-closure-manifest-digest-is-canonical

Type: safety
Reachability: test-only - `HarnessClosureStore::open` is called only from
`crates/host-runtime/tests/harness_closure.rs` in this tree, and `manifest_digest`
is reached only through the store; the daemon that opens the store in production
(`crates/daemon`) is scheduled for U4 (`docs/properties/README.md:52`); reclassify
in the wave that lands it.
Status: active
Exercised: yes - the committed fixture's digest is asserted, an independent canonical-JSON digest reproduced both the predecessor and the current value, a key-reordered copy of the fixture digests the same, and each manifest and node field is changed alone and shown to move the digest, with the validator-fixed fields shown to be refused.
Guarantee: The manifest digest is SHA-256 over the manifest serialized as key-sorted, two-space-indented JSON, so any manifest with the same fields hashes the same regardless of field order, and the committed `pi-valid.json` fixture digests to `5386c200...f911`.
Check: `always` - `manifest_digest(fixture) == committed literal`; the digest changes when any field changes and is unchanged under key reordering.
Fault/timing angle: A digest that depended on serialization order would let two equal manifests disagree; a digest over a different canonical form would break the TypeScript twin, which lands with the packages in U7 and reads this fixture.
Required faults and enabling state: The committed fixture and an oracle outside the crate.
Confidence: high - [evidence](evidence/harness-closure-manifest-digest-is-canonical.md). The fixture's `schema` field is a renamed identity, so the digest was regenerated once; a Python `json.dumps(sort_keys=True, indent=2)` digest reproduced the predecessor value from the predecessor schema string and the new value from the new one.
Existing check: `canonical_manifest_digest_is_pinned`, `manifest_digest_is_stable_under_key_reordering`, `manifest_digest_changes_when_any_field_changes`, `launch_roots_participate_in_the_digest_on_their_own`, and the strict-decode tests (`crates/host-runtime/tests/harness_closure.rs`); audited at U3.
Impact: A closure verified by one side is rejected by the other, or a tampered closure passes.
Open questions: None.

### credential-fingerprint-derives-from-the-product-domain

Type: safety
Reachability: test-only - the fingerprint comparison runs only when a verifier is installed, and only `BrocaComponent::new_with_credentials` (`crates/host-runtime/src/broca/mod.rs:82`) installs one; its single caller is `tests/broca_protocol.rs:443`. `BrocaComponent::new` (`:73-80`) sets no verifier, so the default construction path skips the check (`:223-235`). Reclassify when a production constructor installs the verifier.
Status: active
Exercised: yes - the committed vector is asserted, an independent HMAC oracle reproduced it, and a campaign over generated keys, every harness-and-provider pair including both Pi aliases, and value shapes including a multibyte value and two non-UTF-8 byte values agrees with an in-test implementation of the documented derivation and yields distinct fingerprints for distinct rows.
Guarantee: The credential fingerprint is `HMAC(derive(connection_key, "eidnara-broca-credential-v1"), canonical_row)` where the canonical row is length-prefixed fields under canonicalization `harness-provider-name-length-value/1`; the committed vector for the documented inputs is `ecac831b...7e80`.
Check: `always` - for the documented row, `credential_fingerprint(key, harness, provider) == committed literal`; for every generated `(key, harness, provider name, value)` row in a campaign that `provider_row` admits (a supported harness and provider under the closed `canonical_provider` mapping and a nonempty value; an empty value returns `CredentialMissing` at `subprocess.rs:151-174` before any fingerprint exists), `credential_fingerprint` equals an independent implementation of the documented derivation (`HMAC(derive(key, "eidnara-broca-credential-v1"), canonical_row)` with length-prefixed fields), including admissible rows that differ only by moving one byte across a field boundary, which must yield distinct fingerprints; boundary cases that leave a field empty or name an unsupported harness or provider are asserted against a pure canonical encoder of the documented row, not against `credential_fingerprint`, which rejects them before canonicalization; and the per-value size cap rejects before fingerprinting. `always` because the derivation is a pure function evaluated on every row.
Fault/timing angle: A fingerprint that leaked the raw credential or that matched across products would let a captured fingerprint be replayed.
Required faults and enabling state: The documented inputs and an oracle outside the crate.
Confidence: high - [evidence](evidence/credential-fingerprint-derives-from-the-product-domain.md). The domain separator is a renamed identity; the vector was regenerated once from a Python implementation of the documented derivation, which also reproduced the predecessor value from the predecessor domain.
Existing check: `credential_fingerprint_matches_the_committed_vector` and `credential_fingerprint_matches_the_documented_derivation_across_rows` (`crates/host-runtime/src/broca/subprocess.rs`, added at U3) and `provider_rows_exclude_ambient_credentials_and_enforce_caps` (`crates/host-runtime/tests/broca_subprocess.rs`, a `harness = false` binary whose checks are plain functions the binary's own runner names); audited at U3.
Impact: A credential row passes a fingerprint check it should fail, or fails one it should pass.
Open questions: `CREDENTIAL_ROW_CAP_BYTES` is defined in `subprocess.rs` but nothing enforces it; only the 16 KiB per-value cap is checked.

### synapse-bundle-fingerprint-covers-every-artifact

Type: safety
Reachability: test-only - every bundle load through a composed `SynapseComponent` recomputes and compares the fingerprint (`load_bundle` is called only from `crates/host-runtime/src/synapse/mod.rs:1025`), but the component is not on `host_runtime::run`'s default path; an embedder composes it, and in this tree the only compositions are tests and `examples/synapse_host.rs:123`. The daemon that will compose it is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: yes - the committed tiny fixture's fingerprint is recomputed from its manifest and pinned as a literal; each artifact hash, each external-initializer name, each embedding-space scalar, and the numeric output index value is changed alone and shown to move the fingerprint; single-bit artifact changes are caught by each artifact's own digest at load.
Guarantee: The bundle fingerprint is SHA-256 over a newline-joined `key=value` pre-image beginning with `eidnara-synapse-fingerprint-v1` and covering the model file, every external initializer, the four tokenizer artifacts, pooling, quantization, output selector, max tokens, dims, table epoch, and corpus digest; a bundle whose manifest fingerprint disagrees does not load.
Check: `always` - `canonical_fingerprint(manifest) == manifest.fingerprint` for the committed fixture; a bundle whose manifest fingerprint disagrees does not load; and for every field the guarantee names (the model hash, each external-initializer hash and each external-initializer name, since the pre-image binds `name.len():name:sha256` per initializer at `crates/host-runtime/src/synapse/bundle.rs:585-594`, plus the name-to-hash pairing, so swapping two names while keeping every hash also changes the fingerprint; each of the four tokenizer artifact hashes, pooling, quantization, output selection, dimension, and the embedding-space scalars), perturbing that field alone in the manifest changes `canonical_fingerprint`, so no verified input is absent from the pre-image. `always` because the pre-image is a pure function of the manifest.
Fault/timing angle: A fingerprint that omitted an artifact would let a swapped artifact change embedding bytes under an unchanged identity.
Required faults and enabling state: The committed fixture and its generator's independent fingerprint function.
Confidence: high - [evidence](evidence/synapse-bundle-fingerprint-covers-every-artifact.md). The pre-image's first line is a renamed identity; the fixture manifest's fingerprint was regenerated once with the generator's Python `canonical_fingerprint`, which also reproduced the predecessor value from the predecessor line.
Existing check: `the_committed_fixture_carries_its_canonical_fingerprint`, `a_bundle_manifest_outside_the_committed_digest_does_not_load`, `one_bit_changes_to_each_artifact_disable_the_lane` (`crates/host-runtime/tests/synapse_bundle.rs`), and `every_artifact_hash_and_embedding_scalar_participates_in_the_fingerprint` (`crates/host-runtime/src/synapse/bundle.rs`); audited at U3.
Impact: A different model produces embeddings under the identity of the certified one.
Open questions: None.

### broca-identical-resends-converge-on-one-run

Type: safety
Reachability: test-only - every Broca send through a composed `BrocaComponent` is deduplicated by the supervisor. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - identical resends and racing identical sends are covered; a resend after the run's terminal was retained then evicted is not.
Guarantee: While a session entry is retained, byte-identical resends of `session.send` converge on one backend run and a differing body for the same key is rejected as a conflict. Retention ends when `TERMINAL_RETENTION` (15 minutes, `crates/host-runtime/src/broca/config.rs:126`) expires or `enforce_terminal_cap` evicts the entry beyond `MAX_TERMINAL_SESSIONS` (256, `:122`); a resend after that legitimately starts a new run.
Check: `always` - within the retention of a session entry, `runs_started <= 1` per identical send key and a differing body returns the conflict terminal; the campaign reads `terminal_retention` and the cap from the supervisor limits and stops counting a key only once `sweep_for` or `enforce_terminal_cap` (`supervisor.rs:1085`, `:1005`) has removed it or `session.delete` has replaced the live run with a retained `SessionEntry::Tombstone` (`:509-527`); the conflict assertion is scoped to live entries, and a send against a tombstone must return `session_deleted` (`:356-372`) rather than the conflict terminal until the tombstone expires or is evicted; and a session entry is present until `terminal_retention` has elapsed since its terminal or the cap has been exceeded, so a removal before either condition holds fails the check rather than ending the count.
Fault/timing angle: Two harness clients retry the same prompt concurrently.
Required faults and enabling state: Concurrent identical sends; a differing resend under the same key.
Confidence: medium - [evidence](evidence/broca-identical-resends-converge-on-one-run.md). `identical_resend_dedups_and_any_byte_difference_conflicts`, `racing_identical_sends_converge_on_one_run_and_one_backend_start` (`crates/host-runtime/tests/broca_supervisor.rs`).
Existing check: The two tests named above; unaudited.
Impact: Two model calls billed and two divergent transcripts for one prompt.
Open questions: None.

### broca-permits-and-charges-return-to-baseline

Type: safety
Reachability: test-only - every run path of a composed `BrocaComponent` releases what it took. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - success, failure, cancel, transport detach, and shutdown paths are covered in-process; a backend that never exits is covered only through the escalation timers.
Guarantee: Every run path returns its pending permits, task permits, and byte charges to the supervisor baseline, and host shutdown drains the supervisor to zero state; when an uncooperative backend outlives the termination grace, shutdown reports the unresolved count to the caller instead of claiming zero state.
Check: `always` - at terminal commitment the run slot is released; once `work_done` is set or the run task has quiesced, the supervisor's pending permits and task permits equal their starting values and the run's excess bytes are released, because the run task retains `_backend_permit` until backend teardown finishes (`crates/host-runtime/src/broca/supervisor.rs:748-782`), `finish` (`:938`) releases the excess only when `work_done` is already true (`:989-995`), and `DoneGuard` releases it at task exit otherwise (`:792-809`), so a committed `Cancelled` terminal may legitimately coexist with a held backend permit until then; while the retained session's base charge and replay frames are still held for `terminal_retention`; the full byte-budget baseline is required only once `remove_session` (`:1059`) has removed that entry by expiry, cap eviction, deletion, or shutdown; after shutdown, either the state is empty and the unresolved count `shutdown` returns (`crates/host-runtime/src/broca/supervisor.rs:611`, `:630-633`) is zero, or the count is nonzero and exactly equals the number of runs whose final `work_unresolved` verdict is set (`supervisor.rs:629-634` counts unproven teardowns, not processes live at inspection time, and a process may exit after `terminate_group` fails to confirm it), with no permit, charge, or run state retained; a zero count with retained state, or a nonzero count that is not surfaced to the caller, fails the check.
Fault/timing angle: A leaked permit shrinks the admission pool until the host restarts.
Required faults and enabling state: Each terminal path: success, error, cancel, detach, shutdown.
Confidence: medium - [evidence](evidence/broca-permits-and-charges-return-to-baseline.md). `every_path_returns_permits_and_charges_to_baseline`, `host_shutdown_drains_the_supervisor_to_zero_state`, `transport_detach_paths_leave_the_run_untouched` (`crates/host-runtime/tests/broca_supervisor.rs`).
Existing check: The tests named above; unaudited.
Impact: Slow admission collapse of the Broca lane.
Open questions: None.

### broca-children-are-reaped-as-a-process-group

Type: safety
Reachability: test-only - every harness child a composed `BrocaComponent` spawns runs in its own process group under `PR_SET_PDEATHSIG`. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - SIGTERM-then-SIGKILL reaping on cancel, delete, and shutdown is covered with real processes; the orphan sweep is covered for dead owners.
Guarantee: On cancellation, deletion, or shutdown, every harness child's process group is terminated within four applications of `termination_grace`, or `terminate_group` reports the group unresolved and the operation that triggered the teardown surfaces `teardown_unconfirmed` (cancel and delete return it; shutdown reports the unresolved count); the orphan sweep never signals a group whose owner is alive.
Check: `always` - measured from the cancellation, deletion, or shutdown instant, `terminate_group` (`crates/host-runtime/src/broca/subprocess.rs:670`) completes within four applications of `termination_grace` (the TERM wait, the KILL wait, the member sweep, and the bounded leader reap at `:679-693`), and the backend task finishes within that bound plus the fixed one-second stdin-task wait at `:554-564`; at completion either no process of the reaped group survives, or `terminate_group` has reported the group unresolved and the classification is surfaced on the operation result, `teardown_unconfirmed` from cancel or delete (`supervisor.rs:560`, after the `Cancelled` terminal has already been committed at `:456-465` and `:486-496` and cannot be replaced, `:767-781`) or the unresolved count from `shutdown`; the terminal itself is not required to carry the classification. The sweep never signals a group whose owner is alive. A teardown that exceeds the bound, or that never surfaces the result, fails the check rather than deferring it.
Fault/timing angle: A grandchild that survives its parent keeps a credential in its environment.
Required faults and enabling state: A child that ignores SIGTERM; a forked grandchild; a dead owner with a live group.
Confidence: medium - [evidence](evidence/broca-children-are-reaped-as-a-process-group.md). `cancel_reaps_group_with_sigterm_first`, `sigkill_escalation_when_term_ignored`, `supervisor_shutdown_reaps_group`, `group_registry_sweep_kills_only_dead_owner_groups` (`crates/host-runtime/tests/broca_subprocess.rs`, `harness = false` runner).
Existing check: The checks named above; unaudited.
Impact: Orphaned model processes holding credentials.
Open questions:
- `supervisor_shutdown_reaps_group` discards the unresolved count `shutdown()` returns (`tests/broca_subprocess.rs:2659`), so it cannot refute a late kill; the cancel and delete variants can. Strengthen it or record the shutdown path as `partial`. (needs human input)

### broca-child-environment-carries-only-the-provider-row

Type: safety
Reachability: test-only - `EnvSnapshot::capture_from` (`crates/host-runtime/src/broca/subprocess.rs:97`) and `BrocaComponent::new_with_credentials` (`broca/mod.rs:82`) have no caller outside tests in this tree, and `OpenCodeBackend::new` and `PiBackend::new` have none at all; the spawn path is exercised by fixtures only. Reclassify when the daemon (U4) wires a real backend.
Status: active
Exercised: partial - launch-identity stripping, per-entry overhead, ambient-credential exclusion, and the size caps are covered; the OpenCode and Pi argv contracts are covered by fixture executables.
Guarantee: A snapshot admitted through `EnvSnapshot::capture_from` (`crates/host-runtime/src/broca/subprocess.rs:97`) has the launch identity stripped and is charged per entry and in aggregate, and the harness child spawned from it receives only the selected provider credential row plus adapter-owned variables.
Check: `always` - for a snapshot built by `capture_from`, the spawned environment contains no `EIDNARA_MODULE_ID` or `EIDNARA_LAUNCH_NONCE`, exactly the selected provider variable with its value under the 16 KiB per-value credential cap, and each adapter-owned variable within its own adapter bound (`OPENCODE_CONFIG_CONTENT` up to `MAX_OPENCODE_CONFIG_BYTES`, 96 KiB at `crates/host-runtime/src/broca/config.rs:19`, added at `opencode.rs:122-170`), so the credential cap is not applied to adapter-owned entries; and the aggregate and per-entry charges are applied; the property is scoped to `capture_from` because the public `from_vars` (`:122`) bypasses that accounting.
Fault/timing angle: A leaked launch identity lets the child impersonate the module; a leaked ambient credential reaches a harness the user did not choose.
Required faults and enabling state: An environment with several provider credentials and the launch identity set.
Confidence: medium - [evidence](evidence/broca-child-environment-carries-only-the-provider-row.md). `env_snapshot_strips_launch_identity`, `env_snapshot_admission_charges_per_entry_overhead`, `provider_rows_exclude_ambient_credentials_and_enforce_caps` (`crates/host-runtime/tests/broca_subprocess.rs`), `credential_snapshot_must_match_before_backend_spawn` (`crates/host-runtime/tests/broca_protocol.rs`).
Existing check: The checks named above; unaudited.
Impact: Credential exfiltration through a harness child.
Open questions:
- `EnvSnapshot::from_vars` (`subprocess.rs:122`) is public and skips the aggregate-byte and per-entry-overhead accounting that `capture_from` applies before calling it (`:98`); an embedder that passes a `from_vars` snapshot to `new_with_credentials` retains an unbounded ambient snapshot. The selected provider value is still capped at spawn. Gap: either make `from_vars` private or account in it. (needs human input)
- `CREDENTIAL_ROW_CAP_BYTES` (`subprocess.rs:51`) has no reader: should it be enforced on the selected row or removed? (needs human input)

### broca-protocol-shapes-are-closed

Type: safety
Reachability: test-only - every request a composed `BrocaComponent` receives is decoded against the closed shape set. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: yes - each valid operation decodes its exact schema, every enumerated malformed shape is rejected, and the 512 KiB boundary is exact.
Guarantee: The Broca application protocol accepts exactly the enumerated operations with their exact schemas; unknown fields, wrong types, and oversize bodies are `schema_violation` terminals, an unsupported harness name is rejected at bind as `invalid_identity`, and malformed requests create no run state.
Check: `always` - every malformed shape is rejected with `schema_violation`, a 512 KiB body is admitted and one byte more is rejected, a bind naming a harness outside the supported set is rejected with exactly `invalid_identity` (`bind_requires_absolute_root_nonempty_session_and_supported_harness`, `crates/host-runtime/tests/broca_protocol.rs:372`, asserts the code at `:397`), and a rejected request or bind leaves no run state; every clause is an invariant over every request, so one `always` covers the conjunction.
Fault/timing angle: A permissive decoder lets a harness smuggle fields the host does not validate.
Required faults and enabling state: Malformed and boundary-sized bodies.
Confidence: medium - [evidence](evidence/broca-protocol-shapes-are-closed.md). `each_valid_operation_decodes_its_exact_schema`, `every_malformed_shape_is_rejected_with_schema_violation`, `the_512kib_boundary_admits_exactly_and_rejects_one_byte_over`, `malformed_requests_over_the_host_create_no_run_state`, `harness_vocabulary_is_closed` (`crates/host-runtime/tests/broca_protocol.rs`).
Existing check: The tests named above; unaudited.
Impact: Unvalidated input reaches the harness spawn path.
Open questions: None.

### broca-payload-hook-owns-the-generation-controls

Type: safety
Reachability: test-only - every Pi run loads the compiled-in hook as the last `--extension` after `--no-extensions` disables discovery, so the hook is the final `before_provider_request` handler on every provider request; but `PiBackend::new` and `run_pi` have no caller outside `crates/host-runtime/tests/broca_subprocess.rs` in this tree, so no production request reaches the hook until the daemon (U4) wires a real backend. Reclassify with the other Broca records then.
Status: active
Exercised: partial - a driver that registers a tampering handler ahead of the hook covers the OpenAI-style, Gemini-style, and mixed-spelling payloads plus one unrecognized shape; nothing runs the hook inside a real Pi process or covers a missing or non-numeric environment value.
Guarantee: The provider payload Pi sends carries exactly the output-token bound and temperature the `session.send` request admitted: every recognized output-token spelling present on the payload and `generationConfig.maxOutputTokens` are rewritten to the request's `max_output_tokens`, `temperature` follows it, every unrelated field survives, and a payload with no recognized output-token field or a non-object payload fails the request rather than running uncapped.
Check: `always` - for every payload the hook returns, each recognized output-token field equals the admitted bound and `temperature` equals the admitted temperature, fields the hook does not own are byte-identical to the input, and a payload with no recognized field throws; the rewrite is an invariant over every provider request, so one `always` covers the conjunction.
Fault/timing angle: An earlier trusted extension leaves a larger limit in a second spelling, or a provider adds a wire family the hook does not recognize; either lets a provider default exceed the caller's budget.
Required faults and enabling state: A payload touched by an earlier handler; a payload carrying two output-token spellings; a payload with no recognized spelling.
Confidence: medium - [evidence](evidence/broca-payload-hook-owns-the-generation-controls.md). `pi_broca_hook_owns_generation_controls` (`crates/host-runtime/tests/broca_subprocess.rs`, `harness = false` runner) materializes the hook bytes from `PI_BROCA_EXTENSION_BYTES` and drives them under Node or Bun.
Existing check: The check named above; unaudited.
Impact: A provider request runs with a token budget or temperature the caller did not admit.
Open questions: None.

### synapse-admission-boundaries-are-exact

Type: safety
Reachability: test-only - every batch and query a composed `SynapseComponent` receives is admitted through these bounds. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - count and byte boundaries, eviction order, and expiry are covered with a deterministic engine; the bounded-waiter test that opens 33 ring clients is ignored because the host admits at most 8 rings per process.
Guarantee: Job admission is exact at the count and queued-byte boundaries, never evicts live work, evicts completed jobs oldest first under count pressure, and reports expired jobs as `module_restarted`.
Check: `always` - the boundary-plus-one request is rejected and the boundary request admitted; no live job is evicted; under count pressure the completed job evicted is the one with the oldest `completed_at`; an expired job is reported as `module_restarted` (`jobs.rs:624`, exact `>=` on retention); and, at completion, the excess over the retained key-and-metadata bytes is released (`publish_ready` splits it off at `jobs.rs:453-455`) while the retained remainder is held by the completed job for polling and returns only when the job is removed, evicted, expired, or cleared (`jobs.rs:96-100`). Every clause is an invariant over every admission, eviction, expiry, and completion, so one `always` covers the conjunction.
Fault/timing angle: Off-by-one at the boundary or eviction of live work loses a caller's result.
Required faults and enabling state: Boundary-sized admission; completion under count pressure; expiry.
Confidence: medium - [evidence](evidence/synapse-admission-boundaries-are-exact.md). `admission_count_boundary_is_exact_and_never_evicts_live_work`, `queued_byte_boundary_is_exact_and_releases_on_completion`, `completed_jobs_evict_oldest_first_under_count_pressure`, `expired_jobs_return_module_restarted` (`crates/host-runtime/tests/synapse_jobs.rs`).
Existing check: The tests named above; unaudited.
Impact: A lost or silently duplicated embedding job.
Open questions:
- Whether `boundary_waiters_with_maximal_texts_are_all_admitted` (`crates/host-runtime/tests/synapse_protocol.rs:415`, a query-waiter admission test, not a job-table test) should be rewritten for the eight-ring admission cap or dropped; it is `#[ignore]` with that reason (`:412-414`). (needs human input)

### synapse-degrades-to-disabled-and-keeps-the-context-routable

Type: liveness
Reachability: test-only - every artifact fault in a composed `SynapseComponent` takes this path. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - missing, corrupt, extra, wrong-identity, and wrong-pooling artifacts disable the lane while the context module stays routable; a fault during inference itself is covered only by the deterministic engine.
Guarantee: An unconfigured or faulted Synapse bundle disables the Synapse lane and is never host-fatal; the context module keeps serving requests, and a bind to the disabled lane is refused with `artifact_invalid`.
Check: `always` - for the unconfigured component (`SynapseComponent::new(None)`, as built at `crates/host-runtime/tests/synapse_bundle.rs:227`) and for every artifact fault, `activate` returns `Ok` with the lane disabled, a bind to the disabled lane is refused with exactly `artifact_invalid` (`crates/host-runtime/tests/synapse_bundle.rs:241`, `tests/synapse_roundtrip.rs:93`), and a context request issued afterwards completes within the campaign's request deadline; the existing test bounds it with the 5 s harness `BUDGET` (`crates/host-runtime/tests/support/synapse.rs:22`, `:265`), and the host itself imposes no dispatch deadline (see [req-a-a-handler-outliving-every-host-deadline-is-reached](#req-a-a-handler-outliving-every-host-deadline-is-reached)), so the bound must come from the campaign. The second clause is asserted inside the same faulted scenario (`corrupt_bundle_degrades_synapse_and_keeps_context_routable`), so it is part of the invariant rather than a separate coverage obligation.
Fault/timing angle: A host-fatal Synapse fault would take the product down for an optional lane.
Required faults and enabling state: Each artifact fault class; an unconfigured component.
Confidence: medium - [evidence](evidence/synapse-degrades-to-disabled-and-keeps-the-context-routable.md). `unconfigured_component_is_disabled_not_fatal`, `one_bit_changes_to_each_artifact_disable_the_lane`, `missing_artifact_disables_the_lane`, `wrong_ort_identity_disables_the_lane`, `corrupt_bundle_degrades_synapse_and_keeps_context_routable` (`crates/host-runtime/tests/synapse_bundle.rs`, `crates/host-runtime/tests/synapse_roundtrip.rs`).
Existing check: The tests named above; unaudited.
Impact: The whole host fails because an embedding model is missing.
Open questions: None.

### synapse-requests-are-validated-before-any-inference

Type: safety
Reachability: test-only - every Synapse request to a composed `SynapseComponent` is decoded and bounded before it reaches the engine. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - constraint violations, unknown fields, excessive depth, oversize bodies, and replay reuse are covered with a deterministic engine that counts calls.
Guarantee: A request that violates a constraint, carries an unknown field, exceeds the depth or size bound, names a different model, fingerprint, or epoch, or names a foreign job is rejected before the engine runs (as `schema_violation`, `substitution_rejected`, or `module_restarted` by class), and equal replays of a queued, running, ready, or permanently failed job reuse that job and one inference, while an equal replay after a retained retryable failure admits a new job.
Check: `always` - `engine.calls` is unchanged by a rejected request; the rejection code matches the violation class exactly: `schema_violation` for a constraint violation, an unknown field, or an exceeded depth or size bound, `substitution_rejected` for a different model, fingerprint, or epoch, and `module_restarted` for a foreign or unknown job (`unknown_and_foreign_jobs_are_module_restarted`, `crates/host-runtime/tests/synapse_protocol.rs:1218`); and equal replays of a queued, running, ready, or permanently failed job produce exactly one inference, while an equal replay after a retained retryable failure such as `internal_error` removes the failed job and admits a new one (`JobTable::admit_charged`, `crates/host-runtime/src/synapse/jobs.rs:324-368`, pinned by `an_identical_retry_replaces_a_failed_job`), so a second inference on that path is the supported outcome and not a violation. Every clause is an invariant over every request, so one `always` covers the conjunction.
Fault/timing angle: Validation after inference would spend model time on hostile input.
Required faults and enabling state: Each violation class; replayed requests.
Confidence: medium - [evidence](evidence/synapse-requests-are-validated-before-any-inference.md). `embed_query_rejects_every_constraint_violation`, `embed_batch_validation_creates_no_job_and_no_inference`, `an_unknown_top_level_field_is_rejected_without_reading_its_value`, `a_routed_depth_nine_request_is_a_schema_violation`, `equal_replays_reuse_one_job_and_one_inference` (`crates/host-runtime/tests/synapse_protocol.rs`).
Existing check: The tests named above; unaudited.
Impact: Model time spent on requests that were never valid.
Open questions: None.

### synapse-inference-runs-through-a-sealed-runtime-image

Type: safety
Reachability: test-only - every inference in a composed `SynapseComponent` loads ONNX Runtime through the sealed memfd path. The component is not on `host_runtime::run`'s default path; an embedder composes it into the handler, and in this tree the only compositions are tests and `crates/host-runtime/examples/` (`synapse_host.rs:123`, `synapse_perf.rs`). The daemon that will compose it in production is scheduled for U4 (`docs/properties/README.md:52`); reclassify then.
Status: active
Exercised: partial - `source_replacement_cannot_change_verified_loader_bytes` asserts the seals, rejected writes, replacement resistance, and the digest on the memfd path; the full load into ONNX Runtime is exercised only where the runtime library is present.
Guarantee: The ONNX Runtime library is loaded from a sealed memfd named `host-onnxruntime` whose bytes were certified with the bundle, so a library swapped on disk after certification cannot reach inference.
Check: `always` - the loaded image's digest equals the certified digest, and the memfd carries the shrink, grow, write, and seal seals (`F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_SEAL`, as applied at `crates/host-runtime/src/synapse/inference.rs:152-159`), so the image can neither be modified, grown, truncated, nor unsealed after certification; and the ONNX Runtime object actually mapped into the process is that memfd, asserted by matching the `host-onnxruntime` memfd entry in `/proc/self/maps` against the loaded library's mapping, so a loader that seals one image and initialises from a filesystem path fails the check; all three are invariants over every load, so one `always` covers the conjunction.
Fault/timing angle: A library swapped between certification and load changes every embedding.
Required faults and enabling state: A modified library on disk after certification; a memfd without seals.
Confidence: medium - [evidence](evidence/synapse-inference-runs-through-a-sealed-runtime-image.md). `source_replacement_cannot_change_verified_loader_bytes` (`crates/host-runtime/src/synapse/inference.rs`) observes the seals and the digest.
Existing check: `source_replacement_cannot_change_verified_loader_bytes` (`crates/host-runtime/src/synapse/inference.rs`); unaudited.
Impact: Embeddings from an uncertified runtime under a certified identity.
Open questions:
- Whether `ort::init_from` loads from the given `/proc/self/fd/<n>` path and nothing else is unverified from this tree; it needs the `ort` source or a `/proc/self/maps` assertion. (needs human input)
