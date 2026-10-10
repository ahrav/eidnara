# source-health-is-advisory-and-secret-free

- `crates/host-runtime/tests/source_health_vectors.rs:5`
  `rust_sanitizer_matches_the_shared_vectors` runs the 27 cases in
  `crates/host-runtime/tests/fixtures/source-health-vectors.json`, including
  canaries for profile, account, role ARN, path, session, hash, token, and
  error text.
- `packages/opencode-plugin/src/shared/source-health.test.ts:25` runs the same
  vectors through the TypeScript parser; `:68` shows a missing, malformed, or
  extra-key block is unknown and never ready.
- `crates/host-runtime/tests/model_execution_protocol.rs:408`
  `health_reports_the_cached_source_observation_for_each_mode` reads the cached
  observation for each source mode.
- `health()` reads only the cached cell; no test counts its I/O.
