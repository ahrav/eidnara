# uo-memory-disabled-route-takes-no-canonical-read

## Discovery trigger

Spec #1145 C2.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `f4dd2b618`).

- Gate: `project_memory_read` (`crates/daemon/src/lib.rs:5509`); the pass calls
  it with the route-frozen config.
- With `None`, `project_memory_composition()` is `None`, so meta and the
  response carry no composition.
- Correction: the spec cited `canonical_memory.rs:457`, which is
  `withheld_reads_record_the_verdict_and_inject_nothing`, a Withheld-read test.

- Guarantee under test: A route bound with `memory.enabled` off takes no Kernel
  read, so the pass has no composition.
- Existing check: `a_memory_disabled_pass_takes_no_canonical_read`
  (`crates/daemon/tests/transform_canonical_memory.rs:320`). Status: unaudited.

## Failure scenario

A writer keyed off request fields would record a serve on a disabled route.

## Timing windows and dependencies

None.

## What a test must construct

The existing test; #1155 adds the no-row assertion.

Required faults and enabling state: A route bound with `/memory/enabled: false`.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
