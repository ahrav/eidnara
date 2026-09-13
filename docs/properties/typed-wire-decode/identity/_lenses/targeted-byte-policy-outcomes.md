# Targeted discovery: numeric consumers of block bytes

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
Trigger: supplied fresh evaluation `ses_f6756093fffeVjNp36S3E8pKrM` and the
plan's FlatBlock consumer table. W1 is invalidated; this is not measurement.

Hash consistency alone does not prove policy equivalence. Verified consumers:

- `crates/daemon/src/lib.rs:17275-17287`: original token counts and lengths.
- `crates/daemon/src/boundary.rs:966-1108`: token prefixes and suffix bounds.
- `crates/daemon/src/transform.rs:5885-5913,4592-4602`: protected/publication floor.
- `crates/daemon/src/transform.rs:6377-6408`: selection byte size.
- `crates/daemon/src/selection.rs:900-1011`: rounded floor/reclaim estimates.

Disposition: add `block-byte-policy-outcomes-remain-stable` rather than burying
numeric behavior inside the hash-domain record. Freeze P outcomes under fixed
policy/estimator inputs. Observe daemon-only cases separately and escalate
semantic differences; accepting 26 fewer bytes is not accepting another fold.

Narrow limit: no invented frequency, performance result, or permission to
change thresholds. The independent `typed_wire_identity_byte_policy_inputs`
marker records threshold-focused input construction, not a changed decision.
No checks have run, and existing numeric tests remain unaudited.
