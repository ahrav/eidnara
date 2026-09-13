# Targeted discovery: durable lineage anchor

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
Trigger: supplied fresh evaluation `ses_f6756093fffeVjNp36S3E8pKrM`.

Competing explanations: the anchor is diagnostic metadata, or exact projected
bytes enforce continued-lineage behavior. The enforcement path is verified at
`crates/daemon/src/transform.rs:2183-2196,2199-2259` and durable storage at
`crates/memory-store/src/lib.rs:1703,10364-10366`.

The failure is not necessarily a request error or forced HARD pass. At
`crates/daemon/src/transform.rs:3957-3960,4614-4616,4782-4788`, it selects
Defer, keeps reconciliation pending, and uses a no-trim output metadata view.

Disposition: add `durable-lineage-anchor-preserves-validation` with unchanged
P anchors and real violation controls. `typed_wire_identity_durable_anchor`
independently marks supplied completed-descent state and anchor inputs.

Narrow limit: unknown-envelope legacy hashes are not proven absent. R3 does
not specify how to migrate an already durable anchor. The specification owner
must resolve that conflict; no relaxed validator or stored-hash rewrite is
authorized. Existing anchor tests remain unaudited and the record unexercised.
