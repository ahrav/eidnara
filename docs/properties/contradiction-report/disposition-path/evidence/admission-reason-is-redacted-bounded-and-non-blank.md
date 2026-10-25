# admission-reason-is-redacted-bounded-and-non-blank

Catalog record: [catalog.md](../catalog.md#admission-reason-is-redacted-bounded-and-non-blank).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

Plan P4 OQ4-1 and OQ4-2: encode the reporter class and `evidence_ref` in a
structured `reason` for PR A; "Confirm the `reason` byte bound at
implementation." Scope Boundaries: "The `reason` column is redacted
(`admission.rs:1684`) and sized for codes, not prose." Lens: resource
boundaries and data integrity. The record states what the column enforces
before it becomes a carrier.

## Evidence trail

- Blank check: `request.event.reason.trim().is_empty()` is
  `AdmissionPolicy` in `prepare_with_facts`
  (`crates/kernel/src/admission.rs:1560-1562`) and in
  `revoke_approval_inner` (`:1364-1366`).
- Bound: `redact(value)` returns `InvalidInput` when `value.len() >
  MAX_REDACTABLE_BYTES` (`crates/kernel/src/redaction.rs:72-77`);
  `MAX_REDACTABLE_BYTES = secret_scanner::MAX_INPUT_BYTES = 512 * 1024`
  (`crates/context-core/src/redaction.rs:19`;
  `crates/secret-scanner/src/api.rs:6`).
- Redaction: `redact_lossy` runs `redact_durable_text` and returns the
  rewritten text plus detections (`redaction.rs:41-47`); `write_admission`
  stores `reason.text` (`admission.rs:1684`, `:1772`) and calls
  `record(tx, "admission_decision", id, "reason", &reason, commit_seq)`
  (`:1782-1789`), which inserts one `durable_text_redactions` row per
  detection (`redaction.rs:104-130`).
- The pending change also carries `redactions: vec![("reason", reason)]`
  (`admission.rs:1825`) into the change-event payload.
- Schema: `reason TEXT NOT NULL`, no `CHECK` on length
  (`crates/kernel/src/schema.rs:162`).
- Route value: the constant `"kernel.commit"` (`commit.rs:46`, `:466`,
  `:596`); the route cannot produce a blank, oversize, or secret-bearing
  reason at HEAD.
- Other production reasons: `format!("{kind:?}")` (`lib.rs:13905`),
  `CLASSIFY_KERNEL_PRODUCER` (`lib.rs:15059`), `CAUSE`
  (`harness_sources.rs:977`).
- Test: `a_revocation_reason_survives_without_dependents`
  (`kernel_admission.rs:1676`) reads a stored revocation reason.

## Failure scenario

OQ4-1 places a structured value (reporter class, path, git object id) in
`reason`. If `redact_durable_text` matches a repository path segment or a
hex object id as a secret, the stored structure is rewritten with a
placeholder and the `evidence_ref` is unrecoverable; the only trace is the
`durable_text_redactions` row. A reason built from user-controlled text
can also approach 512 KiB and abort the whole envelope with
`invalid_input` after the subject's facts were read.

## Timing windows and dependencies

None in time. Depends on the detector set in `crates/secret-scanner` and
on `redact_durable_text` in `crates/context-core/src/redaction.rs`, not
read in this pass.

## What a test must construct

- Store API `record_admission` with a reason of exactly
  `MAX_REDACTABLE_BYTES` bytes (accepted) and one byte more
  (`InvalidInput`, no row).
- A reason with a known-detected token; assert the stored `reason` differs
  and a `durable_text_redactions` row with `owner_kind =
  'admission_decision'`, `field_name = 'reason'` exists.
- A reason that is a repository path plus a 40-hex and a 64-hex id (the
  OQ4-2 shape); assert the stored text equals the input.
- Coverage markers `admission_reason_at_bound`, `admission_reason_redacted`.

## Investigation log

### Q: Can `redact_lossy` alter a repository path or a 40/64-hex object id?

- Sources examined: `redaction.rs:41-47`, `:72-77`;
  `context-core/src/redaction.rs:19`; `secret-scanner/src/api.rs:6`. The
  detector list and the label-word heuristics (`context-core/src/
  redaction.rs:21+`) were not read.
- Findings: the redactor is lossy by design ("the placeholder is not
  recoverable", `context-core/src/redaction.rs:17-18`). Whether a bare hex
  string or a path triggers a detector is undetermined here.
- Missing evidence: the detector patterns and a run of `redact_durable_
  text` over the OQ4-2 shape.
- Conclusion: partial; unresolved, needs a read of the `secret-scanner`
  detectors or a direct test.
