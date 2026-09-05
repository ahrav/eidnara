# publish-signal-implies-committed-frame

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

Comparing the two sides of the publish hook. The host stores its completion marker *after* commit returns; the
TypeScript client sets its `published` flag *inside* the hook, which the native layer invokes *before* commit.
Both are internally consistent, so neither side's tests catch it. The disagreement is visible only on the one
input where the orderings differ: a commit that fails after the hook has already fired.

## Evidence trail

- `packages/shm-native/src/lib.rs:998-1079` `produce` — reserves with the caller's wire header (`:1022-1029`),
  runs the fill callback (`:1049`), advances the cursor (`:1064-1072`), then:
  ```rust
  before_publish.call(())?;
  reservation
      .commit(written)
      .map_err(|_| error("producer underfill or invalid commit"))?;
  ```
  (`:1073-1077`). The hook still runs before commit is attempted, but at HEAD it is no longer unconditional:
  `check_wire_header(&header, written_len)` at `:1071-1072` refuses a header that disagrees with the committed
  body before the hook fires, so the only commit failures that can follow the hook are quarantine, `Underfill`,
  `CommitOutsideReservation`, and a `prepare_commit` rejection of a descriptor page a peer rewrote in between.
- `packages/shm-native/src/lib.rs:1202-1207` — the same ordering on the two-phase `commit_reservation` entry
  point: `before_publish.call(())?` then `reservation.commit(written as usize)`.
- `packages/plugin/src/shared/host-client/shm-frame-channel.ts:289-321` (source tree; not at HEAD) `publishFrame` —
  `let published = false;` (`:296` (source tree; not at HEAD)), and the callback passed as the native before-publish hook sets
  `published = true;` (`:303` (source tree; not at HEAD)) then invokes `hooks?.onPublish?.()` (`:305` (source tree; not at HEAD)). The ticket returned at `:321` (source tree; not at HEAD) is
  `{ cancel: () => !published }`, so once the hook has run the frame is uncancellable by contract.
- `crates/host-runtime/src/ring_transport.rs:749-786` `publish_one` — the host's ordering is the opposite: the publish
  attempt is wrapped at `:769-772`, then `if !matches!(result, Ok(Ok(()))) { return Err(()); }` (`:773-775`), and
  only then `completion.store(COMPLETE, Ordering::Release)` (`:591` (source tree; not at HEAD)) followed by the hook at `:776-780`. The host
  never marks a failed commit complete.
- `crates/shm-transport/src/backend/ring.rs:2536-2570` `commit` — five failure branches, all aborting the
  reservation: `Aborted` (`:2537-2539`), `CommitOutsideReservation` (`:2546-2550`), `Underfill` (`:2551-2555`),
  and, in the source tree, any error from the transport's `Ring::commit_reservation` (`:2562-2566`); at HEAD that
  helper is `prepare_commit` (`:2308-2343`), distinct from the addon's N-API entry point of the same name
  (`packages/shm-native/src/lib.rs:1159`), which still exists.
- `ring.rs:2316-2317` — in the source tree, inside `Ring::commit_reservation`,
  `if declared_len as usize != exact_len || wire_header[4] != 2` returns `ProducerError::WireHeaderMismatch`.
  In the source tree this record was written against, the addon fixed the header at *reserve* time but committed
  the length the fill callback reported, so a fill that under-advanced produced this failure with no injected
  fault. At HEAD both addon paths run `check_wire_header` against the committed count before the hook
  (`lib.rs:1071-1072`, `:1192-1193`; pinned by `tests/mechanism.ts:703`), so an under-advancing fill is refused
  before `before_publish` and this branch is no longer reachable after the hook from the addon.
- `ring.rs:2271-2282` `abort_reservation` — the failure path stores `SLOT_FREE` and never touches `published`,
  so the peer genuinely sees no frame.
- Existing check, **corrected and re-anchored at post-#131 HEAD**: the catalog cites
  `packages/shm-native/tests/runtime.ts:113-128`.
  `runNativeLifecycle` begins at `:110`, its publish hook is `:123-127`, and the assertion
  `assert.equal(publishSawDetached, true)` is at `:129` — just outside that range. The accurate span is
  `:110-129`. What it pins is the hook's *position* relative to alias detachment, not commit success. Status
  unaudited.
  At HEAD: the declared-length and version comparison is delegated to `check_wire_header` inside `prepare_commit`, not written out as a declared_len versus exact_len comparison plus a wire_header[4] test.
  At HEAD: The fifth branch now aborts on any error from `prepare_commit` (`:2308-2343`); the transport's `Ring::commit_reservation` no longer exists (the addon's N-API `commit_reservation` at `lib.rs:1159` does), and a Quarantined branch sits at `:2541-2545`.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The client calls `publishFrame`; the header declares `len = N`.
2. Native `produce` reserves capacity `N` with that header.
3. The fill callback advances the cursor by `M != N` — an under-filling `DirectFrameBody.fill`, a partial
   serializer, or a caught error inside `fill`.
4. `advance(M)` succeeds; `before_publish` fires. The client sets `published = true` and calls
   `hooks.onPublish()`.
5. `commit(M)` reaches `commit_reservation`, which compares the header's declared `N` against `M` and returns
   `WireHeaderMismatch` (`ring.rs:2316-2317`).
6. `commit` aborts the reservation (`:2563`) and the slot returns to `SLOT_FREE`. `published` is never advanced,
   so the peer will never see this frame.
