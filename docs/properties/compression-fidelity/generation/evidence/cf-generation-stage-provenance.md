# cf-generation-stage-provenance

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
[Source and evidence flow][contract] (historical plan lines 175-179,209-212)
requires distinct authored, parsed/healed, fallback, and published observations.
External scope is the supplied plan and
local code/docs/history. No additional incident logs or related repositories
are supplied. No tests or model calls run here.

## Evidence trail

- [validate.rs:320-341][validator], `parse_history_segment_output`, materializes
  P2/P3 fallbacks and default-empty P4. Their resulting values alone do not
  identify which tier tags were authored in raw XML.
- [validate.rs:595-617,1004-1069][validator] heals tool-only gaps and terminal
  completed arcs before mapping parsed ranges to native boundary IDs.
- [validate.rs:645-709,773-783][validator] can discard the final segment,
  changes the publication floor, and derives extraction outcome against the
  retained citable range. Ordinal inclusion is not a claim about meaning.
- [history_summarizer.rs:2432-2564][driver], `publish_output_from_awaiting`,
  persists output state, rejects capped/invalid output, validates, and calls
  publication. Its pending publication retains validated JSON and aliases.
- [history_summarizer.rs:48-83,661-666,712-753][driver] converts effective
  tiers to store rows and returns a distinct publication success or rejection.
- [validate.rs:1656-1730][validator] has P1-only and lenient-closure fixtures.
  [citations_golden.rs:311-343][golden] has discard/citation controls.
- [history_summarizer.rs:6139-6199,6389-6420][driver] has capped-output and
  invalid-primary/valid-fallback checks. All these tests are unaudited.

Reachability is `explicit-config-only`: [config.rs:139-145][config] has no
default models, and [lib.rs:5685-5757][lib] gates production assembly on the
configured chain and eligible firing. High confidence concerns visible stage
transformations, not an exercised end-to-end provenance assertion.

## Failure scenario

A producer emits only P1. A report counts the filled P2/P3 fields as independent
authored tiers. Another report credits material in a discarded final segment
because it appears in raw output, or attributes a fallback model's publication
to the failed primary. Every endpoint may behave as designed while the
cross-boundary fidelity claim is wrong. These are synthetic scenarios.

A competing explanation is that the producer intentionally authored identical
tiers or storage lost a row. Raw XML distinguishes authored equality from
fallback. Parsed/validated observations and the actual store result distinguish
discard or publication refusal from missing evidence in the reporter. Do not
use aggregate row or backend-completion counts as the primary oracle.

## Timing windows and dependencies

Capture raw output before parsing loses authored-presence information. Observe
coverage before and after healing/discard, then the real store publication.
Retain each source-bound attempt through retry and reattachment. A selected
source change during await must not be credited to the earlier source; an
unrelated tail extension is a distinct control, not automatically a failure.
The plan-to-HEAD diff changes the Curator lifecycle. This lane does not claim
its activation/recovery behavior is requalified by generation source inspection.

## What a test must construct

Use P1-only, empty-P4, lenient-close, healed-tool-gap, and discard-last outputs;
include valid history with rejected citations and capped output. Drive a
rejected primary followed by a valid fallback and read published rows.
Check artifact/attempt identities, effective body bytes, accepted ranges,
extraction status, and explicit nonpublication against owning observations.
Keep the original native obligation even if its range is discarded.
None found for the joined fidelity provenance harness. Existing endpoint
checks are reuse targets, not duplicated atomicity or healing properties.

## Investigation log

### Q: Can stored tiers reconstruct authored generation?
- Sources examined: Parser fallback and stored conversion ranges above.
- Findings: P2/P3 are filled before storage; raw authored presence is lost in
  those values. Equal stored strings permit competing histories.
- Missing evidence: A replay capture retaining raw XML beside stage results.
- Conclusion: Resolved: stored tiers alone are insufficient.

### Q: Can the complete stage join use existing private observations?
- Sources examined: Plan KTD2, lib.rs:18252-18276, and publication orchestration.
- Findings: Private test ancestry and scripted execution exist. Capturing
  pre-heal ranges and authored presence still needs implementation work.
- Missing evidence: The joined replay, including Curator lifecycle attribution
  at the inspected HEAD rather than the plan's older revision.
- Conclusion: Unresolved, needs a private orchestration witness, not new APIs.

[contract]: https://github.com/ahrav/eidnara/issues/707
[validator]: ../../../../../crates/daemon/src/history_summarizer_validate.rs
[driver]: ../../../../../crates/daemon/src/history_summarizer.rs
[golden]: ../../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[config]: ../../../../../crates/daemon/src/config.rs
[lib]: ../../../../../crates/daemon/src/lib.rs
