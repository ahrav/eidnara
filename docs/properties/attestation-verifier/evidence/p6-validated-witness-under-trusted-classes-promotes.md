# p6-validated-witness-under-trusted-classes-promotes

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S1. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`observe_code` (`crates/daemon/src/lib.rs:14394`) writes a `code_present` observation and a `CodeObserved` admission under trusted classes; it is compiled only under the feature gate at `:14041`. The route tests `reads_at_a_snapshot_return_what_was_visible_then` (`crates/daemon/tests/kernel_routes.rs:1932`) and `a_disposition_preview_reports_visibility_per_surface_and_writes_nothing` (`:5177`) insert an observation from `code_observation` (`:887`). The specification cited lines inside those tests that call the sibling fixture; at the #1172 branch the calls are `crates/daemon/tests/kernel_routes.rs:1945` and `:5194`.

## Failure scenario

A campaign that never constructs a promotion leaves every guard on promotion untested.

## Timing windows and dependencies

None.

## What a test must construct

One trusted-class subject promoted through a validated trigger.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
