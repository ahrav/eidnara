# apply-adapter-validates-entire-assembled-invocation

## Discovery trigger

The RP2.7 adapter capability gate requires a host adapter to validate the
whole invocation, not only the packed payload. The Rust-core cleanup deleted
the dormant Pi adapter, so its former test cannot remain evidence.

Repository: `/local/home/ahrav/scratch/eidnara-stack`; inspected 2026-09-19 at
`d34ff88300ec947c5d3ed0ed5162422993a98d34` before this documentation change.

## Evidence trail

- `packages/opencode-plugin/src/hooks/context/invocation-budget.ts:1-69`
  defines the host byte-budget projection. `chargeInvocation` sums UTF-8 byte
  lengths, divides by 3.5 with upward rounding, adds permille headroom with
  upward rounding, and labels the charge `heuristic` with fixed revision
  `utf8-bytes-div-3.5-v1`. Its comment states that this projection never
  substitutes for native token counting.
- `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1603-1619`
  validates `application.lengths` against all `inputLengths` before
  `replaceHostArrayContents` publishes the candidate.
- `packages/opencode-plugin/src/hooks/context/invocation-budget.test.ts:25-54`
  checks admission at the limit, refusal one below it, shrinking admission,
  and unknown-limit admission.
- `packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts:552-681`
  drives fitting, over-limit, shrinking, and untrusted-limit outcomes through
  the OpenCode publication path.
- `crates/daemon/tests/edit_receipts.rs:827` checks append allowance and
  replacement capacity before preparation. This is daemon boundary evidence,
  not proof that a host adapter validates its assembled invocation.
- Retired witness: `packages/pi-plugin/src/context-application-pi.ts` and
  `packages/pi-plugin/src/context-application-pi.test.ts` charged a system
  prompt slot under `pi-heuristic`. Both files are deleted on current HEAD.
  No current Pi production adapter sees or validates the assembled invocation.

## Failure scenario

An edit that fits its payload bound overflows the complete invocation. A host
adapter that invokes native token counting as fallback for this admission
projection also changes the separate byte-budget contract and can fail closed
for the wrong reason when the addon is unavailable.

## Timing windows and dependencies

None. The OpenCode check runs synchronously on one assembled candidate before
publication. Pi has no application point at which to run this check.

## What a test must construct

- An OpenCode candidate larger than the incoming surface with a trusted limit
  one below its charged total.
- A candidate no larger than the incoming surface under a one-token limit.
- A model with no trusted reported limit.
- For future Pi work, a reachable adapter that sees messages, tool schemas,
  and system prompt as one assembled invocation, plus a limit-minus-one case.

## Investigation log

### Q: Does current OpenCode source validate the whole candidate before edit?

- Sources examined: `invocation-budget.ts`, `rust-mode-transform.ts`, and their
  focused tests at the lines above.
- Findings: yes. Every candidate entry contributes its UTF-8 byte length, and
  the validation precedes in-place replacement.
- Missing evidence: none for the OpenCode byte-budget path.
- Conclusion: resolved with answer - OpenCode remains default-production and
  exercised.

### Q: Does Rust or current Pi source prove the Pi host-adapter clause?

- Sources examined: deleted-path checks for `context-application-pi.ts` and
  `context-application-pi.test.ts`, current Pi build entries, and
  `crates/daemon/tests/edit_receipts.rs:827`.
- Findings: no. Rust checks daemon capacity. It cannot prove validation by an
  absent Pi host adapter. Current Pi still uses native token counts for live
  bookkeeping, but that is not assembled-invocation application admission.
- Missing evidence: a reachable Pi application adapter and real Pi harness
  witness.
- Conclusion: unresolved, needs future Pi transform and packing work.
