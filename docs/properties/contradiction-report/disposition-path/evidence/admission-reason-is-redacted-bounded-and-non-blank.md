# admission-reason-is-redacted-bounded-and-non-blank

Catalog record: [catalog.md](../catalog.md#admission-reason-is-redacted-bounded-and-non-blank).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 OQ4-1 and OQ4-2: encode the reporter class and `evidence_ref` in a
structured `reason` for PR A; "Confirm the `reason` byte bound at
implementation." Scope Boundaries: "The `reason` column is redacted
(`admission.rs:1749`) and sized for codes, not prose." Lens: resource
boundaries and data integrity. The record states what the column enforces
before it becomes a carrier.

## Evidence trail

- Blank check: `request.event.reason.trim().is_empty()` is
  `AdmissionPolicy` in `prepare_with_facts`
  (`crates/kernel/src/admission.rs:1616-1618`) and in
  `revoke_approval_inner` (`:1417-1419`).
- Bound: `redact(value)` returns `InvalidInput` when `value.len() >
  MAX_REDACTABLE_BYTES` (`crates/kernel/src/redaction.rs:72-77`);
  `MAX_REDACTABLE_BYTES = secret_scanner::MAX_INPUT_BYTES = 512 * 1024`
  (`crates/context-core/src/redaction.rs:19`;
  `crates/secret-scanner/src/api.rs:6`).
- Redaction: `redact_lossy` runs `redact_durable_text` and returns the
  rewritten text plus detections (`redaction.rs:41-47`); `write_admission`
  stores `reason.text` (`admission.rs:1749`, `:1846`) and calls
  `record(tx, "admission_decision", id, "reason", &reason, commit_seq)`
  (`:1860-1867`), which inserts one `durable_text_redactions` row per
  detection (`redaction.rs:104-130`).
- The pending change also carries `redactions: vec![("reason", reason)]`
  (`admission.rs:1903`) into the change-event payload.
- Schema: `reason TEXT NOT NULL`, no `CHECK` on length
  (`crates/kernel/src/schema.rs:164`).
- Route value: the constant `"kernel.commit"` (`commit.rs:100`; inserts at
  `:600`) for every event except the two disputes; a dispute carries the
  composed `v=1` reason (`commit.rs:223-229`, passed through at `:739`).
  At the PR A tip the route cannot produce a blank, oversize, or
  secret-bearing reason: `DisputeReason::validated` refuses a rendering
  over 4096 bytes or holding a detection
  (`crates/daemon/src/kernel_routes/dispute.rs:75-77`), and every
  rendering starts with `v=1;op=kernel.commit`.
- Other production reasons: `format!("{kind:?}")` (`lib.rs:14067`),
  `CLASSIFY_KERNEL_PRODUCER` (`lib.rs:15224`), `CAUSE`
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
on `redact_durable_text` in `crates/context-core/src/redaction.rs`, which
the initial pass at `3a3b916b7` did not read; the second investigation
question below records the read at the PR A tip and its conclusion.

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
- Conclusion: at `3a3b916b7`, partial; unresolved, needs a read of the
  `secret-scanner` detectors or a direct test. Resolved at the PR A tip
  (portfolio gap G3), see the next question.

### Q: Which token does the scanner detect, and do hex object ids and ordinary paths pass (G3)?

- Sources examined: `crates/kernel/src/redaction.rs:41-42`
  (`redact_lossy` calls `context_core::redaction::redact_durable_text`);
  `crates/context-core/src/redaction.rs:462-464`;
  `crates/daemon/src/kernel_routes/dispute.rs:373-380`, `:362-375`;
  `crates/secret-scanner/conservative_overlay.yaml:5-7`;
  `crates/secret-scanner/default_rules.yaml:410-411`;
  `crates/secret-scanner/src/rules.rs:31`, `:299-306`.
- Findings: `a_path_the_secret_scanner_detects_is_invalid_input` asserts
  that `redact_durable_text` detects
  `sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678`.
  `git_object_ids_and_ordinary_paths_carry_no_detection` renders 64
  seeded SHA-256 digests at 40 and 64 hex beside
  `src/kernel_routes/commit.rs` and asserts no detection. By the rule
  regexes, the token matches the conservative overlay rule
  `magic-anthropic-api-key` and not the default `anthropic-api-key` rule,
  which requires 93 characters and a trailing `AA`; the overlay is
  embedded with the default rules (`rules.rs:31`, `:299-306`).
- Missing evidence: the detector id reported for the token was not
  observed; the rule attribution is inferred.
- Conclusion: resolved with answer for detection (the token) and for
  pass-through (hex ids and an ordinary path). Because the route refuses
  any dispute reason holding a detection before the Kernel sees it, a
  route-written reason is stored unchanged.

