# parity-directive-command-writes-only-after-ui-confirmation

## Discovery trigger

Plan U4 step 1 ("build a preview via `directive.preview`, require the
confirm dialog, then call `directive.record` / `revise` / `revoke` with
actor `user:pi` or `user:opencode`") and step 4 ("Refuse to run without a UI
confirm (guard against `sendUserMessage`-style queued text)"); U4 test
scenario "Invocation without a confirming UI records nothing". Serves R1,
R7, R13 (identical actor semantics across harnesses).

## Evidence trail

- `packages/opencode-plugin/src/shared/memory-mark-command.ts:30-32`: the
  existing runner accepts `--yes` in place of a dialog; `:81-91`
  `confirm?:` is optional and "A surface without a dialog leaves it out, and
  the decision falls to the user's next invocation"; `:135-137` when the
  preview reports a visibility change and `confirmed` is false, no `confirm`
  yields `needs_confirmation` (nothing written), a `false` answer yields
  `declined`; `:139-141` `isCancelled` is checked before the commit.
- `packages/pi-plugin/src/commands/eidnara-memory-mark.ts:17` actor
  `user:pi`; `:66-75` `confirm` is passed only when `ctx.hasUI`, wrapping
  `ctx.ui.confirm`; `:50-57` `isCancelled` compares the session id.
- `packages/opencode-plugin/src/hooks/context/command-handler.ts:389-390`
  actor `user:opencode`; "OpenCode's command path has no dialog, so the
  confirmation is the reply itself: the user re-issues the command with the
  confirm flag"; `:411-417` no `confirm` is passed.
- Pi docs (`docs/extensions.md:254` of the installed 1.3.0): "use
  `ctx.hasUI` for interactions supported by interactive and RPC clients";
  headless runs have no UI.
- Plan U3 step 5: the daemon refuses asserted class fields and empty text;
  the harness gate is additive to those route checks.

## Failure scenario

Legal witness (Pi): `add "x"` -> `directive.preview` -> `ctx.ui.confirm`
shows the rendered line and scope -> user accepts -> `directive.record`
with `actor: "user:pi"`, `operation_key` -> applied, receipt shown.

Smallest violating traces:
1. Flag substitutes for dialog: the directive runner inherits the
   memory-mark `--yes` semantics. Any text source that can reach the
   handler (an RPC `prompt()`, a future `sendUserMessage` with expansion, a
   script) records an `explicit_user` row with no human dialog. The plan's
   step 4 forbids exactly this.
2. Headless Pi: `ctx.hasUI` is false; the runner proceeds without `confirm`
   because the preview reported no "visibility change" (the memory-mark
   runner asks only when `visibility_changes`). For a new directive there is
   no prior visibility to change, so a runner reusing that predicate never
   asks. The directive runner must require confirmation unconditionally.
3. Stale preview: `confirm <object_id>` previews candidate M, the user reads
   it, M is superseded by the model's own tool call before the user accepts,
   `directive.record` records with `confirmed_from = M` and `ExplicitReject`
   on M (now superseded). The route's answer decides (N1); the harness must
   show the route's verdict rather than "applied".
4. Actor drift: one harness sends `actor: "user"`; the daemon stores a
   different actor string per harness, breaking the parity claim about
   identical semantics and any later audit keyed on the two literals.
5. OpenCode has no dialog: the runner, as specified, can never record on
   OpenCode. Either the feature is Pi-only there or a weaker confirmation is
   accepted. This is the conflict recorded in `open-questions.md`.

## Timing windows and dependencies

- The confirm dialog is user time; the session can be replaced
  (`isCancelled`) or the candidate can move. The runner re-checks
  cancellation before the commit; it cannot re-check the candidate, so the
  route's refusal is the authority.
- Depends on N1 for the route's idempotency (`operation_key`) so that a
  second confirm after an unknown outcome replays rather than duplicates.

## What a test must construct

1. Runner unit tests with a fake kernel client recording calls:
   - with `confirm` returning `true`: calls are `[preview, record]`; the
     body's `actor` is the harness literal; `operation_key` is present.
   - with `confirm` returning `false`: calls are `[preview]`; outcome
     `declined`.
   - with no `confirm` callback: calls are `[preview]` or `[]`; outcome is a
     distinct `needs_ui` (not `needs_confirmation` with a re-run hint).
   - with any `--yes`-like token in the arguments: parse error or ignored;
     never a substitute.
   - `isCancelled` true after confirm: no `record` call; outcome cancelled.
2. Pi registration test: `confirm` is wired to `ctx.ui.confirm` only when
   `ctx.hasUI`; when false the runner gets no callback.
3. Wire-body tests per subcommand (`add`, `confirm`, `revise`, `revoke`):
   method literal and body shape as §7.8 will specify (N1 pass owns the
   shapes).
4. E2E (AE1, Pi harness with UI): `add` then a pass shows the text under
   `USER_DIRECTIVES`; `revoke` then a pass shows it absent.

## Investigation log

### Q: Does OpenCode expose any confirmation dialog to a plugin command?
- Sources examined: `command-handler.ts:389, 406-422`; the plan's U4.
- Findings: the existing code states there is no dialog and uses a textual
  flag. The plan requires a dialog. No OpenCode dialog API was located in
  this pass.
- Missing evidence: the OpenCode plugin API surface for prompts.
- Conclusion: needs human input - choose between a Pi-only authoring
  surface, an OpenCode two-step textual confirmation (weaker than the plan),
  or an OpenCode dialog if one exists.

### Q: Why is the gate unconditional for directives but conditional for marks?
- Sources examined: `memory-mark-command.ts:135-137`; plan KTD1.
- Findings: a mark changes an existing row's disposition and the preview can
  say whether visibility changes; a directive creates an `explicit_user` row
  whose authority is the point, so every record is a visibility change in
  the relevant sense.
- Conclusion: resolved with answer - the directive runner asks every time.
