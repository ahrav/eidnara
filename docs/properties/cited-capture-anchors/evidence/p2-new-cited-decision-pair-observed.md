# p2-new-cited-decision-pair-observed

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed reachability obligation for N2 to N5, activated with partial exercise by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) and completed by the U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136).

## Evidence trail

The U3 tests commit an anchored and cited decision, and decisions for `source_capture_off`, `source_id_unjoinable`, `descriptor_not_found`, and `head_unavailable` (`crates/daemon/src/source_capture_tests.rs:3496`, `:3646`). The U4 tests commit `publish_budget_expired` decisions for a source job past its budget and for a refused handoff (`:4205`, `:4259`).

## Failure scenario

A campaign missing a legal state leaves the safety records that guard it vacuous.

## Timing windows and dependencies

None.

## What a test must construct

The five citation states and both anchor states. U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)) constructs `publish_budget_expired` in `crates/daemon/src/source_capture_tests.rs:4205` and `:4259`.

## Investigation log

### Q: Which state is missing?

- Sources examined: the U3 and U4 tests.
- Findings: none. All five citation states (`cited`, `source_capture_off`, `source_id_unjoinable`, `publish_budget_expired`, `descriptor_not_found`) and both anchor states are committed by tests.
- Conclusion: resolved.
