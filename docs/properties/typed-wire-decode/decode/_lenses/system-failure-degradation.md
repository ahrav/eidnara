# System lens: failure and degradation

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- `DecodeFailure::Invalid` restarts the meter and falls through; `Refused`
  returns immediately (`crates/daemon/src/lib.rs:12923-12946`). Do not turn
  every failure into fallback. Admission details remain with A1/A3.
- Invalid tree JSON becomes `Value::Null` and dispatch decides the outcome.
  Valid tree JSON with an invalid TransformRequest returns `bad_request`
  (`crates/daemon/src/lib.rs:8115-8123`). These are distinct failure classes.
- The compatibility walk rejects raw-value-token keys even when a tree parse
  alone could succeed (`crates/daemon/src/lib.rs:15864-15949`). Thus
  `BodyLane::Tree` alone does not prove that typed decode failed.
- Any page field, including null, keeps the page validation path
  (`crates/daemon/src/lib.rs:13050-13053,15811-15814`).

Candidates: fallback outcome safety plus an independent positive fallback
situation. Preserve original bytes for the second attempt; do not retry a
partially consumed typed object.
