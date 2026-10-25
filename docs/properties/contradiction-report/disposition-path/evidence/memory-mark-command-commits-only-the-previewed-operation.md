# memory-mark-command-commits-only-the-previewed-operation

Catalog record: [catalog.md](../catalog.md#memory-mark-command-commits-only-the-previewed-operation).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Plan P4 KTD3: "`runMemoryMarkCommand` is already shared by both harnesses;
the tool action reuses the same client call." Lens: protocol contracts and
product context: this is the surface a user confirms through, and P4 will
add arguments to it. The record pins what the shared runner guarantees
today.

## Evidence trail

- One operation: `const operation: DispositionOperation = {op:
  "disposition", object_id, event}` (`packages/opencode-plugin/src/shared/
  memory-mark-command.ts:115-119`); both calls send `operations:
  [operation]` with the same `intent` object (`:126`, `:142`).
- Intent: `actor: input.actor`, `operationId = memoryMarkOperationId(
  sessionId, args)`, `cause = "/eidnara-memory-mark <event> <object_id>"`,
  `sourceKind: "user"` (`:120-125`).
- `verdictFor` returns null unless `results.length == 1` and the single
  verdict's `object_id` and `event` equal the operation's (`:103-111`).
- Preview branch: `refused/preview` on a non-available state (`:127-128`);
  when `receipt` is present (replay) the preview judges nothing and
  confirmation is skipped (`:130`); a null verdict is `refused/preview`
  with `invalid("internal")` (`:131-133`); `denied` returns before any
  commit (`:134`); confirmation only when `visibility_changes &&
  !args.confirmed`, asked once (`:135-138`).
- `isCancelled` is checked before the commit (`:140-141`).
- Commit branch: non-available is `refused/commit` (`:143`); a null
  verdict is `unavailable("outcome_unknown")` because a receipt exists
  (`:144-146`); `denied` from the commit is reported as `denied` (`:148`);
  otherwise `applied` with `commitSeq` and `replayed` (`:149-154`).
- Actors: `user:opencode` (`hooks/context/command-handler.ts:390`),
  `user:pi` (`packages/pi-plugin/src/commands/eidnara-memory-mark.ts:17`).
- Client: `targetIds` collects tokens for `supersede_decision` and
  `retire_decision` only (`client.ts:634-641`), so a disposition commit
  sends no tokens; `previewDispositions` sends `commitBody(.., true)`,
  which adds `preview: true` (`:587-631`, `:763`, `:779`).
- Tests: `memory-mark-command.test.ts:92-346`, including the mismatched-
  verdict matrix at `:275`.

## Failure scenario

A refactor that rebuilds the operation for the commit (for example adding
`--yes` into the body) would change `request_digest` and hit
`operation_key_reused` on a retry, or commit an operation the preview did
not describe. A `verdictFor` that accepted the first verdict regardless of
`object_id` would let a daemon bug apply a disposition to one object while
reporting another. Treating a missing commit verdict as `refused` would
invite a retry under a fresh identity after a write that may have landed.

## Timing windows and dependencies

Preview and commit are two round trips with a confirmation dialog between
them; the session can end (`isCancelled`), the object can move, and the
daemon can restart. A replayed preview carries no verdict, so the
confirmation question is never asked for a recorded identity.

## What a test must construct

- A transport stub that captures both request bodies; assert
  `operations`, `intent`, and `source_kind` equal across preview and
  commit.
- Stubs returning zero, two, or mismatched verdicts in each step; assert
  the outcome kinds (`refused/preview/internal`,
  `refused/commit/outcome_unknown`).
- A preview with `receipt` present; assert `confirm` not called and the
  commit sent.
- Coverage marker `command_received_mismatched_verdict`; correlate with
  `preview_then_commit_tip_moved` in one history for the moved-object
  branch.

## Investigation log

No open questions.
