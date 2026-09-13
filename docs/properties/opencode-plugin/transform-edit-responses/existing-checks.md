# Existing checks: client execution (U3)

Status here is per METHOD: `unaudited` means the check exists and ran green
on the revision-bound run (the #533 change on `fix/client-transform-owner`
after merging `origin/main` at `5def3c71`, 2026-09-13, 1107 pass, 0 fail, 34
files) but has not had an independent adequacy review. Adequacy belongs to
`/testing:invariant-test-review`; production guard adequacy belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. The catalog's
`Existing check` fields name the individual `it(` titles, lines, and markers;
this table groups them.

Paths below are relative to `packages/opencode-plugin/src/`. This inventory
covers the client supplement only, not the unavailable 30-record companion.

| Check | Location | Scope | Status |
| --- | --- | --- | --- |
| Bounded transform ownership | `hooks/context/rust-mode-transform.test.ts:1844-3151`, 26 `it(` calls expanding to 52 executed cases | TE17, TE18, TE20, TE22, TE23, TE24, TE26, TE27 windows: preflight, scan yield, transport, between pages, retry, pre-apply, paused ACK and NACK, count and byte pressure, slow cancellation | unaudited |
| Forced full resend after a source-declined dispatch | Same file, `:803-833` | `forceFullWire` set before the first send; next pass sends the full history (TE17, TE27, TE30) | unaudited |
| In-flight supersession, clear, invalidation | Same file, `:858-1018` | Cache discard and NACK on mid-flight faults | unaudited |
| Ordinal continuation | Same file, `:1020-1166` | Memo preservation on shift failure and overflow | unaudited |
| Native output delta ACK/NACK | Same file, `:1241-1619` | Per-identity dispositions, sequential ACK, retry union, fingerprint mismatch | unaudited |
| Transport restart | Same file, `:554-681` | Attempt mismatch, reconnect, full-sync paging (TE26 positive controls) | unaudited |
| Delta prefix-mutation guard | Same file, `:1621-1842` | Cross-pass prefix reuse after edits | unaudited |
| Referenceable JSON domain guard | `hooks/context/transform-capture.test.ts:75-379` | Trap counters at zero for every rejection class | unaudited |
| Built-in prototype scan | Same file, `:381-589` | Accessors on `Object`, `Array`, and `String` prototypes, boxed primitives, null-prototype marker, inherited `then`, 12,000-message positive control | unaudited |
| Capture snapshots and rechecks | Same file, `:591-777` | Tape bounds, root bookkeeping, membership and content edits | unaudited |
| Host array replacement contract | Same file, `:779-886` | Container rejection reasons, built-in numeric accessor refusal, in-place replacement | unaudited |
| Capture admission | Same file, `:888-1067` | Model-based lease sequences, per-session lease, count and byte limits, exact-once release | unaudited |
| Bounded capture sizing | Same file, `:1069-1202` | Reservation before capture, exact byte charge, traversal bounds | unaudited |
| primeOrdinalMemo bounded staging | `hooks/context/module-wire.test.ts:879-1171` | Supplied map untouched on every fault; byte boundaries; restart | unaudited |
| Hook and wrapper entries | `hooks/context/hook.test.ts:169-448` | Unsupported sources at the real entries, identity, warn-level byte decline, single capture per pass, directory-await mutation and admission | unaudited |
| Wrapper contract | `plugin/messages-transform.test.ts:60-351` | `then`, proxy, and polluted-prototype refusal with log level, return container, no rollback after a throwing hook | unaudited |
| Production guards | `hooks/context/transform-capture.ts`, `hooks/context/rust-mode-transform.ts` | Domain, built-in prototype, budget, ownership, recheck, and replacement guards | unaudited |

## Gaps and quiet areas

- TE21 live previous-output validation: none found. The daemon's candidate
  is not inspected before `assertNativeBoundary`. No test mutates a kept
  entry's data values or installs a hook on one before reuse. Kept-prefix
  validation is #538.
- TE25 optional-output byte budget and byte-triggered LRU eviction: none
  found. `wireCaches` is a 64-session count-bounded map. That budget is #538.
- TE30 delta-versus-full control: none found. Two-pass tests dispatch a
  delta after changed output but do not compare with a full-request control.
- Directory and permission fences: the directory and permission awaits are
  followed by `assertCurrentPass()` only. Clear, invalidation, and
  supersession are exercised at the page fence and the ordinal yield; no
  witness lands one of them during the directory or permission await.
- Real transport abort and post-write response loss: no witness. In-process
  fakes honor or ignore the abort signal and throw before returning.
- Package `typecheck`: `bun run typecheck` (including `tsconfig.scripts.json`
  and `tsconfig.tui.json`) exits 0 on this revision.
