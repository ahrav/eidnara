# apply-adapter-validates-entire-assembled-invocation

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/edit_receipts.rs` capacity check before minting.
- `packages/opencode-plugin/src/hooks/context/invocation-budget.ts`
  `chargeInvocation` sums the candidate entries' canonical JSON lengths, the
  same per-entry lengths the recipe application already measured, divides by
  the estimator's heuristic ratio, charges `headroomPermille`, and labels the
  result with `harnessProfile()`: identity `opencode-heuristic`, revision
  `generation:<estimator generation>`, authority `heuristic`.
  `validateInvocation` admits a charge within the limit, admits a candidate no
  larger in bytes than the incoming surface, admits everything when the limit
  is unknown, and refuses the rest.
- `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts` calls it
  on `application.lengths` against `inputLengths` after every other
  publication guard and before `replaceHostArrayContents`, with the reported
  context limit as the bound; a refusal is a `PassDeclined` with reason
  `invocation_budget`, the pass-through disabled path.
- `packages/opencode-plugin/src/hooks/context/invocation-budget.test.ts`
  checks the charge arithmetic, the limit and limit-minus-one boundary, the
  shrinking and unknown-limit admissions, and that the revision moves with the
  estimator generation; `rust-mode-transform.test.ts` drives the three
  outcomes through the transform with the host array observed.

## Failure scenario

An edit that fits its own bound overflows the invocation; or a validation that
estimates only the packed entry admits a surface whose other entries already
fill the window.

## Timing windows and dependencies

None: the invocation bound is checked on one assembled request.

## What a test must construct

- A candidate surface larger than the incoming one and a usage sample whose
  derived limit is one below the charged total.
- A candidate no larger than the incoming surface under a limit of one token.
- An estimator swap through `installTokenizerForTest`.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
