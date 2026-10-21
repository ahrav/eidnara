# kd-p02-daemon-only-authority

## Discovery trigger

Proposed obligation KD-P02 of #1060's Property Catalog section B (KTD1).
Tickets #1062 and #1063; PRs #1069 and #1070. Surface: plugin and Pi.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- The daemon selects the path from the pass's applied authority
  (`crates/daemon/src/transform.rs:3341`).
- Pi witness: `packages/pi-plugin/src/transform/pi-delivery.test.ts:213`
  compares first passes from a folding project and a withdrawn project in
  one compaction-on process and from a compaction-off process: equal field
  names, and equal values once `session_id`, `base_revision`,
  `native_messages`, `request_observed_at_ms`, `system_prompt_hash`,
  `render_config`, and `tool_present` are excluded.
- OpenCode witness:
  `packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1438`
  compares both `compactionOff` values on a cold window past half the cap;
  the capability values follow the adapter's per-mode rules.

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
  working directory; they differ per session, not per authority.
- Missing evidence: None.
- Conclusion: resolved with answer.
