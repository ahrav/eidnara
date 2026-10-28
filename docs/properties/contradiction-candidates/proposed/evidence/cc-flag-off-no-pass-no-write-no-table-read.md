# cc-flag-off-no-pass-no-write-no-table-read

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

The feature ships default-off (KTD4), so the default build must offer no
candidate pass, write no candidate row, and run no candidate-table read after
the store opens; the Memory Store baseline that `MemoryStore::open` applies
creates the table whatever the flag.

## Evidence trail

- `crates/daemon/src/config.rs:158` the default is false; `:1107` classes the
  key `UserOnly` and `:520-523` pins that class at compile time;
  `contradiction_candidates_default_off_and_only_the_user_tier_enables_them`
  (`:2299`) checks both.
- `crates/daemon/src/memory_reviewer/lifecycle.rs:448` the sweep reads the
  daemon-level value on each cleanup pass.
- The pass is offered only when the route's frozen value consents; the
  capture job's offer carries it.
- `crates/daemon/src/contradiction_candidates/read.rs:188` the read answers
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
`crates/daemon/tests/contradiction_candidates_sweep.rs:553` (sweep),
`crates/daemon/src/candidate_pass_tests.rs:2429` (pass), and
`crates/daemon/src/candidate_pass_tests.rs:2047` (read).

## Investigation log

### Q: Does a flag-off pass or read touch the table?

- Sources examined: the three tests above.
- Findings: each flag-off run succeeds with the table absent, and each
  flag-on control reports `store_unavailable`.
- Missing evidence: none.
- Conclusion: resolved with answer.
