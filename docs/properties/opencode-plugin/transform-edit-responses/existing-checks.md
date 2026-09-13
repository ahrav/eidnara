# Existing checks: client execution (U3)

Status here is per METHOD: `unaudited` means the check exists and ran green
on the revision-bound run (the #533 change on `fix/client-transform-owner`,
parent `b0023512`, 2026-09-13, 1098 pass, 0 fail, 34 files) but has not had an independent
adequacy review. Adequacy belongs to `/testing:invariant-test-review`;
production guard adequacy belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`. The catalog's
`Existing check` fields name the individual `it(` titles, lines, and markers;
this table groups them.

Paths below are relative to `packages/opencode-plugin/src/`. This inventory
covers the client supplement only, not the unavailable 30-record companion.

| Check | Location | Scope | Status |
| --- | --- | --- | --- |
| Bounded transform ownership | `hooks/context/rust-mode-transform.test.ts:1812-3157`, 27 `it(` calls expanding to 53 executed cases | TE17, TE18, TE20, TE22, TE23, TE24, TE26, TE27 windows: preflight, scan yield, transport, retry, pre-apply, paused ACK and NACK, count and byte pressure, slow cancellation | unaudited |
| In-flight supersession, clear, invalidation | Same file, `:826-986` | Cache discard and NACK on mid-flight faults | unaudited |
| Ordinal continuation | Same file, `:988-1134` | Memo preservation on shift failure and overflow | unaudited |
| Native output delta ACK/NACK | Same file, `:1209-1586` | Per-identity dispositions, sequential ACK, retry union | unaudited |
| Transport restart | Same file, `:554-681` | Attempt mismatch, reconnect, full-sync paging (TE26 positive controls) | unaudited |
| Delta prefix-mutation guard | Same file, `:1589-1810` | Cross-pass prefix reuse after edits | unaudited |
| Referenceable JSON domain guard | `hooks/context/transform-capture.test.ts:74-378` | Trap counters at zero for every rejection class | unaudited |
| Capture snapshots and rechecks | Same file, `:380-566` | Tape bounds, root bookkeeping, membership and content edits | unaudited |
| Host array replacement contract | Same file, `:568-675` | Container rejection reasons, in-place replacement | unaudited |
| Capture admission | Same file, `:677-864` | Model-based lease sequences, per-session lease, count and byte limits, exact-once release | unaudited |
| Bounded capture sizing | Same file, `:866-998` | Reservation before capture, exact byte charge, traversal bounds | unaudited |
| primeOrdinalMemo bounded staging | `hooks/context/module-wire.test.ts:879-1171` | Supplied map untouched on every fault; byte boundaries; restart | unaudited |
| Hook and wrapper entries | `hooks/context/hook.test.ts:169-389` | Unsupported sources at the real entries, identity, directory-await mutation and admission | unaudited |
| Wrapper contract | `plugin/messages-transform.test.ts:60-312` | `then` and proxy refusal, return container, no rollback after a throwing hook | unaudited |
| Production guards | `hooks/context/transform-capture.ts`, `hooks/context/rust-mode-transform.ts` | Domain, budget, ownership, recheck, and replacement guards | unaudited |

## Gaps and quiet areas

- TE21 live previous-output validation: none found. The kept-entry accessor
  test refuses hooks; no test mutates a kept entry's data values before reuse.
  That validation is #538.
- TE25 optional-output byte budget and byte-triggered LRU eviction: none
  found. `wireCaches` is a 64-session count-bounded map. That budget is #538.
- TE30 delta-versus-full control: none found. Two-pass tests dispatch a
  delta after changed output but do not compare with a full-request control.
- Permission-verdict await: the `recheckCapture("permission")` boundary has no
  dedicated pause-and-mutate witness; it shares the mechanism the other
  windows exercise.
- Real transport abort and post-write response loss: no witness. In-process
  fakes honor or ignore the abort signal and throw before returning.
- Package `typecheck`: `tsc --noEmit` over `src/` passes on this revision;
  the script config fails at `scripts/bench-transform-client.ts:206`. That
  script is outside these records.