7. Native returns `Err("producer underfill or invalid commit")` (`lib.rs:1076`).
8. Consequence: the sender's `onPublish` has already fired for a frame that does not exist and will never be
   delivered. There is no retry on this transport, and `cancel()` would report `false` — not cancellable — for a
   frame that was never published.

## Timing windows and dependencies

The window is the interval between `before_publish.call(())` and `commit`'s return — `lib.rs:1073-1076` and
`:1202-1207`. It contains one JavaScript callback invocation and one commit, so it is short but entirely
deterministic: it is entered on every publish and the outcome depends only on whether commit succeeds. No
configuration dependency, no platform gating. This is a client-side property: the host path
(`ring_transport.rs:773-780`) is ordered correctly, so a host-only test cannot observe it. It interacts with
`no-frame-observable-before-commit`, which establishes the other half — the peer really does see nothing — and
that is what makes the client's signal wrong rather than merely early.
At HEAD: the host returns Err on a failed publish (`:773-775`) and only then runs the hook (`:776-780`); there is no completion marker left to store after commit.

## What a test must construct

No process kill and no memory fault. Construct it from the TypeScript surface with a `DirectFrameBody` whose
`fill` advances the cursor by fewer bytes than `byteLength` declares, then assert: `produce` threw;
`hooks.onPublish` was *not* called, or if the contract permits calling it, that the caller was given a
distinguishable signal that publication failed; and that the peer's `try_receive` returns `Ok(None)` for a
bounded window afterwards. A second arm should inject the failure at the two-phase entry point
(`lib.rs:1202-1207`) so both call sites are covered. A third arm should assert the host path stays correct under
the same fault, so the test documents the asymmetry rather than the symptom. Coverage check to emit:
`shm_commit_failed_after_publish_hook`.

## Investigation log

### Q: Does the client's `FrameSendTicket.cancel()`/`onPublish` contract mean "handed to the transport" or "committed"?

- Sources examined: `packages/plugin/src/shared/host-client/shm-frame-channel.ts:289-321` (source tree; not at HEAD);
  `packages/shm-native/src/lib.rs:998-1079` and `:1159-1210`; `crates/host-runtime/src/ring_transport.rs:749-786`;
  `crates/shm-transport/src/backend/ring.rs:2536-2570`, `:2308-2386`;
  `packages/shm-native/tests/runtime.ts:110-129`.
- Findings: the *mechanics* are settled and verified — the hook precedes commit on both native paths, the
  client's flag is set inside the hook, the host's marker follows commit, and `WireHeaderMismatch` is a
  reachable post-hook commit failure that needs no failpoint. What is not settled is which meaning the
  `FrameSendTicket` contract intends. Nothing in the channel source, the frame-channel contract helper, or
  `docs/shm-transport.md` states whether `onPublish` promises "the transport accepted this frame" or
  "this frame is receivable by the peer". The TCP channel is the natural comparison for intent, but its ordering
  is not evidence about the shared-memory contract's intent.
- Missing evidence: a written statement of the ticket contract. There is no specification text to read, and the
  two implementations embody different answers, so the code cannot arbitrate.
- Conclusion: needs human input. The correct oracle depends entirely on the intended meaning, and inventing one
  would make the test assert a preference rather than a contract. Until it is answered the property stays
  `medium`, and the test above should assert only the fact both readings agree on: the peer sees no frame.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 36, `packages/shm-native/src/lib.rs:1090-1093` now `packages/shm-native/src/lib.rs:1202-1207`: At HEAD both `commit_reservation` and `produce` validate the header against the committed count with `check_wire_header` before the hook runs (`:1192-1193`, `:1071-1072`; pinned by `a header that disagrees with the body is refused before beforePublish runs`, `tests/mechanism.ts:703`), so neither path can reach a post-hook WireHeaderMismatch; the post-hook failures left are quarantine, `Underfill`, `CommitOutsideReservation`, and a `prepare_commit` rejection of a descriptor page a peer rewrote after the pre-hook check.
  - line 48, `:1839-1843` now `:2610-2614`: The fifth branch now aborts on any error from `prepare_commit` (`:2356-2391`); the transport's `Ring::commit_reservation` no longer exists, and a Quarantined branch sits at `:2589-2593`.
  - line 49, `ring.rs:1591-1593` now `ring.rs:2316-2317`: At HEAD the declared-length and version comparison is delegated to `check_wire_header` inside `prepare_commit`, not written out as a declared_len versus exact_len comparison plus a wire_header[4] test.
  - line 85, `ring_transport.rs:588-591` now `ring_transport.rs:773-780`: At HEAD the host returns Err on a failed publish (`:773-775`) and only then runs the hook (`:776-780`); there is no completion marker left to store after commit.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 38, `packages/plugin/src/shared/host-client/shm-frame-channel.ts:289-321` (publishFrame and the FrameSendTicket contract): `packages/plugin` is absent from this tree and has no successor here, so the client half of the comparison cannot be resolved against HEAD.
  - line 39, `:296` (let published = false): Same missing file.
  - line 40, `:303` (published = true inside the before-publish hook): Same missing file.
  - line 40, `:305` (the hooks.onPublish invocation): Same missing file.
  - line 40, `:321` (the returned ticket with cancel): Same missing file.
  - line 44, `:591` (completion.store(COMPLETE, Ordering::Release)): `publish_one` stores no completion marker at HEAD; after the hook it invokes an optional written(Instant::now()) callback (`:781-783`) and drops the charge.
  - line 104, `packages/plugin/src/shared/host-client/shm-frame-channel.ts:289-321` (publishFrame): `packages/plugin` is absent from this tree.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
