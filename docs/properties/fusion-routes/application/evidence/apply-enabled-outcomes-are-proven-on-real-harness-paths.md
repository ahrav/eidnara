# apply-enabled-outcomes-are-proven-on-real-harness-paths

## Discovery trigger

The RP2.7 adapter gate requires every enabled class to have a witness on its
real harness path. The Rust-core cleanup deleted dormant host application
clients, so daemon route tests must not be presented as host edit proof.

Repository: `/local/home/ahrav/scratch/eidnara-stack`; inspected 2026-09-19 at
`d34ff88300ec947c5d3ed0ed5162422993a98d34` before this documentation change.

## Evidence trail

- `crates/daemon/tests/context_capabilities.rs:263` drives OpenCode and Pi
  routes under the recorded capability tables. It proves daemon gate behavior.
- `crates/daemon/tests/context_capabilities.rs:703` drives Pi gated-class
  denials, append allowance failure, profile mismatch, append completion, and
  lost acknowledgment under two consumer-capability string sets. Its kernel
  write counters stay unchanged. It proves route and receipt behavior, not a
  Pi system-prompt edit.
- Retired OpenCode witnesses:
  `packages/opencode-plugin/src/hooks/context/context-application.ts` and
  `context-application.test.ts`. They supplied a scripted prepare, apply,
  publish, and confirm client with a host-applied identity. Both are deleted.
- Retired Pi witnesses: `packages/pi-plugin/src/context-application-pi.ts` and
  `context-application-pi.test.ts`. They adapted the shared client to a system
  prompt slot and checked append, fallback, delimiter refusal, and lost
  acknowledgment. Both are deleted.
- Not performed and not currently reachable: application through a real
  `opencode serve` process or through `packages/e2e-tests/src/pi-runner/`.
  No daemon route produces a packed body for either plugin, and no current
  plugin application adapter publishes one.

## Failure scenario

A capability class is declared enabled because a daemon route accepts it, but
no built host has proved it can apply, identify, account for, and acknowledge
the edit on its owned surface.

## Timing windows and dependencies

None. This proof requires a real harness route, not a timing interleaving.
Lost acknowledgment remains a required fault once a host route exists.

## What a test must construct

- A daemon-produced packed body accepted by a reachable host adapter.
- A built OpenCode or Pi harness that publishes the edit on its owned surface
  and returns the host-applied identity for confirm.
- One enabled outcome per declared class, plus denied-class and lost-ack
  controls. Consumer capability strings must not widen the declaration.

## Investigation log

### Q: What do current Rust tests prove?

- Sources examined: `context_capabilities.rs:263`,
  `context_capabilities.rs:703`, and daemon receipt tests.
- Findings: capability declarations, denials, profile binding, receipt states,
  and zero kernel writes remain exercised.
- Missing evidence: any host surface publication or host-applied identity.
- Conclusion: resolved with answer - Rust tests prove daemon contracts only.

### Q: Which host witnesses were retired?

- Sources examined: current tree existence and production import searches for
  `context-application.ts`, `context-application.test.ts`,
  `context-application-pi.ts`, and `context-application-pi.test.ts`.
- Findings: all four files are deleted. The three dormant Pi runtime modules
  `context-application-pi.ts`, `pi-pressure.ts`, and `read-session-pi.ts` are
  absent from the shipped Pi graph.
- Missing evidence: replacement host adapters. None exist on current HEAD.
- Conclusion: resolved with answer - OpenCode scripted-host and Pi
  system-prompt witnesses are retired.

### Q: What remains unexercised?

- Sources examined: OpenCode and Pi build entry graphs and existing E2E harness
  directories.
- Findings: neither real OpenCode server nor Pi runner applies a daemon-packed
  body. No current plugin confirms an identity returned by host publication.
- Missing evidence: reachable adapters and real-harness scenarios for both
  plugins.
- Conclusion: unresolved, needs future host application work.
