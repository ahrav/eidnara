# fa-e14-native-status-conflates-pruning-with-folding

## Discovery trigger

Existing-behavior record FA-E14 of #834's first comment, at `265df096`.
Surface: plugin. PR #899 (#853) replaces its `auto || prune` clause and its
TUI label (FA-N07, D2c `native_compaction_active` clause); PR #904 (#856)
adds the fold-authority status beside it. Both PR descriptions record the
replacement.

Exercised status: yes - the replacing and surviving clauses are asserted by
the RPC and TUI tests below, green in the `bun run check:repo` gates of #899
through #904, and in #859 PR A's at `d7b330113`. The plugin source at HEAD
is `d7b330113`'s: it carries the review changes merged through `main` after
#904's head, and `d7b330113` itself changes only comments and generated TUI
source.

## Evidence trail

At `265df096`: `nativeActive` was
`args.nativeCompaction.auto || args.nativeCompaction.prune`
(`packages/opencode-plugin/src/plugin/rpc-handlers.ts:764-769` at that tree),
and the TUI suffix returned "no active compaction" only for an explicit
`false` and "native compaction" otherwise, including absence
(`tui/compaction-off.ts:9-12` at that tree).

At HEAD `0ff62b29a`:

- Replaced: `nativeActive: args.nativeCompaction?.auto`
  (`packages/opencode-plugin/src/plugin/rpc-handlers.ts:789-792`). The RPCs
  receive only the host's resolved observation (`index.ts:199`).
- Surviving: the snapshot sets `native_compaction_active` only while Eidnara
  compaction is off and an observation exists; absence omits the field
  (`rpc-handlers.ts:542-545`). `status-detail` builds on the same snapshot
  (`buildStatusDetail`, `:616`, which calls `buildSidebarSnapshot`); the two
  handlers are registered at `:865` and `:882`.
- Replaced: the TUI suffix has three states, "compaction owner unknown" for
  absence, "native compaction" for true, "no active compaction" for false
  (`packages/opencode-plugin/src/tui/compaction-off.ts:9-12`).
- Added by #904: the snapshot's optional `fold_authority`
  (`rpc-handlers.ts:546-548`) and `detail.foldAuthorityLines` (`:673-675`)
  report disk, startup, applied, and pending authority, and a stalled
  summarizer.

Checks at HEAD:

- `the snapshot carries the host's compaction ownership only while Eidnara compaction is off`
  (`packages/opencode-plugin/src/plugin/rpc-handlers.test.ts:582`): both RPCs
  agree; `auto = false, prune = true` gives `false`; `auto = true` gives
  `true`; no observation gives `undefined`; Eidnara folds gives `undefined`.
- `names the owner from compaction.auto alone and marks an absent observation unknown`
  (`packages/opencode-plugin/src/tui/compaction-off.test.ts:75`).
- #904: `a pending authority reaches both RPCs and raises a warn conflict while they keep answering`
  and `a stalled summarizer and an unknown daemon path render without a conflict`
  (`rpc-handlers.test.ts:179`, `:246`).

Citation corrections from `265df096` to HEAD: `rpc-handlers.ts:764-769` is now
`:789-792`; `:528-531` is `:542-545`; the two handlers cited at `:829-864`
are at `:865-908`; `rpc-handlers.test.ts:470-522` is `:582-651`.

## Failure scenario

Before #899, a user with Eidnara compaction off and OpenCode `auto = false`
but `prune = true` saw "native compaction" in the sidebar while no process
folded the session.

## Timing windows and dependencies

None; the observation is resolved at boot and the RPCs read it per poll.

## What a test must construct

Eidnara compaction off with `auto` false and `prune` true, `auto` true, and a
missing observation, through both RPCs and the TUI label; an Eidnara-folds
control where the field is absent.

## Investigation log

### Q: Do the doc comments match the replaced code?

- Sources examined: `rpc-handlers.ts:385-393`; `shared/rpc-types.ts:37-41`.
- Findings: At `286ffaf81`, no for `rpc-handlers.ts`. Its comment on
  `CompactionOwnership` said `nativeActive` is whether `compaction.auto` or
  `compaction.prune` owns the window, while the code reads `auto` alone. The
  comment was unchanged since `265df096` (it was at `:373` there).
  `d7b330113` corrected it: it now says `nativeActive` reads
  `compaction.auto` and that pruning alone does not fold. `rpc-types.ts`
  correctly says `compaction.auto`.
- Missing evidence: None.
- Conclusion: resolved with answer; the comment was corrected at
  `d7b330113` (behavior correct).

### Q: Is the reachability label still explicit-config-only?

- Sources examined: `config/agent-disable.ts:21-29`.
- Findings: No. At HEAD an empty chain makes compaction off, and an empty
  chain is the default, so the field and label render under the default
  configuration. The label at `265df096` assumed compaction off required an
  explicit `false`.
- Missing evidence: None.
- Conclusion: resolved with answer; relabelled default-production.
