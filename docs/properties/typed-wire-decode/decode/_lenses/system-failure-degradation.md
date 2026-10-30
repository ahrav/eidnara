# System lens: failure and degradation

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- `DecodeFailure::Invalid` restarts the meter and falls through; `Refused`
  returns immediately (`crates/daemon/src/lib.rs:12934-12957`). Do not turn
  every failure into fallback. Admission details remain with A1/A3.
- Invalid tree JSON becomes `Value::Null` and dispatch decides the outcome.
  Valid tree JSON with an invalid TransformRequest returns `bad_request`
  (`crates/daemon/src/lib.rs:8126-8134`). These are distinct failure classes.
- The compatibility walk rejects raw-value-token keys even when a tree parse
  alone could succeed (`crates/daemon/src/lib.rs:15876-15961`). Thus
  `BodyLane::Tree` alone does not prove that typed decode failed.
- Any page field, including null, keeps the page validation path
  (`crates/daemon/src/lib.rs:13061-13064,15823-15826`).

Candidates: fallback outcome safety plus an independent positive fallback
situation. Preserve original bytes for the second attempt; do not retry a
partially consumed typed object.
