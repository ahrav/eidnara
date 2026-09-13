# System lens: claimed safety guarantees

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R3-R7, KTD2, and latency-audit B1.

## Claims and discriminating code

- R4 promises unchanged plugin block bytes and hashes. HEAD retains raw JSON
  (`crates/memory-store/src/lib.rs:250-279`), so simply deriving serialization
  does not establish this claim.
- R3 drops unknown envelope fields. Existing comments and tests preserve
  them (`lib.rs:108-110,16096-16112`). This is an accepted prospective change,
  not evidence those tests already cover the replacement contract.
- R5 permits false metadata and synthetic tool false omissions. The typed
  metadata already skips false (`lib.rs:72-89`), but typed tool flags do not
  (`lib.rs:340-352`).
- R7's common fresh byte basis differs from equality identity. Equality
  normalization at `crates/daemon/src/wire.rs:885-903` preserves signed-zero
  equality without making the two emitted strings equal.

## Candidate and handoff

Keep canonical-byte, equality, and persisted-identity checks distinct.
Send legacy unknown-field assertions to invariant-test review for replacement
contract analysis rather than declaring them adequate or obsolete automatically.

## Narrow nonapplicability

No consensus or authentication guarantee is inferred from these byte contracts.
