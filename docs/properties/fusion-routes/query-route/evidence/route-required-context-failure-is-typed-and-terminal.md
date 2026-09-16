# route-required-context-failure-is-typed-and-terminal

## Discovery trigger

The RP2.7 specification names `RequiredContextFailure` as a typed terminal
the packing integration raises; the U3b ticket fixes the closed terminal set
(parent Q5).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `Terminal` has six variants,
  `Terminal::code` maps each to one wire code, `terminal_response` emits
  `{"kind":"terminal","terminal":<code>}`; the unit test
  `every_terminal_has_one_wire_code_and_the_response_names_it`.
- `LaneStatus::Unavailable` carries a closed reason code produced by
  `lookup_refusal`, `retrieval_refusal`, `lexical_refusal`, and
  `identity_reason`; engine text is never serialized. `degrades` marks the
  answer. `crates/daemon/tests/query_route.rs`
  `a_lane_that_cannot_run_degrades_the_answer_while_the_other_serves` drives
  the exact lane's `no_checkpoint` through the route.
- No stage constructs `Terminal::RequiredContextFailure` yet.

## Failure scenario

A new failure reaches the harness as free text or as a degraded answer.

## Timing windows and dependencies

None.

## What a test must construct

- The packing stage from U4 that misses required context.

## Investigation log

### Q: Where do blocking failures map?

- Sources examined: `BlockingFailure` in `crates/daemon/src/request_budget.rs`.
- Findings: `Panicked` is a programming fault and is answered as the
  transport's `internal_error`; `RuntimeStopped` and `RouteClosing` mean the
  caller is gone and are answered as `cancelled`.
- Missing evidence: none.
- Conclusion: resolved with answer.
