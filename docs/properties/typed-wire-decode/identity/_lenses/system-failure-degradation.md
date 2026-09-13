# System lens: failure and degradation

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R2/R6 and latency-audit A2/W5.

## Observations

- `crates/daemon/src/lib.rs:12923-12951` falls back from invalid direct decode
  to the tree lane. Resident refusals terminate rather than falling back.
- `lib.rs:16443-16461` skips missing or undecodable persisted raw messages.
  A compatibility regression can appear as missing history rather than an error.
- `crates/daemon/src/codec/sidecar.rs:169-174,407-410` has serialization
  fallbacks to `Null` and empty bytes. These do not prove typed serialization
  is infallible; the prospective helper must have an explicit error contract.
- `crates/daemon/src/transform.rs:5176-5196` may reject identity drift.

## Candidates and seams

Compare known-good historical row recovery with expected message identities,
not just absence of errors. Delegate malformed/duplicate/refusal equivalence
to the decode catalog and preserve its returned accepted value for identity.
The source references in dirty `lib.rs` are verified with `git show HEAD`.

## Narrow nonapplicability

No network outage injection is needed to distinguish deterministic byte bases.
No new retry or fallback policy is designed in this catalog.
