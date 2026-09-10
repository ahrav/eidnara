# history-outer-retry-pressure-is-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
This working-tree evidence follows the [catalog scope][scope], not an exercise.

## Discovery trigger

The independent review identifies outer retry pressure as separate from inner
demotion. A body that fits can still exceed the wrapped-slice retry threshold.

## Evidence trail

- [m0_compose.rs:178-215][retry] extracts the session-history slice, checks
  `cost > budget * 1.05`, and retries at most three times after the initial render.
- [memory_render.rs:191-200][wrapper] tightens the inner budget by pressure but
  retains a wrapper even for empty history, using [M0_EMPTY_BODY][empty].
- [decay_render.rs:301-303][inner] returns empty for no compartments. Thus the
  nonempty wrapper does not disappear when the inner body is empty.
- [tokenizer/lib.rs:123-149][tokenizer] counts token IDs for the rendered text;
  the nonempty wrapper has positive integral cost, unlike empty inner text.

## Failure scenario

A refactor validates only the inner body's fit and never tests the wrapper.
It then changes the outer threshold, retries indefinitely, or truncates output
to invent a strict whole-prompt budget. Inner pressure coverage cannot catch this.

## Timing windows and dependencies

Let `B` be finite and positive and `T0` be the actual cost of the initial
baseline wrapped slice. The marker requires `T0 > 1.05 * B`, independently of
the candidate. Its success does not require over-budget return or three attempts.

## What a test must construct

Use the fixed baseline and actual tokenizer to record initial slice bytes and
cost before candidate execution. Cover this boundary matrix separately:

| Case | Reference observation to preserve |
| --- | --- |
| The budget is nonpositive. | The outer guard performs no retries. |
| Cost equals the computed 105% threshold. | The strict comparison performs no retry. |
| Initial cost exceeds the threshold. | The reference enters a rerender; H4 fires. |
| Empty history uses budget 0.5. | The wrapper remains nonempty; all three retries exhaust and the last render returns. |

The exhaustion constructor follows from the source: each render has the same
wrapper with cost at least one, exceeding 0.525. No actual count or execution
is claimed. Candidate internal work may differ if H2's observable result and
bounded retry contract remain intact. All existing checks are unaudited.

## Investigation log

### Q: Is outer exhaustion constructible without requiring a broken renderer?

- Sources examined: [The retry loop][retry], [empty wrapper][empty], and
  [wrapper construction][wrapper].
- Findings: Empty compartments and a small positive budget preserve the wrapper
  through every attempt, so source semantics permit three-attempt exhaustion.
- Missing evidence: No boundary matrix or independent H4 campaign runs here.
- Conclusion: The constructor is resolved from source. Execution remains
  pending; H4 witnesses initial pressure, not an erroneous return condition.

[scope]: ../catalog.md#scope-and-provenance
[retry]: ../../../../crates/daemon/src/m0_compose.rs#L178-L215
[wrapper]: ../../../../crates/daemon/src/memory_render.rs#L191-L200
[empty]: ../../../../crates/daemon/src/memory_render.rs#L9
[inner]: ../../../../crates/daemon/src/decay_render.rs#L301-L303
[tokenizer]: ../../../../crates/tokenizer/src/lib.rs#L123-L149
