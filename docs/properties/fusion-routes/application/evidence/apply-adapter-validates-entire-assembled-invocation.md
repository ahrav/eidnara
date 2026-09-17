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
  same per-entry lengths the recipe application already measured, charges them
  through `estimateTokensFromLength` from `shared/token-estimator.ts` (the one
  `HEURISTIC_CHARS_PER_TOKEN` the estimator's own fallback divides by), adds
  `headroomPermille`, and labels the result with `harnessProfile(identity)`:
  the budget's identity (`opencode-heuristic` or `pi-heuristic`), revision
  `generation:<estimator generation>`, authority `heuristic`.
  `validateInvocation` admits a charge within the limit, admits a candidate no
  larger in bytes than the incoming surface, admits everything when the limit
  is unknown, and refuses the rest.
- `packages/pi-plugin/src/context-application-pi.ts` `validatePiInvocation`
  charges the whole candidate system prompt as one entry by its UTF-8 byte
  length against the incoming prompt under `pi-heuristic` with
  `PI_INVOCATION_HEADROOM_PERMILLE`; `editSystemPrompt` calls it on every
  candidate and confirms `keep` with the prompt unchanged when it refuses. The
  limit is `promptTokenBudget`, the capacity the caller leaves for the prompt
  after charging the rest of the invocation against Pi's usable window; the
  adapter sees only the prompt, and no production caller supplies the budget
  yet.
- `packages/pi-plugin/src/context-application-pi.test.ts` checks the one-entry
  charge and profile, the byte-length charge on a non-ASCII prompt, the
  limit and limit-minus-one boundary through `editSystemPrompt`, and the
  revision moving with the estimator generation.
- `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts` calls it
  on `application.lengths` against `inputLengths` after every other
  publication guard and before `replaceHostArrayContents`. The bound is
  `resolveTrustedContextLimit` alone: the usage sample's percentage is not fed
  back as a limit, because `event-handler.ts` and `plugin/rpc-handlers.ts`
  compute it against `resolveContextLimit`, which substitutes the 128k default
  for a model models.dev cannot name, so inverting it would enforce that
  default as if the host had reported it. The inversion still sizes
  `contextLimit` for thresholds. A refusal is a `PassDeclined` with reason
  `invocation_budget`, the pass-through disabled path.
- `packages/opencode-plugin/src/hooks/context/invocation-budget.test.ts`
  checks the charge arithmetic, the limit and limit-minus-one boundary, the
  shrinking and unknown-limit admissions, that the revision moves with the
  estimator generation, and that the charge equals `estimateTokens` under the
  forced heuristic; `rust-mode-transform.test.ts` drives the three outcomes
  through the transform with the host array observed under a spied
  `resolveTrustedContextLimit` whose usage sample inverts to the opposite
  verdict, and publishes a growing candidate charged over 128k tokens for an
  unknown model whose usage sample was computed against the default.

## Failure scenario

An edit that fits its own bound overflows the invocation; or a validation that
estimates only the packed entry admits a surface whose other entries already
fill the window; or a gate that inverts the usage percentage declines every
growing candidate over 128k charged tokens for a model models.dev cannot name,
silently disabling the transform on large-context or unlisted models.

## Timing windows and dependencies

None: the invocation bound is checked on one assembled request.

## What a test must construct

- A candidate surface larger than the incoming one under a models.dev limit
  one below the charged total, with a usage sample inverting to the opposite
  verdict.
- A candidate no larger than the incoming surface under a limit of one token.
- On Pi, a system prompt whose appended block charges one over the usable
  window, and a non-ASCII prompt whose code-unit charge fits a limit its byte
  charge exceeds.
- A model models.dev cannot name, a usage sample computed against the 128k
  default, and a growing candidate charged over 128k tokens.
- An estimator swap through `installTokenizerForTest`.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: the harness-side applied-identity witnesses (RP2.8.U5).
- Conclusion: resolved for the gate; enablement witnesses remain open.
