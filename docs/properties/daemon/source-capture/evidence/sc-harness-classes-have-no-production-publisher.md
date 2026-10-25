# sc-harness-classes-have-no-production-publisher

## Discovery trigger

Part A record at baseline `08de9051a`: no production code published `messages`
or `raw_tool_spans` descriptors. The spec named U2 as the unit that inverts it.
Re-verified at HEAD `5a842d998`, where it is invalidated.

## Evidence trail

- Baseline claim: adapters and `SourcePublisher` had callers only under tests,
  examples, and `#[cfg(test)]`; `publish_batch` had one production caller in
  `crates/daemon/src/claim_sources.rs`.
- HEAD production chain:
  - `crates/daemon/src/lib.rs`: the transform handler builds a
    `source_capture::Offer` when `source_capture::consents(...)` is true, and
    `Handler::capture_after_response` calls `SourceCapture::offer`, then
    `CaptureHandoff::into_job`, and spawns `CaptureJob::run` through the
    admission spawner after the response settles.
  - `crates/daemon/src/source_capture.rs` `CaptureJob::publish` builds a
    `SourcePublisher` with domain `harness-capture`, `ProviderEgress::LocalOnly`,
    and `Sensitivity::Sensitive`; `CaptureJob::units` runs `opencode_units` or
    `pi_units`; `CaptureJob::publish_locked` calls `publish_screened`, which
    calls `publish_batch`.
- Gate: `source_capture::consents` requires `config.source_capture_enabled`,
  `config.memory_enabled`, `!is_subagent`, a native serializer profile, and a
  profile harness equal to the route's. `DaemonConfig::default()` sets
  `source_capture_enabled: false` (`crates/daemon/src/config.rs`).
- Inverting test: `crates/daemon/src/source_capture_tests.rs`
  `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  passes one OpenCode window through the handler and reads one live `messages`
  descriptor for `msg_u1` and `raw_tool_spans` descriptors for `call_bash` and
  `call_read`.
- Gate test: `capture_off_memory_off_or_a_subagent_session_runs_no_adapter`
  asserts every report counter is 0 and the kernel tip is unchanged for key
  off, memory off, and a subagent session.
- Label: `crates/eval-core/src/manifest.rs` `Ingestion::AdapterIngestedNoProductionCaller`
  still exists with the serde name `adapter-ingested, production caller:
  none`. The `enum Ingestion` doc comment says the evaluator drives its
  ingestion adapters outside their production callers.

## Failure scenario

This record is invalidated, so its original falsifier (any harness descriptor
after a production pass) now fires by design with the key on. The live risk
moves to the consent predicate: publication with the key off, memory off, or a
subagent session. That obligation belongs to `sc-consent-gate-admits-capture`.
The R1 reach obligation, that every text block, tool output, and tool error of
a consented window reaches `publish_screened`, belongs to
`sc-consented-window-reaches-publisher`.

## Timing windows and dependencies

The production publisher runs after the response settles, on the admission
spawner, under the capture commit gate. None of this affects the inversion
itself.

## What a test must construct

For the inversion: a capture-enabled handler with a real kernel, one pass, and
a class inventory read. For the replacement obligation: the three consent-off
cases with an unchanged tip. Both exist at HEAD.

## Investigation log

### Q: Which unit inverted the record, and which code?
- Sources examined: `ticket-1082.md` (Part A records this ticket inverts),
  `lib.rs` `capture_after_response`, `source_capture.rs`
  `CaptureJob::publish_locked`.
- Findings: U2 (#1082) added the supervisor and the production call to
  `publish_screened`.
- Missing evidence: none.
- Conclusion: resolved with answer: invalidated by U2; the consent bound is
  carried by `sc-consent-gate-admits-capture` and the R1 reach obligation by
  `sc-consented-window-reaches-publisher`.

### Q: Does the default daemon still publish nothing?
- Sources examined: `config.rs` default, `consents`.
- Findings: yes. With `DaemonConfig::default()` the key is false and no
  adapter runs. The record is still invalidated because its premise, that no
  production publisher exists, is false; the publisher is one user-tier switch
  away.
- Missing evidence: none.
- Conclusion: resolved with answer: reachability is `explicit-config-only`.

### Q: Is the evaluator label consistent with HEAD?
- Sources examined: `crates/eval-core/src/manifest.rs`,
  `crates/daemon/examples/eval_runner/aging.rs`.
- Findings: the variant is still used by the aging runner. Its doc comment
  says the evaluator drives its ingestion adapters outside their production
  callers, which agrees with HEAD; only the serde name keeps the wording
  `production caller: none`.
- Missing evidence: whether the evaluator part renames the label.
- Conclusion: the doc comment is resolved with answer. The rename is a
  manifest schema-compatibility decision recorded as an open question on
  `ing-adapter-ingested-no-production-caller` in
  `docs/properties/evaluator/catalog.md` (needs human input).
