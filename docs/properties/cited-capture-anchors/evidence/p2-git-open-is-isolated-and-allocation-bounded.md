# p2-git-open-is-isolated-and-allocation-bounded

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S5, touchpoints C5 and AC7. U2 routes the capture HEAD read through this open.

## Evidence trail

`isolated_options` (`crates/daemon/src/git_sources.rs:128`) starts from `gix::open::Options::isolated()` and overrides `gitoxide.objects.allocLimit` with the bound after the repository's own configuration. `open` (`:138`) maps an `extensions.objectFormat` refusal to `UnsupportedObjectFormat` and sets `objects.ignore_replacements`. At the U1 branch its only caller was `GitReconciler` (`crates/daemon/src/git_reconcile.rs:244`), which only tests construct. U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)) adds `observe_head` (`crates/daemon/src/memory_capture/head.rs:20`), which every native capture submit reaches with the shipped configuration, so the record is relabelled `default-production` per METHOD rule 4.

## Failure scenario

A repository configuration that raised the allocation limit would let one packed delta allocate past the bound.

## Timing windows and dependencies

None.

## What a test must construct

A delta-packed commit against an oversized base; a repository declaring an unsupported object format; a `refs/replace` entry.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/tests/git_sources.rs:549`, `:827`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which reachability label holds after U2?

- Sources examined: callers of `git_sources::open` at the U2 branch.
- Findings: `observe_head` calls it from `accept_native_output` on every
  submit whose output parses; memory capture is on by default.
- Missing evidence: none.
- Conclusion: resolved with answer: `default-production`.
