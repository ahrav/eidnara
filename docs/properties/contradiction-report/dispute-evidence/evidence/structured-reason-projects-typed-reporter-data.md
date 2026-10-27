# structured-reason-projects-typed-reporter-data

## Discovery trigger

Plan P4 decisions D5 and D9 and the specification's OQ4-1 and OQ4-2: in PR
A the reporter class and the repository reference ride in `reason`, and
PR B ([#1162](https://github.com/ahrav/eidnara/issues/1162)) adds the
typed columns beside it. The reason must therefore be a faithful,
parseable carrier, and since PR B it must equal the rendering of the
typed values the grammar carries: the event kind, `reporter_taint_class`,
`evidence_id`, and the reference in `event_payload`
(`reporter_source_class` and `use_outcome_id` have no field in the
grammar).

## Evidence trail

- Grammar: `v=1;op=kernel.commit;event=<event>;reporter_taint=<taint>;evidence_id=<pct>;path=<pct>;oid=<hex>`,
  absent fields omitted, values percent-encoded, decode-idempotent, at
  most 4096 bytes (`crates/daemon/src/kernel_routes/commit.rs:30-39`; the
  `dispute.rs` module header points there,
  `crates/daemon/src/kernel_routes/dispute.rs:1-2`).
- `render` writes the fields in order, percent-encoding every value
  (`dispute.rs:83-101`); `encode` keeps RFC 3986 unreserved bytes and
  writes every other byte as `%` and two uppercase hex digits
  (`dispute.rs:172-186`).
- `parse` reads the fields in order, rejects an unknown version, an
  unknown event or taint, a value that decodes to a different string a
  second time, a path without an oid or the reverse, and any input whose
  re-rendering differs (`dispute.rs:103-148`).
- `validated` refuses a rendering over `MAX_REASON_BYTES` (4096,
  `dispute.rs:14`) or holding a detection (`dispute.rs:77-79`). Since
  `511cd6d73` it also refuses a raw `evidence_id` holding a detection
  before rendering (`dispute.rs:58-64`); the scan of the composed reason
  alone could miss a secret the percent-encoding breaks up.
- The Kernel stores `redact(reason)` (`crates/kernel/src/admission.rs:1749`);
  `redact_lossy` calls the same `redact_durable_text`
  (`crates/kernel/src/redaction.rs:41-42`), so a reason the route accepted
  is stored unchanged and records no redaction.
- The cascade copies the reason onto each `approval_revoked` row it writes
  (header at `commit.rs:60-62`; asserted at
  `crates/daemon/tests/kernel_routes.rs:6241-6245`).

## Failure scenario

A path `a;b=c.rs` stored without encoding would split into extra fields
and `parse` would read a different path. A reason the Kernel redacted
would hold a placeholder and `parse` would fail or read a wrong value.

## Timing windows and dependencies

None in time. The reason depends on the scanner's rule set, as in
`evidence-ref-well-formed-and-redaction-stable`.

## What a test must construct

- The documented rendering for one reference
  (`the_documented_rendering_parses_back`, `dispute.rs:245`).
- Round trip over `;`, `=`, `%`, a malformed escape, `@`, spaces,
  non-ASCII, and unreserved punctuation, for both events, with an evidence
  id holding `;=%` (`render_and_parse_are_inverse_over_reserved_and_non_ascii_bytes`,
  `dispute.rs:259`).
- `encode` and `decode` on reserved and multi-byte input give uppercase
  escapes and the original string
  (`reserved_and_multi_byte_values_encode_to_uppercase_escapes`,
  `dispute.rs:293`). Every accepted path and optional id renders an ASCII
  reason within the cap that parses back to its inputs (proptest
  `every_accepted_reference_parses_back_to_its_inputs`, `dispute.rs:305`).
- Refusal of every rendering `render` does not produce
  (`parse_refuses_every_rendering_render_does_not_produce`,
  `dispute.rs:461`) and of a reason over the cap
  (`a_reason_over_the_cap_is_invalid_input`, `dispute.rs:421`).
- A client cannot supply the reason: a body carrying `reason` answers an
  error with nothing written (`kernel_routes.rs:6258`).
- Stored reasons through the route parse back, re-render to the stored
  bytes, and are ASCII (`kernel_routes.rs:5880`); event, reporter taint,
  and evidence id parse back (`kernel_routes.rs:5723`); the cascade row's
  reason parses (`kernel_routes.rs:6199`); a reason carrying both an
  evidence id and a reference parses back (`kernel_routes.rs:6291`).
- Not constructed: a read of `durable_text_redactions` after a landed
  dispute. The absence of a redaction row is argued from the shared
  detector.
- Typed form (PR B): `a_dispute_row_records_its_reporter_and_links_the_sessions_latest_serve`
  (`crates/daemon/tests/kernel_routes.rs:6651`) asserts the reporter
  columns read `(model_inference, user_inferred)` for a command-shaped
  dispute and `(model_inference, assistant_inference)` for a tool-shaped
  one, that `event_payload` holds the sent `evidence_ref` serialized byte
  for byte and is null when none was sent, and that `DisputeReason`
  rendered from the typed columns and the row's `evidence_id` equals the
  stored `reason`; `reporter_columns_are_dispute_only_and_checked_by_the_schema`
  (`crates/kernel/tests/kernel_use_outcome.rs:1456`) asserts every other
  writer stores NULL and the CHECKs refuse an unknown class, a half-set
  pair, and a payload that is not a JSON object
  (`crates/kernel/src/schema.rs:164`).

## Investigation log

### Q: Which event does a cascade row's reason name?

- Sources examined: `kernel_routes.rs:6236-6245`.
- Findings: the dependent's row has `event_kind = approval_revoked` and a
  reason byte-equal to the approval's, which names `event=mark_disputed`.
- Missing evidence: none.
- Conclusion: resolved with answer: a reader takes the row's event from
  `event_kind` and the dispute's event from the reason.

### Q: Do PR A reasons carry into PR B's typed columns?

- Sources examined: [`../open-questions.md`](../open-questions.md) Q6;
  `crates/kernel/src/schema.rs:164`; `crates/kernel/src/admission.rs:998-1003`;
  [`../../../use-outcome-record/evidence/uo-kernel-opens-only-at-the-compiled-baseline-identity.md`](../../../use-outcome-record/evidence/uo-kernel-opens-only-at-the-compiled-baseline-identity.md).
- Findings: the grammar is versioned (`v=1`) and parseable. #1162 takes
  Q6's option (a): the four U3 columns are nullable and no code reads a
  `reason` into them. A store written before #1162 carries the previous
  schema digest, and the Kernel opens a store only at the compiled
  baseline identity, so no PR A row is readable at this baseline. At this
  baseline a `mark_disputed` or `contradict` row with NULL reporter
  columns is one an in-process caller wrote through the public
  `Envelope::record_admission`, which passes no reporter
  (`crates/kernel/src/admission.rs:998-1003`); the route stores the
  reporter on every dispute it writes. Rows of every other event kind
  hold NULL in all four columns by the CHECK that ties the reporter
  columns to `mark_disputed` and `contradict`
  (`crates/kernel/src/schema.rs:164`).
- Missing evidence: none.
- Conclusion: resolved with answer: they do not, because a PR A store
  does not open at this baseline. A NULL reporter on a dispute row says
  the row was written through `record_admission` rather than the route,
  and its `reason` is whatever that caller supplied.
