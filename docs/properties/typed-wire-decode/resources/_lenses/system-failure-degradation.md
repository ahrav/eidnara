# System lens: failure and degradation

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

`DecodeFailure::Invalid` restarts into the tree lane; `Refused` exits at
`crates/daemon/src/lib.rs:12931-12946`. `decode_metered` checks refusal before
interpreting the serde result, then checks trailing bytes at
`crates/daemon/src/metered_decode.rs:329-348`.

A late error can occur after a large prefix was allocated. Final retained
bytes may be zero while peak memory was large. Meter restart clears the
needed count but keeps charges (`:201-207`); a measurement must not reset its
peak there. Error construction and cleanup also belong in the ledger.

Candidates: failure/fallback peak coverage and no classification parse outside
admission. Crash recovery is N/A to this synchronous decode change.
