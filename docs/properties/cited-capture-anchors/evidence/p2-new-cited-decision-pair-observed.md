# p2-new-cited-decision-pair-observed

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed reachability obligation for N2 to N5, activated with partial exercise by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

The U3 tests commit an anchored and cited decision, and decisions for `source_capture_off`, `source_id_unjoinable`, `descriptor_not_found`, and `head_unavailable` (`crates/daemon/src/source_capture_tests.rs:3474`, `:3624`).

## Failure scenario

A campaign missing a legal state leaves the safety records that guard it vacuous.

## Timing windows and dependencies

None.

## What a test must construct

The five citation states and both anchor states; the `publish_budget_expired` state needs the U4 settlement record.

## Investigation log

### Q: Which state is missing?

- Sources examined: the U3 tests.
- Findings: `publish_budget_expired`.
- Missing evidence: the U4 settlement record.
- Conclusion: unresolved, needs #1136.
