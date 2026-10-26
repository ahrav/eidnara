# uo-consent-gate-source-is-the-route-frozen-config

## Discovery trigger

Spec #1145 C2 and lead L5.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1156 branch tip (the commit that adds this file;
its parent is `ed75b9e5b`).

- Bind: `crates/daemon/src/lib.rs:13422`.
- Wrapup: `prepare_wrapup_fire` (`crates/daemon/src/lib.rs:5996`) reads
  `self.effective_config` at `crates/daemon/src/lib.rs:6038`.
- Correction: the spec cited `config.rs:2630, 2753, 2762`, which are config
  tests at this tip.

- Guarantee under test: A pass reads `memory.enabled` from the config frozen at
  `route.open`; the wrapup fire reads the live effective config.
- Existing check:
  `history_summarizer_diagnostics_omit_project_memory_when_memory_is_disabled_or_not_fired`
  (`crates/daemon/src/lib.rs:39643`);
  `only_an_available_composition_is_recorded`
  (`crates/daemon/src/use_outcome_writer_tests.rs:190`);
  `a_wrapup_under_a_config_that_differs_from_the_binding_records_no_serve`
  (`crates/daemon/src/use_outcome_writer_tests.rs:536`). Status: unaudited.

## Failure scenario

A reload that disabled memory would not stop a pass on a route bound before it;
this is the documented contract, and the writer (#1155) follows the pass.

## Timing windows and dependencies

Between bind and the pass.

## What a test must construct

A reload between bind and pass, asserting the pass follows the frozen value; a
wrapup under a reloaded config (marker `uo_wrapup_fired_under_reloaded_config`).

Required faults and enabling state: A route bound with memory on, then a reload
that turns it off (and the reverse), then a pass and a wrapup.

## Investigation log

### Q: Lead L5

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Lead L5: the pass and the wrapup read consent from different
  sources. Owner: daemon owner (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.

### Q: Gap G4

- Sources examined: the specification (#1145) open questions and the code cited above.
- Findings: Gap G4: the summarizer reattach path reads `memory_enabled` from the
  live effective config (`crates/daemon/src/lib.rs:5223`, used at
  `crates/daemon/src/lib.rs:5370`), a third consent source beside the frozen
  binding and the live wrapup config; the fire path states the frozen-config
  rule in its own comment (`crates/daemon/src/lib.rs:5880`). Recorded, not
  fixed; whether the reattach should follow the frozen config is an owner
  decision (needs human input).
- Missing evidence: an owner decision or the named read.
- Conclusion: needs human input.
