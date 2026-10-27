# cc-flag-off-no-pass-no-write-no-table-read

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

The feature ships default-off (KTD4), so the default build must run no
candidate table statement on any path.

## Evidence trail

- `crates/daemon/src/config.rs:1024` the key is user-only and default false.
- `crates/daemon/src/memory_reviewer/lifecycle.rs:448` the sweep reads the
  daemon-level value on each cleanup pass.
- The pass is offered only when the route's frozen value consents; the
  capture job's offer carries it.
- `crates/daemon/src/contradiction_candidates/read.rs:132` the read answers
  `disabled` before reaching the store.

## Failure scenario

A flag-off deployment writes or reads candidate rows: an unmeasured matcher
reaches users and a write load appears unasked.

## Timing windows and dependencies

A configuration reload after a route bound leaves that route's value; the
sweep follows the reload on its next pass.

## What a test must construct

The candidate table renamed away under each arm, a flag-off run with every
counter at zero, and a flag-on control that fails on the hidden table:
`crates/daemon/tests/contradiction_candidates_sweep.rs:552` (sweep),
`crates/daemon/src/candidate_pass_tests.rs:1767` (pass), and
`crates/daemon/src/candidate_pass_tests.rs:1544` (read).

## Investigation log

### Q: Does a flag-off pass or read touch the table?

- Sources examined: the three tests above.
- Findings: each flag-off run succeeds with the table absent, and each
  flag-on control reports `store_unavailable`.
- Missing evidence: none.
- Conclusion: resolved with answer.
