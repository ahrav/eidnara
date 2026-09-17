# apply-enabled-outcomes-are-proven-on-real-harness-paths

## Discovery trigger

The RP2.7 specification's adapter capability gate section and the RP2.7.U5
acceptance criteria state this obligation; the companion bundle proposed the
slug as an unexercised `test-only` record.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u4-context-edits` at
`342cd18e`; inspected 2026-09-16. Harness-side witnesses re-inspected
2026-09-17 on `rp28/u5b-pi-packing`.

## Evidence trail

- `crates/daemon/tests/context_capabilities.rs`
  `the_real_harness_tables_allow_exactly_the_recorded_classes` drives
  `opencode` and `pi` routes under the recorded tables.
- `packages/opencode-plugin/src/hooks/context/context-application.ts`
  `ContextApplication.run` is the harness-side client: prepare, apply, publish
  through the target's `edit`/`publish` adapter, confirm. The applied outcome
  carries the identity the host returned from `publish`; a lost or rejected
  publication is `unknown`, never `applied`.
- `packages/opencode-plugin/src/hooks/context/context-application.test.ts`
  drives that client against a scripted daemon: the applied identity on
  `append` (`echoes the daemon's profile at apply, edits the live surface, and
  reports the applied outcome`), the lost acknowledgment under both daemon answers,
  the receipt-alone negative control, the confirm that cannot reach the daemon
  after publication, the confirm-stage terminal read as `unknown`, the
  capability fallback latched per route, and the minted-shape check on
  `preparation_id` (`treats a preparation id outside the minted shape as a
  malformed answer and never applies it`).
- `crates/daemon/tests/context_capabilities.rs`
  `pi_pure_packing_yields_one_outcome_set_whatever_the_consumer_advertises_and_writes_nothing`
  drives every gated class, an over-allowance preparation, a foreign profile
  echo, and the append lifecycle through two `pi` binds and observes one
  outcome set with the kernel tip and projection counter unchanged.
- `packages/pi-plugin/src/context-application-pi.ts` `editSystemPrompt` is the
  Pi surface adapter: one trailing owned block, `keep` on a window refusal or a
  block whose id or body reproduces the open delimiter, the whole prompt
  charged under `pi-heuristic`.
- `packages/pi-plugin/src/context-application-pi.test.ts` drives the shared
  client with the system-prompt slot: the appended block under the daemon's
  echoed profile, the preparation failure by reason, the denied class falling
  back to `append` once without a second block, the lost acknowledgment as
  `unknown`, and the unlocatable block refused (`never writes a block whose
  body or id reproduces the open delimiter, so the owned block stays
  locatable`).
- Not performed: the run against a real OpenCode server and the run through
  the Pi runner. Neither plugin has a production caller for the client because
  no daemon route yet produces a packed body.

## Failure scenario

A class is declared enabled without a harness proving it.

## Timing windows and dependencies

None: the proof is a harness run, not a race.

## What a test must construct

- A scripted daemon answering prepare, apply, and confirm for each harness
  client, with the applied identity observed at the host surface.
- A running harness with the RP2.8.U5 apply, for the end-to-end arm.

## Investigation log

### Q: Where is the capability truth read?

- Sources examined: `HandlerCore::bind`, `RouteScope`, the prepare handler.
- Findings: the declaration is read exactly once, at bind, and every gated
  prepare reads the latched copy; nothing reads consumer strings.
- Missing evidence: none for the gate.
- Conclusion: resolved.

### Q: Are the enabled outcomes witnessed on each harness's own surface?

- Sources examined: `context-application.ts`, `context-application.test.ts`,
  `context-application-pi.ts`, `context-application-pi.test.ts`,
  `context_capabilities.rs`.
- Findings: the OpenCode client witnesses the applied identity for `append`
  against a scripted daemon and the `replace` edit shape through `editEntries`;
  the Pi client witnesses pure
  packing on the system-prompt slot and every gated class denied, with the
  daemon-side outcome set pinned under two consumer-string sets.
- Missing evidence: the real OpenCode server run and the Pi runner run.
- Conclusion: resolved for the scripted-daemon arm; the end-to-end arm stays
  open.
