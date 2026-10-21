# parity-directive-command-dispatch-is-host-user-only

## Discovery trigger

Plan requirement R1 ("A user-invoked command in each harness records a
directive ... no model tool call can reach that route"), KTD1 ("The route
method is the trust boundary ... the model tool client never constructs a
`directive.*` call"), U4 step 2 ("Pi: `pi.registerCommand`, registered only
in the top-level entry") and step 4 ("guard against `sendUserMessage`-style
queued text"). Serves R1, R3, AE2; the key decision "No model in any
authority path".

## Evidence trail

Pi, pinned `@earendil-works/pi-coding-agent` 0.80.2
(`packages/pi-plugin/package.json:33`), at
`node_modules/.bun/@earendil-works+pi-coding-agent@0.80.2+72022b8d6e175469/
node_modules/@earendil-works/pi-coding-agent/dist/core/agent-session.js`:
- `:708-722` `prompt(text, options)`: `expandPromptTemplates` defaults to
  `true`; when true and the text starts with `/`, `_tryExecuteExtensionCommand`
  runs and, if the command exists, the handler is invoked and the prompt is
  not sent.
- `:839-845` `_tryExecuteExtensionCommand` looks the name up in the extension
  runner's registered commands and calls `command.handler(args, ctx)`.
- `:1020-1048` `sendUserMessage` calls `prompt` with `expandPromptTemplates:
  false` (comment at `:1042`: "to skip command handling and template
  expansion"). An extension cannot dispatch a command through it.
- `:904, 921, 963-969` `_queueUserInput` and `_throwIfExtensionCommand`:
  queued steer or follow-up text naming an extension command throws
  "cannot be queued".
- Locally installed pi 1.3.0 (`/local/home/ahrav/.pi/agent/install/releases/
  1.3.0/.../dist/core/agent-session.js:1881-1908`) adds
  `options.expandPromptTemplates` to `sendUserMessage` (default false). An
  upgrade would let another extension dispatch the command
  programmatically. The model still has no tool that calls
  `sendUserMessage`.
- `packages/pi-plugin/src/index.ts:332-337`: `EIDNARA_PI_SUBAGENT=1` child
  processes return before any registration; `:338-340` in-process
  `@gotgenes/pi-subagents` children re-run the factory.
- `packages/pi-plugin/src/index.ts:550`: `registerEidnaraMemoryMarkCommand`
  is the registration pattern.

OpenCode:
- `packages/opencode-plugin/src/hooks/context/command-handler.ts:487-500`:
  `command.execute.before` is the host hook; `:389` "OpenCode's command path
  has no dialog"; `:406-422` `executeMemoryMark` runs the shared runner.
- `packages/opencode-plugin/src/features/builtin-commands/commands.ts:38-41`
  (plan-cited): the command template registry.

Daemon:
- Plan KTD1 and `crates/daemon/src/kernel_routes/commit.rs:236-312`
  (plan-cited `derive_classes`): every plugin `kernel.commit` write is
  `model_inference`; `directive.*` is a separate method family. Verification
  of the route itself belongs to the N1 pass.

## Failure scenario

Legal witness: the user types `/eidnara-directive add "Run cargo fmt before
every commit"` in the Pi TUI; `prompt()` dispatches the handler; the handler
previews, confirms, records with actor `user:pi`. On OpenCode the user types
the same; `command.execute.before` fires; the runner records with actor
`user:opencode`.

Smallest violating traces:
1. Model tool path: the model calls `eidnara_memory create` with the same
   text; the row is `(model_inference, user_inferred)`, Hidden. No violation;
   this is AE2 and must stay so. The violation would be a tool whose
   execute path calls `directive.record`; none exists and none may be added.
2. Queued text (Pi 0.80.2): an extension calls
   `pi.sendUserMessage("/eidnara-directive add x")`; `prompt` runs with
   `expandPromptTemplates: false`; the text becomes a user prompt to the
   model, not a command. No violation at 0.80.2.
3. Queued text (Pi >= 1.3.0): the same call with `{ expandPromptTemplates:
   true }` dispatches the handler. The handler must then refuse without a UI
   confirmation (next record), which is the covering control; without it,
   an extension-authored directive is recorded as `explicit_user`.
4. Subagent child: a `@gotgenes/pi-subagents` in-process child re-runs the
   factory and registers the command; a child's `ctx.ui.confirm` may not be
   a human dialog. Whether this is reachable from a child's model is
   unknown; recorded as needs human input.
5. Plugin bypass: a plugin-side helper constructs a `directive.record`
   request outside the command runner (for example from a hook). The route
   cannot tell; only code review and the absence of a `directive.` literal
   outside the runner guards it.

## Timing windows and dependencies

- Dispatch happens "even during streaming" (`agent-session.js:713`), so a
  command can run while the model is mid-turn; the daemon commit and the
  pass's read are at different snapshots and the next pass sees the row.
- Depends on `parity-directive-command-writes-only-after-ui-confirmation`
  as the covering control for traces 3 and 4.

## What a test must construct

1. Static assertion in the plugin tests: `grep` over
   `packages/opencode-plugin/src/tools/**` and the Pi tool backends finds no
   `directive.` literal; the only call sites are the command runner.
2. Pi unit test: the registration function is not called when
   `EIDNARA_PI_SUBAGENT=1` (the factory returns at `index.ts:333-337`).
3. Pi behavior test against the pinned version: `pi.sendUserMessage("/eidnara-
   directive add x")` results in zero kernel-client calls (the text is a
   prompt, not a command). Mark it version-pinned so an upgrade fails it
   loudly rather than silently changing semantics.
4. E2E (AE2): the model's `eidnara_memory create` with directive text yields
   a Hidden row and no `USER_DIRECTIVES` entry.
5. Daemon marker (N1 pass): the `directive.*` handler is never entered with a
   request whose route-bound harness is a tool client; `unreachable`.

## Investigation log

### Q: Can a model-driven action reach `pi.registerCommand` handlers?
- Sources examined: `agent-session.js` (0.80.2) `prompt`,
  `sendUserMessage`, `_queueUserInput`, `_throwIfExtensionCommand`.
- Findings: only `prompt()` with expansion dispatches; `sendUserMessage`
  disables expansion; queued input throws. The model has no `prompt()`
  surface. RPC clients call `prompt()`, which is a host user surface.
- Conclusion: resolved with answer for 0.80.2 - host user only; version
  drift at 1.3.0 is recorded.

### Q: Do in-process Pi child sessions register the command?
- Sources examined: `index.ts:338-340` comment; `subagent-runner.ts:115`.
- Findings: the comment says children re-run the factory and the spawned-
  child guard does not run; a process-global latch records the full runtime.
  Whether `registerCommand` is skipped for a child is not determined from
  the code read in this pass.
- Conclusion: needs human input (and a code read of the latch in U4).
