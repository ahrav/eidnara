# memory-mark-command-is-model-free-and-confirms-before-writing

## Discovery trigger

Lens: architecture (S5), security boundary, claimed guarantees C11. The
brief named "the memory-mark command runner's model-free invocation path on
both harnesses (confirm dialog, sentinel abort, registration only in the
top-level Pi entry)". KTD1 and U4 copy this runner for `/eidnara-directive`,
so its write discipline is the template N5 inherits.

## Evidence trail

Runner (`packages/opencode-plugin/src/shared/memory-mark-command.ts`):

- `:1-7` header: names an `EventKind`, never a disposition; "the harness
  asks for confirmation only when a serving surface would display a
  different state after the event".
- `:98-100` `memoryMarkOperationId = [sessionId, event, objectId].join(SEP)`.
- `:117-158` `runMemoryMarkCommand`: `previewDispositions` first (`:126`);
  non-available -> `refused/preview` (`:127-128`); `previewed.receipt ===
  undefined` branch (`:130-139`): verdict must match the operation
  (`:131-134`), `denied` -> `denied` (`:134`), `visibility_changes &&
  !confirmed` -> `needs_confirmation` without a `confirm`, or `declined` when
  `confirm` resolves false (`:135-138`); a replay receipt skips the gate;
  `isCancelled()` -> `refused/cancelled` (`:140-141`); `client.commit`
  (`:142`); commit verdict must match (`:144-146`); commit-side `denied`
  (`:148`); `applied` with `replayed` (`:149-154`).
- Pi wrapper (`packages/pi-plugin/src/commands/eidnara-memory-mark.ts`):
  `:28` `pi.registerCommand(MEMORY_MARK_COMMAND, ...)`; `:48-56`
  `isCancelled` throws-to-true when `ctx` is invalidated; `:65-72` `confirm`
  supplied only when `ctx.hasUI`, through `ctx.ui.confirm`; `:76` no reply
  when the session is gone. Actor `user:pi` (`:17`).
- Pi registration site: `packages/pi-plugin/src/index.ts:550`, after the env
  guard (`:332-336`) and latch (`:343-348`). `subagent-entry.ts` calls
  `registerEidnaraTools` only (`:55-66`) and contains no `registerCommand`.
- OpenCode (`packages/opencode-plugin/src/hooks/context/command-handler.ts`):
  `:389-390` "OpenCode's command path has no dialog, so the confirmation is
  the reply itself"; `executeMemoryMark` (`:392-422`) with
  `isSessionDeleted` as `isCancelled` and sentinel aborts at `:411` and
  `:420`; dispatch at `:528`; the shared tail sends the notification and
  `throwSentinel(input.command)` at `:713`; `throwSentinel` (`:141-152`)
  builds `__CONTEXT_MANAGEMENT_<CMD>_HANDLED__` with status 204.
- Tracker #314 acceptance rows match: tightening without confirmation,
  confirmation on a verified automatic object, denied relaxation, replay,
  preview writes nothing.
- Tests: `memory-mark-command.test.ts:93-290` (12), `eidnara-memory-mark.test.ts:64-150`
  (5), `command-handler.test.ts:757-850` (4), `index-env-guard.test.ts:38`.

## Failure scenario

A runner that committed before the confirmation resolved, or that treated a
foreign verdict as the user's, would write a disposition the user did not
approve. For the directive channel the same defect writes an
`(explicit_user, user_explicit)` authority row from queued text, which R1
("no model tool call can reach that route") is meant to exclude. The
`--yes` flag (`MEMORY_MARK_CONFIRM_FLAG`) lets a text-only surface confirm
without a dialog; U4 step 4 says the directive command refuses to run
without a UI confirm, a stricter rule than this runner has.

## Timing windows and dependencies

Preview and commit are two daemon calls; the object may change between
them and the commit's verdict is reported (`:147-148`). The session may end
during the dialog; both wrappers re-check before the commit.

## What a test must construct

1. Fake client where the preview reports `visibility_changes: true`; call
   without `confirm` and with `confirmed: false`; assert `needs_confirmation`
   and zero commits. With `confirm` resolving false: `declined`, zero
   commits. Resolving true: one commit.
2. Fake client returning a replay receipt from the preview; assert one
   commit with no `confirm` call.
3. `isCancelled` flipping true after the preview; assert `refused/cancelled`
   and zero commits.
4. Static: `subagent-entry.ts` contains no `registerCommand`; `index.ts`
   contains exactly one `registerEidnaraMemoryMarkCommand` call after the
   env guard.
5. Coverage: `memory_mark_preview_reported_visibility_change` and
   `memory_mark_session_ended_between_preview_and_commit` fire.

## Investigation log

### Q: Does the directive command keep the `--yes` flag?

- Sources examined: `memory-mark-command.ts:30-33`, `:135-138`;
  `eidnara-memory-mark.test.ts:117` ("falls back to the confirm flag when no
  dialog is available"); plan U4 step 4.
- Findings: the template accepts `--yes` without a UI; the plan's directive
  command refuses to run without a UI confirm. The two rules differ.
- Missing evidence: the implementation decision for `/eidnara-directive`.
- Conclusion: needs human input.
