# ordinal-memo-promotion-is-owned

## Discovery trigger

Parent #525 requires pass-local ordinal work and forbids an obsolete resolver
from promoting shared memo state.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner` after merging
`origin/main` at `5def3c71`. The session memo field is `state.ordinals`
(`rust-mode-transform.ts:151`). Line numbers were verified against that tree.

- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts)
  charges a copy of every existing entry and ID before the first await
  (`:1006-1009`), builds `stagedMemo` as that copy (`:1028-1031`), passes it
  to `primeOrdinalMemo` and `annotateOrdinals` (`:1164-1182`), rechecks the
  capture between the prime and the annotation (`:1172`), shifts it for a
  continuation base (`:1447-1466`), and assigns `state.ordinals = stagedMemo`
  (`:1478`) only after `replaceHostArrayContents` (`:1476`).
- `invalidateWireState` (`:815-832`) and `clearSession` (`:1538-1540`) reset
  or drop the memo by their own contract and request lease cancellation, so a
  pass in flight declines at its next `assertCurrentPass` (`:957-962`).
- [module-wire.ts](../../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts)
  exports `primeOrdinalMemo` (`:473`) and `annotateOrdinals` (`:533`), which
  charge `ORDINAL_ENTRY_RETAINED_BYTES` per row (`:22`, `:434`) through the
  caller's `reserve` and honor the lease signal.
- Reachability is `explicit-config-only`: the Rust-mode
  [hook](../../../../../packages/opencode-plugin/src/hooks/context/hook.ts)
  (`:337-345`) calls `run`;
  [resolveTransformMode](../../../../../packages/opencode-plugin/src/config/transform-mode.ts)
  gates Rust activation on configuration and user-tier consent.

## Failure scenario

An obsolete pass leaves entries or an anchor that a later pass trusts even
though the obsolete pass did not publish.

## Timing windows and dependencies

The window spans ordinal scans, their yield boundaries, transport, retry, the
continuation shift, and publication. Cancellation and failed reservations must
not publish staged work. A series whose source changed between pages completes
its transport and is refused at publication; the staged memo is dropped there.

## What a test must construct

Start with known shared memo entries and metadata. Pause an ordinal scan or
transport, invalidate or supersede the pass, then settle it. Compare the full
shared memo with an independent pre-pass copy. Also test a successful
promotion and reservation failure. A size-only assertion does not establish
preservation of entries or metadata.

## Investigation log

### Q: Are all stale and failed ordinal paths isolated from shared state?

- Sources examined: the staging and promotion sites above, `primeOrdinalMemo`
  and `annotateOrdinals`, and the witnesses below.
- Findings: Rejection witnesses compare the whole memo object with
  `toEqual(priorMemo)` or spy on `Map.prototype.set` and `clear` of the
  supplied map. Successful promotion is observed directly.
- Missing evidence: None for the paths this revision implements.
- Conclusion: resolved as exercised; the witness list is under the next
  question.

### Q: Does the owner promote only in the publication block?

- Sources examined: `rust-mode-transform.ts:1006-1009`, `:1028-1031`,
  `:1164-1182`, `:1447-1466`, `:1476-1478`, `:815-832`, `:1538-1540`; the
  witnesses below.
- Findings: The staging and promotion sites are the ones listed above. A pass
  assigns `state.ordinals` only at `:1478`; `invalidateWireState` resets it
  (`:823`) and `clearSession` drops the state (`:1538`). The between-pages
  witness completes a paged series, is refused at publication, and leaves the
  memo empty. The `module-wire` witnesses cover the supplied map, byte
  boundaries, and restart.
- Missing evidence: None for the paths this revision implements.
- Conclusion (2026-09-13, revision-bound run after merging `origin/main` at
  `5def3c71`, 1107 pass, 0 fail): resolved as exercised. Witnesses and
  markers:
  - `rust-mode-transform.test.ts:2243` "rejects <fault> at the persisted
    ordinal yield with <shared|distinct> arrays" (8 cases). Marker:
    `pageSizes` equals `[MODULE_ORDINAL_PAGE_SIZE]`, `entries.size` 0, and
    `heldBytes > MODULE_ORDINAL_PAGE_SIZE * ORDINAL_ENTRY_RETAINED_BYTES`
    (`:2282`) at the yield; `entries.size` equals `rows.length` only after
    the recovery pass.
  - `:2470` "does not dispatch a need_full_sync retry after <fault> of the
    valid first send" (4 cases). Marker: `priorMemo.entries.size` 1 and a
    pending tail delta; `state.ordinals` equals `priorMemo` (`:2528`) for
    mutation and supersession, and is empty for clear and invalidation.
  - `:2156` "charges every existing ordinal entry and ID before copying a warm
    memo". Marker: decline log `ordinal memo copy`;
    `toEqual(priorMemo)` at `:2196`.
  - `:1084` "discards a partly shifted ordinal memo before host publication
    when shifting throws". Marker: `shiftFailed` true; `toEqual(priorMemo)`
    at `:1125`.
  - `:1144` "rejects ordinal continuation overflow before publication and
    recovers on a valid response". Marker: `entries.get("m-1")` 11 after the
    valid response (`:1164`).
  - `:2683` "rejects publication and promotes no memo when the wire state is
    invalidated mid-flight". Marker: `calls` 1 before `invalidateWireState`.
  - `:2536` "declines before publication and NACKs known deliveries when the
    source changes between pages". Marker: the series completed with
    `transform_page_complete` true; `ordinals.entries.size` 0 after the
    refusal (`:2571`). This witness starts from an empty memo, so it shows
    non-promotion rather than preservation of prior entries.
  - `module-wire.test.ts:892` (27 cases), `:990` (2 cases), `:1085` (5 cases)
    as above.
  - Promotion controls: `rust-mode-transform.test.ts:2901`
    (`entries.get("m-1")` is 1 at `:2944` while the ACK is paused) and
    `:3100` (`entries.size` 2 after a delta publication).
