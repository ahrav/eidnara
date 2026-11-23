# source-health-is-advisory-and-secret-free

## Discovery trigger

Specification #860, record N10: `always(no credential I/O in source health &&
no secret/selector canary && absent/malformed != ready)`, exercised for every
state through the Rust sanitizer and the TypeScript consumers, with N2 still
enforced against a stale `ready` sample. D6 makes health a sampled derivative
that is advisory only. The M4 ticket (#872) lands the record with its runnable
checks.

## Evidence trail

- `crates/host-runtime/src/model_execution/source_health.rs:7` fixes the five
  closed `FIELDS`; `:83` `SourceHealth::to_json` emits only `kind`, `state`,
  the two bounded seconds fields, and `consecutive_failures`, clamping numbers
  at their bounds; `:208` `sanitize` admits a block only in the closed shape.
- `source_health.rs:132` `SourceHealthCell` is the shared cached observation;
  `:139` `get` reads the cell under its mutex and demotes a ready row past its
  expiry to unknown. It performs no credential I/O.
- `crates/host-runtime/src/model_execution/aws_refresh.rs:263` `health()`
  clones that cell for the owner's consumers.
- `crates/host-runtime/src/model_execution/mod.rs:451` reads the cached cell
  with `self.source_health.get()` and `:463` serializes it with `to_json`. The
  surrounding probe at `:439` checks harness descriptors on the blocking pool;
  it touches no credential.
- `crates/host-runtime/tests/source_health_vectors.rs:5`
  `rust_sanitizer_matches_the_shared_vectors` runs the 29 cases in
  `crates/host-runtime/tests/fixtures/source-health-vectors.json` through
  `sanitize`, checks `fields` against `FIELDS` and
  `max_consecutive_failures` against `u32::MAX`. The cases cover every kind and
  state, each number at and above its bound, negative and fractional numbers,
  missing and unknown fields, a non-object and an array, and canaries for
  profile name, account, role ARN, path, session, hash, token, and error text.
  An earlier draft of this file counted 27 cases; the fixture holds 29.
- `packages/opencode-plugin/src/shared/source-health.test.ts:25` runs the same
  vectors through the TypeScript parser; `:68` shows a missing, malformed, or
  extra-key block is unknown and never ready, with a canary value that must not
  appear.
- `crates/host-runtime/tests/model_execution_protocol.rs:408`
  `health_reports_the_cached_source_observation_for_each_mode` reads the
  `aws_credentials` block for no source (`kind: none`), an environment row
  (`kind: environment`, `state: ready`), and a selected profile before any
  observation (`kind: profile`, `state: unknown`).

## Failure scenario

A status surface serializes a field outside the closed shape and a profile
name, account, role ARN, session, or token reaches a log or a client. Or a
health read refreshes credentials, so a status poll performs the network and
subprocess work that only a dispatch may charge.

## Timing windows and dependencies

None for the serialization half: the block is a pure function of the cached
observation at read time. The no-I/O half depends on every reader going
through `SourceHealthCell::get`; a reader that reached the owner's refresh path
would break it without changing the serialized shape.

## What a test must construct

Blocks with extra keys, out-of-set values, out-of-bound and malformed numbers,
and canary strings for each sensitive field, in both the Rust and TypeScript
parsers; a host health report for each source mode; and, for the no-I/O half,
a counter on the owner's refresh path asserted at zero across health reads.

## Investigation log

### Q: Is the absence of credential I/O in health witnessed at runtime?

- Sources examined: `source_health.rs:139`, `aws_refresh.rs:263`, `mod.rs:434`
  through `:463`, the three tests above.
- Findings: every health reader goes through the cached cell; no test counts
  refresh calls across a health read.
- Missing evidence: an I/O counter or scripted-dispatch call count asserted
  unchanged by `health()`.
- Conclusion: needs human input - whether `health()` carries an I/O counter so
  this half has a runtime witness. Until then the record is `partial`.
