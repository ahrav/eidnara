# p6-plugin-source-kind-table-is-closed-and-classes-never-raised

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S6. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`derive_classes` (`crates/daemon/src/kernel_routes/commit.rs:370`) matches the four kinds and returns `None` otherwise; the caller maps `None` to `InvalidInput`. `a_plugin_write_is_capped_at_inference_classes` (`:1249`) and `a_plugin_route_cannot_declare_a_class_above_the_derived_one` (`crates/daemon/tests/kernel_routes.rs:1145`) cover the cap.

## Failure scenario

A plugin commits under `verifier` and its rows are counted as attestations.

## Timing windows and dependencies

None.

## What a test must construct

A plugin write under each kind outside the table.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
