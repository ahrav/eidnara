# p2-new-head-unavailable-keeps-decision-admitted

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N2 and constraint C5. OQ2-11 (owner
decision 2026-10-25) anchors a detached HEAD at its commit, so the
unavailable path covers unborn, unreadable, unsupported-format, and
non-repository roots. Activated by the U2 ticket
[#1134](https://github.com/ahrav/eidnara/issues/1134) with partial exercise;
the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) adds the
rationale reason, the counter, and the no-anchor clause.

## Evidence trail

`observe_head` (`crates/daemon/src/memory_capture/head.rs:32`) returns
`HeadUnavailable` when `git_sources::open_with_slots` refuses the root (no
repository, an `extensions.objectFormat` gix refuses, an alternates chain
that does not resolve, or a `pack` directory that cannot be listed), when
the object format is not SHA-1, when HEAD's symbolic chain reaches no id
(an unborn branch, directly or through a branch alias, or a cyclic alias
chain), when the HEAD target is not a commit, when neither the loose
object, the repository's pack indices, nor the object store holds it, when
a pack of the repository holds it behind an index paired with another
pack's data or in a delta chain that revisits an entry (the store is then
not consulted), and when its id is not 40 lowercase hex characters
(`git_sources::parse_oid`).
The pack index that held the last packed HEAD orders the next pack search
only when it lies in the observed repository's pack directory, and the
search reads regular `.idx` files only, so another repository's pack never
answers (`:568`). The open performs no upward discovery, as the
Kernel's checkout open does not, so a project root that is a subdirectory of
a work tree also freezes `HeadUnavailable`. The value is
frozen into the plan like a commit id, and nothing else in the submit path
reads it at U2, so admission is unchanged.

## Failure scenario

A head read that returned an error instead of a value would fail every
capture in a fresh repository or a plain directory.

## Timing windows and dependencies

None.

## What a test must construct

Roots for an unborn HEAD, a plain directory, a missing directory, a
repository declaring SHA-256, a HEAD detached at a tree, a HEAD detached at
an absent object, a cyclic alternates chain, a `pack` that is a regular
file, and an alternate whose `pack` is a regular file
(`crates/daemon/src/memory_capture/head.rs:416`); a HEAD naming a commit
that only another repository's pack holds, reached through the hint and
through a symlinked index (`:568`); a HEAD whose branch alias is cyclic or
names an unborn branch (`:373`); and the route-level
submission over unborn, plain, and SHA-256 roots, asserting the frozen value
and that both jobs publish (`crates/daemon/src/lib.rs:24619`).

## Investigation log

### Q: Is a detached HEAD unavailable?

- Sources examined: OQ2-11 in ticket #1134 and `observe_head`.
- Findings: the owner chose to anchor detached HEADs;
  `a_branch_head_and_a_detached_head_freeze_their_commit`
  (`crates/daemon/src/memory_capture/head.rs:351`) freezes the detached
  commit.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should a subdirectory root discover its repository?

- Sources examined: `git_sources::open`, `repository_identity::derive`, and
  the Kernel's checkout open.
- Findings: `repository_identity::derive` searches parents for the plugin's
  repository identity; the Kernel's applicability open, which evaluates
  anchors, opens the given path only. An anchor taken by discovery at a
  subdirectory root would name a commit the engine evaluates against the
  same root and cannot open.
- Missing evidence: an owner decision on subdirectory project roots.
- Conclusion: needs human input; U2 matches the engine's open.

### Q: Are the rationale reason and the no-anchor clause checked?

- Sources examined: the publish closure and tests at the U3 branch.
- Findings: a `HeadUnavailable` plan builds no anchor spec, every decision
  records `anchor: "head_unavailable"` with `anchor_id` empty, and
  `decisions_head_unavailable` counts each; `crates/daemon/src/source_capture_tests.rs:3624` checks a plain and
  an unborn root, and `crates/daemon/src/lib.rs:24729` a plan without
  `head`.
- Missing evidence: none.
- Conclusion: resolved with answer.
