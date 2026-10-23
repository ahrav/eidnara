# sc-harness-capture-deadline-replay-never-renews

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R11 and decision D6. The
U4 ticket (#1084) lands the publisher retention fields, names the record with
fault F11 (two publications of one unit 30 days apart with the first expired),
and resolves OQ14 as a per-row deadline where the earliest owner wins.

## Evidence trail

`crates/daemon/src/harness_sources.rs` - `SourcePublisher::evidence_request`
sets `retention_class = kernel::HARNESS_CAPTURE_RETENTION_CLASS` and
`retain_until = observed_at.saturating_add(HARNESS_CAPTURE_RETENTION_MS)` for
`OccurrenceClass::Messages` and `OccurrenceClass::RawToolSpans`, and
`("canonical", None)` for every other class. `HARNESS_CAPTURE_RETENTION_MS`
is `memory_store::memory_capture::CAPTURE_REPLAY_RETENTION_MS`, which
`crates/memory-store/src/memory_capture.rs` defines as
`30 * 24 * 60 * 60 * 1000`. `observed_at` is the caller's clock argument to
`publish`, `publish_within`, `publish_screened`, and `publish_batch`, carried
in `Prepared::observed_at`; it is not the native record timestamp.

Replay: `publish_within` previews `stored_receipt` for the descriptor intent
(`source-descriptor:{key}`) and returns `replayed: true` before any ingest.
When only the evidence receipt (`source-evidence:{key}`) exists, the ingest
runs under the digest that wrote it, so the Kernel replays the first evidence
row. `publish_batch` does the same through its `retained` map and builds an
evidence request only for an owner unit with no retained evidence. The
request digest (`request_digest`, `digest_inputs`) binds text, domain, scope,
role, sensitivity, egress, and the outcome record, and never the clock, so a
later attempt with another `observed_at` matches the stored receipt.

Shared text: `publish_batch` maps each pending unit to the first pending unit
of the same publisher with equal text (`owners`), and only owners get an
evidence request. Every unit of one batch shares one `observed_at`, so a
non-owner's descriptor cites a row whose deadline equals its own.

`crates/kernel/src/cas/ingest.rs` - the artifact-level request check refuses
`InvalidInput` for `curator_capture` or `harness_capture` with
`retain_until: None`. The reference-commit liveness check (`retain_until >
current_time_ms()`) applies to `MEMORY_REVIEWER_CAPTURE_RETENTION_CLASS`
only. `crates/kernel/src/cas/mod.rs` - `HARNESS_CAPTURE_RETENTION_CLASS` is
`"harness_capture"`. `crates/kernel/src/schema.rs` documents the class in the
`retention_class` schema comment.

Landed checks:
`crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it`
publishes an OpenCode message at `NOW` and a Pi tool result at `NOW + 5`,
asserts `(harness_capture, NOW + 30 d)` and `NOW + 30 d + 5`, asserts a
`git_commit` row and a repository document stay `("canonical", None, None)`,
republishes the message at `deadline + 1 d` and asserts `replayed` with the
deadline unchanged, sweeps at `deadline - 1` (nothing, tip unchanged) and at
`deadline` (one retired), then republishes after expiry and asserts a replay
with no new staged artifact and the deadline still the first one.
`crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`
asserts a `harness_capture` ingest with `retain_until: None` is refused
`InvalidInput` with the tip unchanged.
`crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress`
drives the maintenance slice at `4_999` and `5_000` over a row due at
`5_000`.

## Failure scenario

A publisher that stamps `retain_until` from the attempt clock, or a digest
that binds the clock, turns every retry or every pass into a fresh evidence
row or a renewed deadline, so captured text never ages out. A publisher that
writes `canonical`/`None` for the two classes keeps every capture forever.

## Timing windows and dependencies

The deadline is fixed when the evidence row is first created. The window that
matters is a later attempt of the same unit: a restart (memo loss, F3) or a
pending unit retried after its first evidence expired. Both answer from the
receipts. An evidence-only receipt whose evidence the sweep already retired
(evidence with no descriptor is a sweep candidate once due) is a corner the
landed tests do not construct.

## What a test must construct

A message unit and a tool unit published at a known `observed_at`; read
`evidence_meta.retention_class` and `retain_until` directly. A `canonical`
row and a repository document in the same store. A second publication of the
message after its deadline, then a sweep at `deadline - 1` and `deadline`, and
a publication after expiry. The legal witness is evidence at
`retain_until - 1` surviving maintenance. A complete test also drives
`publish_screened` (the path `SourceCapture` uses) with two equal-text units
in one batch and asserts both descriptors cite one row whose deadline is the
batch's `observed_at + 30 d`.

## Investigation log

### Q: Per-unit versus per-evidence-row deadline under shared text (OQ14)
- Sources examined: `ticket-1084.md` Design Decisions; `evidence_request`
  doc comment; `publish_batch` owner mapping.
- Findings: evidence intents are keyed per unit, so units share a row only
  inside one batch, where `observed_at` is identical.
- Missing evidence: no test publishes equal-text units in one batch and reads
  their deadline.
- Conclusion: resolved with answer - per-row deadline, earliest owner wins,
  landed in U4.

### Q: Does a harness capture require a live deadline at reference commit?
- Sources examined: `cas/ingest.rs` request check and reference-commit check.
- Findings: only `curator_capture` is checked against `current_time_ms()`; a
  `harness_capture` ingest with a past deadline commits (the kernel test
  ingests one at `retain_until: Some(NOW)` with `NOW = 10_000`).
- Missing evidence: whether the contract requires the check; R11 and D6 do
  not name it.
- Conclusion: resolved with answer - not checked at HEAD; reported to the
  lead as a difference from the curator class.

### Q: Does an evidence-only receipt whose evidence expired republish?
- Sources examined: `publish_within`, `publish_batch` `retained` handling,
  `expired_captures_sql`.
- Findings: the later attempt cites the retired evidence id.
- Missing evidence: a test that retires evidence-only rows by expiry and
  republishes the unit.
- Conclusion: unresolved, needs a test of that sequence.

### Q: Re-admission of identical bytes after their artifact is purged (OQ21)
- Sources examined: `ticket-1084.md`; `artifact_is_blocked` in
  `cas/ingest.rs`.
- Findings: U4 lands under the purge tombstone; re-observed bytes are refused
  `ReAdmissionBlocked`.
- Missing evidence: the Kernel owner's decision.
- Conclusion: needs human input.
