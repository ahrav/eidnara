# ordinal-memo-promotion-is-owned

## Discovery trigger

Parent #525 requires pass-local ordinal work and forbids an obsolete resolver
from promoting shared memo state.

## Evidence trail

Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`. The
session memo field is `state.ordinals` (`rust-mode-transform.ts:155`).

- [rust-mode-transform.ts](../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts)
  charges a copy of every existing entry and ID before the first await
  (`:1003-1006`), builds `stagedMemo` as that copy (`:1124-1127`), passes it
  to `primeOrdinalMemo` and `annotateOrdinals` (`:1159-1177`), shifts it for a
  continuation base (`:1452-1464`), and assigns `state.ordinals = stagedMemo`
  only after `replaceHostArrayContents` (`:1475-1477`).
- `invalidateWireState` (`:813-830`) and `clearSession` (`:1533`) reset or
  drop the memo by their own contract and request lease cancellation, so a
  pass in flight declines at its next `assertCurrentPass` (`:955-960`).
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
not publish staged work.

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
- Conclusion (2026-09-13, revision-bound run, 1098 pass, 0 fail): resolved
  as exercised. Witnesses and markers:
  - `rust-mode-transform.test.ts:2255` "rejects <fault> at the persisted
    ordinal yield with <shared|distinct> arrays" (8 cases). Marker:
    `pageSizes` equals `[MODULE_ORDINAL_PAGE_SIZE]`, `entries.size` 0, and
    `heldBytes > MODULE_ORDINAL_PAGE_SIZE * ORDINAL_ENTRY_RETAINED_BYTES`
    at the yield; `entries.size` stays 0 after settlement, then one recovery
    pass primes `rows.length` entries.
  - `:2482` "does not dispatch a need_full_sync retry after <fault> of the
    valid first send" (4 cases). Marker: `priorMemo.entries.size` 1 and a
    pending tail delta; `state.ordinals` equals `priorMemo` for mutation and
    supersession, and is empty for clear and invalidation.
  - `:2168` "charges every existing ordinal entry and ID before copying a warm
    memo". Marker: decline log `ordinal memo copy`, `bodies` stays 1, and
    `expect(transform.getState(sessionId).ordinals).toEqual(priorMemo)`.
  - `:1052` "discards a partly shifted ordinal memo before host publication
    when shifting throws". Marker: `shiftFailed` true; memo equals
    `priorMemo`; NACK of `shift-1`; the next pass publishes shifted entries.
  - `:1112` "rejects ordinal continuation overflow before publication and
    recovers on a valid response". Marker: `entries.size` 0 and NACK after
    the overflow; `entries.get("m-1")` 11 after the valid response.
  - `:2689` "rejects publication and promotes no memo when the wire state is
    invalidated mid-flight". Marker: `calls` 1 before `invalidateWireState`.
  - `module-wire.test.ts:892` "keeps the supplied map untouched on <fault>
    during <mode>" (27 cases). Marker: `expect(set).not.toHaveBeenCalled()`,
    `expect(clear).not.toHaveBeenCalled()`, and `pageSizes` matching the
    fault's expected scan progress.
  - `:990` "charges row and memo storage plus <short|long> IDs at exact byte
    boundaries" (2 cases). Marker: refused budgets leave `entries` equal to
    `original`; the exact budget promotes.
  - `:1085` "preserves the memo through a full restart ending in <outcome>"
    (5 cases). Marker: the provider asserts `entries` equals `original` on
    every page and count read.
  - Promotion controls: `rust-mode-transform.test.ts:2907`
    (`entries.get("m-1")` is 1 while the ACK is paused) and `:3106`
    (`entries.size` 2 after a delta publication).
