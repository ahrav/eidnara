# p2-reachable-from-negative-needs-complete-evidence

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S1, touchpoints C3 and OQ2-9 (no capture payload by default, so the fallback rungs have nothing to match).

## Evidence trail

`resolve_commit` (`crates/kernel/src/applicability/resolve.rs:230`) returns `NotReachable` only on `WindowMatch::None` when the anchor was present, the ancestry test decided, and `window_scan_complete` (`:270`) held. An absent anchor, an undecided ancestry test, a truncated window, an ambiguous match, budget exhaustion, or an unreadable object each yields `Uncertain`. `ReachableBetween` resolves its end first and reports an unreachable start as non-historical.

The eleven cited tests construct each undecided case: absent objects (`crates/kernel/tests/kernel_anchor_resolution.rs:283`), a match outside the window (`:325`), unreadable captures (`:507`), an exhausted budget (`:601`), unreadable blobs (`:990`, `:1130`), shallow boundaries (`:1254`, `:1289`, `:1345`), and patch-id shapes (`:1406`, `:1455`).

## Failure scenario

A partial clone missing the anchored commit would report `DoesNotHold` and label a current memory `Historical`, dropping it from injection.

## Timing windows and dependencies

Budget exhaustion can happen mid-walk; the ladder records it and answers `Uncertain`.

## What a test must construct

Each undecided state separately: an absent anchor commit, a window truncated at `CANDIDATE_WINDOW`, an unreadable blob, a shallow boundary, and an exhausted `EvalBudget`, asserting `Uncertain` in every case and `DoesNotHold` only for a present, unreachable anchor with a complete scan.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: the eleven `crates/kernel/tests/kernel_anchor_resolution.rs` tests named in the record, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
