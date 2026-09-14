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

- Revision: the #533 change on `fix/client-transform-owner` at `d5a525e8`,
  after merging `origin/main` at `5def3c71`. The merge brought in #565's follow-up commits
  (built-in prototype accessor scan, boxed-primitive rejection, null-prototype
  tape marker, `Object.hasOwn` descriptor checks, warn-level logging for
  polluted prototypes). `client.ts` is unchanged from `origin/main`.
- Command, from `packages/opencode-plugin` with Node 24.18.0 first on PATH:
  `bun test src/hooks/context/ src/plugin/messages-transform.test.ts
  src/shared/host-client/client.test.ts`.
- Result: 1122 pass, 0 fail, 31,659 `expect()` calls, 34 files, Bun 1.3.14,
  2026-09-13. The "bounded transform ownership" block alone runs 60 cases.
- Nine records are `Exercised: yes` (TE17, TE18, TE19, TE20, TE22, TE23,
  TE24, TE26, TE27). Two remain `partial` (TE21, TE30) because their missing
  oracles are #538 work, not because a witness failed.

## Dispositions

- Every `Existing check` names individual `it(` titles with file and line
  in this tree and the marker assertion that proves the enabling state was
  reached before the fault.
- Recheck boundaries are stated per site. `recheckCapture` runs after each
  ordinal prime, before the wire is built, before a series restart, before a
  full-sync retry rebuilds or re-serializes its body, and at publication. The directory await, the permission await, and each
  transport page are `assertCurrentPass()` fences only. A content change
  between pages is refused at publication and every reported delivery is
  NACKed; the series is not stopped at the next page. The witness is
  "declines before publication and NACKs known deliveries when the source
  changes between pages".
- The candidate is not inspected before `assertNativeBoundary`. TE19, TE20,
  and TE21 do not cite a kept-entry accessor witness. TE21 stays partial and
  states that kept-prefix validation is #538.
- `hostArrayReplacementRejection` takes no length and applies no cap. TE23
  records the slot charge in `buildNativeCandidate` as the only candidate
  bound. TE20 cites "refuses a numeric accessor on a built-in prototype and
  defines slots without invoking it" for the inherited-accessor case.
- `state.forceFullWire` is set before the first daemon send. TE17, TE27, and
  TE30 cite "forces a full send after a dispatched delta pass is
  source-declined" and the retry-fault loop's `forceFullWire === (fault !==
  "clear")` assertion.
- TE19 gains the built-in prototype scan witnesses and the wrapper's
  warn-level polluted-prototype witness.
- TE25 stays outside this supplement. Successful publication transfers the
  candidate array and `captured.snapshots` to the 64-session `wireCaches`
  owner, which is count-bounded, and the promoted memo to `state.ordinals`
  in `states`, which is an unbounded `Map` that only `clearSession` deletes
  from. The separate 64 MiB optional-output byte budget with byte-triggered
  LRU eviction is #538; the `states` bound is a gap recorded in the catalog's
  open questions, not covered by that budget.
- TE30 stays partial. Two-pass witnesses dispatch a delta after changed
  output; the forced full resend shows realignment after a source-declined
  dispatch; no delta-versus-full control exists.
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
4. Add a clear, invalidation, or supersession witness landed during the
   directory or permission await.
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
  here. The package `typecheck` script exits 0 on this revision.

## Structural check

The supplement has 11 records, 11 matching index rows, and 11 evidence files.
Record fields follow METHOD's order, evidence files have its required
sections and investigation fields, and local links resolve. Check semantics
are 10 `always` and one `sometimes`; exercise statuses are 9 `yes` and 2
`partial`.
