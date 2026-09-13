# System lens: bug history and density

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Relevant source comparison against plan baseline `e451a2b4` finds only the
  added `pub mod search_seed;` line in daemon lib.rs. Wire types, projection,
  transform, codec sidecar, meter, producer, and inspected fixtures match.
- Local main has no `crates/daemon/src/metered_decode.rs`. It cannot serve as
  the plan's before artifact. No branch is switched.
- Local history includes `97334311` (metered decode), `7edeb90f` (bounded
  charge and classification), `33511c5e` (unescape charge), and `3de11b27`
  (admission before probe). Titles are leads, not regression proofs.
- GitHub issues 350, 435, 436, 438, 441, and 524 are read with comments.
  They establish requested constraints and adjacent ownership, not runtime
  incident evidence. No separate incidents or external repositories are supplied.

Quiet area: the decode corpus repeats ingress `mid`, but has no literal
duplicate CK `role`, block `kind`, or enum `type` case
(`crates/daemon/src/lib.rs:19762-19964`). Preserve this distinction.
