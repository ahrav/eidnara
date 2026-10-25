# p2-undecodable-or-oversized-anchor-row-is-uncertain

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for surface S1, touchpoints C4 and D1. The anchor writer added by #1133 reuses this record's two checks, so its behavior fixes what a stored row must satisfy.

## Evidence trail

`crates/kernel/src/anchor.rs:273` (`AnchorCondition::decode`) maps the stored kind through `AnchorKind::from_stored` and returns `UnknownKind` for anything else. It then calls `unowned_columns` (`:246`), which compares the populated columns against an owned-column table per kind and returns `ConflictingColumns` on any populated unowned column. Git kinds require a nonempty value that `is_commit_oid` accepts (40 or 64 lowercase hex characters, `crates/kernel/src/scope.rs:710`). Platform ranges must parse as `semver::VersionReq`; wall-clock intervals must satisfy `start_ms < end_ms`. `decode_captures` (`crates/kernel/src/anchor.rs:201`) ignores an absent or unparseable payload but refuses two captures of one commit.

In the engine, `crates/kernel/src/applicability/engine.rs:537` checks `anchor_row_exceeds_bounds` (`:1277`) before any decode and classifies the candidate `Uncertain` with an uncacheable reason. Decode failures reach the same state through the engine's anchor evaluation path.

A raw-SQL anchor fixture needs an `object_registry` row of kind `anchor` and a `commit_log` row for its `created_commit_seq` (refinements R2 and R3); the engine tests build `AnchorRowSpec` values directly and never store them.

## Failure scenario

A row stored with `anchor_kind = "reachable_from"` and a populated `exact_value` decodes as `ReachableFrom` and ignores the exact value. If the decoder accepted it, a memory meant to hold only for one deployment would evaluate against git ancestry alone.

## Timing windows and dependencies

None. Decode is a pure function of the row, and the bound check reads only the row.

## What a test must construct

One `AnchorRowSpec` per refused shape: unknown kind, missing value, uppercase oid, unowned column, a payload just above `MAX_OBJECT_PAYLOAD_BYTES`, and duplicate captures. Place each beside a well-formed candidate in one batch and assert the well-formed verdict is unchanged.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_anchor.rs:252`, `:289`, `:304`; `crates/kernel/tests/kernel_applicability_engine.rs:241`, `:2108`, `:4143`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
