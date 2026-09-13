# System lens: product context

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan goal capsule and issue 350.

## Observations

The plugin's ordinary transform request requests native output
(`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:745`).
CK projection therefore feeds both durable ingress identity and provider-facing
native reconstruction. Loss of a signature, opaque payload, or tool failure
variant is observable independently of allocation savings.

`crates/daemon/src/wire.rs:652-715` preserves provider extras and tool failure
classification when reducing blocks. `transform.rs:5176-5244` can reject drift
on a covered, anchored, or frozen message. A decode optimization can therefore
affect established sessions, not just one new turn's JSON.

## Candidate and handoff

Preserve byte identity for the actual plugin shape family. Treat explicit false
and unknown envelope input as accepted normalization changes, not as plugin
observations. Verify persisted-session and native-cache cold paths.

## Narrow nonapplicability and missing evidence

No production frequency or business-impact measurements were supplied.
Latency targets and payoff belong to the plan's measurement work, not identity.
