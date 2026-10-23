# sc-consent-gate-admits-capture

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R3 and decision D4. The
U1 ticket (#1080) adds the `source_capture.enabled` key and resolves OQ15 as
two switches. The U2 ticket (#1082) adds the gate and its AC9 gate-half test.

## Evidence trail

`crates/daemon/src/config.rs` - `ConfigKey::SourceCaptureEnabled` maps to the
pointer `/source_capture/enabled` and to `TierClass::UserOnly`;
`DaemonConfig::default` sets `source_capture_enabled: false`. The test
`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`
asserts that a project tier cannot enable it, and the module
`source_capture_parity` covers the Rust side of the key parity.

`crates/daemon/src/source_capture.rs` - `consents` returns true only when
`config.source_capture_enabled`, `config.memory_enabled`, `!is_subagent`,
`profile.serves_native()`, and the profile's harness equals the route's
claimed harness all hold. `SourceCapture::offer` is the only constructor of a
`CaptureHandoff`, and `CaptureJob::units` is the only caller of
`opencode_units` and `pi_units` on the capture path.

`crates/daemon/src/lib.rs` - the transform pass computes `consents` from
`binding.config` once, before the pass runs, and builds a
`source_capture::Offer` only when it is true
(`consents.then(|| source_capture::Offer { .. })`). Document reads for
capture are allocated only under `consents && binding.config.inject_docs`.
`capture_after_response` then calls `SourceCapture::offer`.

`crates/daemon/src/memory_capture.rs` - `capture_enabled` gates memory
capture on `memory_enabled`, `auto_promote`, and `memory_auto_capture`, and
`capture_checkpoint` skips `is_subagent`. Source capture reads neither
`auto_promote` nor `memory_auto_capture`, as OQ15 decides.

## Failure scenario

A worker reads `memory_enabled` only, or a late config read admits a subagent
handoff. The adapters then copy exact user text and tool bytes the user never
granted, and publication retains them for 30 days.

## Timing windows and dependencies

The predicate is evaluated once per pass from the route binding the pass
resolved. The job carries the `Offer` and never re-reads the config, so a
config reload between the pass and the job leaves the job under the snapshot
that admitted it. A reload that turns the key off stops capture from the next
pass on. No test constructs a reload between pass and job.

## What a test must construct

A real handler with a Kernel store and a memory store, one OpenCode window,
and each of: key off, `memory_enabled` off, `is_subagent` true, and a route
whose claimed harness differs from the serializer profile. For each case
assert that no job was admitted, no adapter refusal was counted, and the
Kernel tip is unchanged. A legal witness is AE1: with every switch on, one
user text block publishes one `messages` row.

Landed checks:

- `crates/daemon/src/source_capture_tests.rs`
  `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` runs the
  first three cases and asserts every report field is zero except
  `oldest_deferred_age_ms`, and the Kernel tip is unchanged.
- `a_route_claiming_another_harness_captures_nothing` binds a `pi` route,
  sends an OpenCode window, and asserts `admitted == 0` and an unchanged tip.
- `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  is the legal witness with every switch on.

## Investigation log

### Q: Two switches or the existing three (OQ15)?

- Sources examined: `ticket-1080.md` (Resolved OQ15), `ticket-1082.md`
  (Resolved OQ15), `source_capture::consents`,
  `memory_capture::capture_enabled`.
- Findings: U1 resolves OQ15 as two switches; `consents` reads
  `source_capture_enabled` and `memory_enabled` and never
  `memory_auto_capture` or `auto_promote`.
- Missing evidence: none.
- Conclusion: resolved with answer. Two switches plus the subagent skip,
  landed in U1 (key) and U2 (gate).

### Q: Does the test observe adapter invocation directly?

- Sources examined: `capture_off_memory_off_or_a_subagent_session_runs_no_adapter`,
  `CaptureJob::run`, `CaptureJob::units`.
- Findings: the adapters run only inside a job, and every job increments
  `admitted` in `CaptureJob::run`. An all-zero report therefore shows no job
  ran and no adapter refusal was counted. The test does not instrument the
  adapters themselves.
- Missing evidence: a direct adapter-call counter.
- Conclusion: resolved with answer. The zero `admitted` counter is an
  adequate proxy because no other path calls the adapters on capture.

### Q: Where do the baseline citations sit at HEAD?

- Sources examined: `crates/daemon/src/memory_capture.rs`.
- Findings: `capture_enabled` starts at line 570 and `capture_checkpoint`'s
  subagent skip is at line 694, matching the baseline citations.
- Missing evidence: none.
- Conclusion: resolved with answer.
