# kd-p02-daemon-only-authority

## Discovery trigger

Proposed obligation KD-P02 of #1060's Property Catalog section B (KTD1).
Tickets #1062 and #1063; PRs #1069 and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- The daemon selects the path from the pass's applied authority
  (`crates/daemon/src/transform.rs:3341`).
- Pi witness: `packages/pi-plugin/src/transform/pi-delivery.test.ts:236`
  compares first passes from a folding project and a withdrawn project in
  one compaction-on process and from a compaction-off process: equal field
  names, and equal values once `session_id`, `base_revision`,
  `native_messages`, `request_observed_at_ms`, `system_prompt_hash`,
  `render_config`, and `tool_present` are excluded.
- OpenCode witness:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1524`
  compares both `compactionOff` values on a cold window past half the cap;
  it excludes `tool_present` and `todo_tool_present` (`:1535-1542`), which
  OpenCode resolves per session: `tool_present` from the session's frozen
  and callable reduce availability
  (`packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts:456-458`)
  and `todo_tool_present` from the active agent's todowrite permission and
  the tool's availability (`:459-471`).

## Failure scenario

A plugin flag that selected additive serving would let a plugin override
the daemon's stored authority and break FA-N01 parity.

## Timing windows and dependencies

None.

## What a test must construct

Sessions bound folding and native in one process; both OpenCode modes.

## Investigation log

### Q: Why are `system_prompt_hash` and `render_config` excluded?

- Sources examined: the observed differing fields during #1069.
- Findings: Both carry the system prompt's hash, which names the session's
  working directory, so they vary per session and carry no authority.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Why are the capability fields excluded from the equality?

- Sources examined: `packages/pi-plugin/src/index.ts:597-598`;
  `pi-delivery.test.ts:259-277`; `rust-mode-window.test.ts:1535-1542`;
  `opencode-transform-adapter.ts:456-471`.
- Findings: Pi's `tool_present` is process-scoped: KD-P03 requires `false`
  in a compaction-off process, and the Pi witness asserts
  `[true, true, false]` for the folding, withdrawn, and compaction-off
  passes, so an equality over it would fail on correct code. OpenCode's
  `tool_present` and `todo_tool_present` are session-scoped verdicts from
  the session's earliest persisted user row and the active agent, so they
  differ between sessions for reasons unrelated to authority; their own
  tests check them (`rust-mode-transform.test.ts:554`, `:576`, `:832`;
  `rust-mode-window.test.ts:1593`, `:1608`, `:1662`). KD-P03 is Pi-only and
  checks neither OpenCode field. All of them report capability and select
  no fold path.
- Missing evidence: None.
- Conclusion: resolved with answer; the record's Check excludes them.
