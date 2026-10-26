# dispute-preview-writes-nothing

## Discovery trigger

Plan P4 rule A9: the command previews before it commits, so the preview
must apply the same evidence checks and write nothing, or the user would
confirm a dispute the commit then refuses, or a preview would record a
dispute the user declined.

## Evidence trail

- The handler composes every reason before it branches on `preview`
  (`crates/daemon/src/kernel_routes/commit.rs:1147-1153`, branch at
  `:1145`), so form refusals are identical by construction.
- The preview runs `PreviewRun::disposition`, which calls the same
  `disposition_request` as the commit (`commit.rs:853`; commit at `:790`),
  so liveness and visibility refusals are identical at the same tip.
- The preview runs in `store.preview` (`commit.rs:895-915`), which writes
  no commit-log row, no receipt, and no admission row; a recorded identity
  replays its receipt instead (`:880-882`).
- A preview refuses tokens and non-disposition operations
  (`commit.rs:1182-1195`).
- The command treats the commit's verdict as the one recorded
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:243-244`)
  and reports a refused preview with no commit (`:223-224`).

## Failure scenario

A preview that skipped `require_visible_evidence` would answer
`available` for a retired id; the user confirms; the commit answers
`not_found`. A preview that wrote would charge the identity and record a
row the user never confirmed.

## Timing windows and dependencies

A commit landing between preview and commit can change the verdict; that
is permitted and the commit wins. The checks in this record compare a
preview and a commit at the same tip.

## What a test must construct

All constructed:

- `assert_refused_without_writing`
  (`crates/daemon/tests/kernel_routes.rs:5703-5715`) previews then commits
  one body under one key, asserts the same refusal name from both, and
  compares `commit_log`, `admission_decisions`, `operation_receipts`, and
  `durable_text_redactions` counts before and after (`write_counts`,
  `:5597-5609`). It runs for unknown, retired, uncited, unscoped, and
  foreign ids (`:5801`), every malformed reference (`:5880`), and missing,
  empty, and misplaced fields (`:5971`).
- A valid reference previews with counts unchanged, then commits
  (`kernel_routes.rs:5880`).
- The command reports `refused` at the preview step, writes no receipt,
  and makes one call (`memory-mark-command.test.ts:242`).
- A tip that moves between preview and commit: the preview answers
  `available`, the evidence's only project citer retires, and the commit
  answers `not_found` with nothing written
  (`a_dispute_previewed_while_its_evidence_was_visible_is_judged_again_at_commit`,
  `kernel_routes.rs:6381`). The commit's verdict is the one recorded.

## Investigation log

### Q: Does the preview report the cascade a dispute on an approval causes?

- Sources examined: `PreviewRun::disposition` (`commit.rs:845-884`), the
  module header (`commit.rs:60-63`), the command usage
  (`memory-mark-command.ts:53`), `memory-mark-command.test.ts:156`.
- Findings: no. `visibility_changes` compares the subject's current and
  projected surfaces only; dependents demoted in the same commit are not
  reported. The limit is documented in the header and in the usage text,
  and the usage test asserts the text holds "visibility only".
- Missing evidence: none; this is portfolio gap G2, resolved by
  documenting (OQ4-10).
- Conclusion: resolved with answer; recorded as an open question on
  `disposition-on-an-authority-bearer-demotes-dependents-in-the-same-commit`.
