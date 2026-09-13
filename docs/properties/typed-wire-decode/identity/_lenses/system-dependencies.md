# System lens: dependencies

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: normative wire protocol and plan Appendix C.

## Observations

The relevant dependencies are serde's field/default rules, serde_json's number
and map serialization, SHA-256, and the two local producer/consumer languages.
The workspace declares serde_json with `raw_value` in `Cargo.toml`; declaration
alone is not proof that no transitive dependency enables another feature.

`crates/daemon/src/served_json.rs:144-163` compares unescaped key bytes directly
and decodes escaped keys before sorting. It preserves scalar spans.
`crates/daemon/src/wire.rs:889-894` separately normalizes floating signed zero
for equality indexing. Neither operation is generic JSON canonicalization
under the host's unrelated control-channel rules.

## Candidate and missing evidence

Use the existing value-round-trip reference for serde's byte semantics and
pin the resolved feature set when the implementation campaign runs. No Cargo
build or dependency-update operation is part of this discovery pass.

## Narrow nonapplicability

DNS, brokers, external database retries, and cloud APIs are not dependencies
of the canonical block-byte computation. Issue reads supply leads only.
