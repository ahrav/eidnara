# Portfolio evaluation: client execution (U3)

## Status

This file records the documentation author's disposition after the
revision-bound run, not a fresh-context evaluation. METHOD's independent
portfolio evaluation through harness fit, coverage balance, implementability,
and wildcard lenses has not run for this supplement. No test-adequacy verdict
is issued; that belongs to `/testing:invariant-test-review`.

The parent #525 names a reusable 30-record external companion. It is
unavailable here. This 11-record client supplement neither replaces it nor
inherits its evaluation. Missing companion records are not reconstructed.

## Executed state

- Revision: the #533 change on `fix/client-transform-owner`, parent `b0023512`.
- Command, from `packages/opencode-plugin` with Node 24.18.0 first on PATH:
  `bun test src/hooks/context/ src/plugin/messages-transform.test.ts
  src/shared/host-client/client.test.ts`.
- Result: 1098 pass, 0 fail, 31,141 `expect()` calls, 34 files, Bun 1.3.14,
  2026-09-13. The "bounded transform ownership" block alone runs 53 cases.
- Nine records are `Exercised: yes` (TE17, TE18, TE19, TE20, TE22, TE23,
  TE24, TE26, TE27). Two remain `partial` (TE21, TE30) because their missing
  oracles are #538 work, not because a witness failed.

## Dispositions

- Every `Existing check` names individual `it(` titles with file and line
  in this tree and the marker assertion that proves the enabling state was
  reached before the fault. Describe-block citations were replaced.
- Source references carry line numbers verified against this tree. The memo
  field is `state.ordinals`; an earlier draft named a field that does not
  exist.
- TE21 stays partial. The kept-entry accessor refusal is exercised; value
  validation of kept entries before advertisement or reuse is #538.
- TE25 stays outside this supplement. Successful publication transfers the
  candidate array, `captured.snapshots`, and the promoted memo to the
  64-session `wireCaches` owner and `states`; that retention is count-bounded.
  The separate 64 MiB optional-output byte budget with byte-triggered LRU
  eviction is #538. This is the current boundary, stated as such.
- TE30 stays partial. Two-pass witnesses dispatch a delta after changed
  output; no delta-versus-full control exists.
- Reachability labels remain `explicit-config-only` for every record; the
  Rust-mode hook and `resolveTransformMode` gate activation.
- The implementer's manual negative control for the model-based admission
  test (removing `owner.leases.delete` made it fail) is recorded as reported,
  not re-run for this document.

## Pending gates

1. Run METHOD's fresh-context portfolio evaluation and record its findings as
   gaps, refinements, or biases.
2. Audit witness oracles independently. A green run and exact markers are
   necessary evidence, not an adequacy verdict.
3. Close TE21 and TE30 through #538; assess TE25 in the companion.
4. Add a pause-and-mutate witness at the permission-verdict await.
5. Recover the external companion or its authoritative location and reconcile
   this supplement's slugs with its predicates.

## Biases and scope limits

- In-process fake clients expose selected schedules, not real transport
  abort, post-write response loss, or daemon-side effect accounting.
- Counters and identity assertions show accounting and publication outcomes;
  they do not measure process memory.
- The selected records emphasize client lifecycle and say nothing about the
  other 19 parent properties.
- [baseline.md](baseline.md) is maintained separately and is not verified
  here. The package `typecheck` script fails in its scripts config at
  `scripts/bench-transform-client.ts:206` on this revision; `tsc --noEmit`
  over `src/` passes.

## Structural check

The supplement has 11 records, 11 matching index rows, and 11 evidence files.
Record fields follow METHOD's order, evidence files have its required
sections and investigation fields, and local links resolve. Check semantics
are 10 `always` and one `sometimes`; exercise statuses are 9 `yes` and 2
`partial`.
