# sc-unit-cap-refuses-whole-never-prefix

## Discovery trigger

Part B obligation from #1066 comment 2, requirement R10 and decisions D3 and
D13. At baseline a prepare error failed the whole batch unnamed. The U1
ticket (#1080) adds the pre-transaction unit screen and the 64 KiB and
64 KiB + 1 tests.

## Evidence trail

`crates/daemon/src/harness_sources.rs`:

- `MAX_SOURCE_UNIT_BYTES` equals
  `memory_store::memory_capture::MAX_CAPTURE_SOURCE_BYTES` (64 KiB).
- `publish_screened` checks `unit.text.len() > MAX_SOURCE_UNIT_BYTES` first
  for every unit and records `UnitDisposition::UnitTooLarge` before
  `prepare`, before the scope read, and before any evidence ingest. The unit
  takes no admission slot, and siblings continue through screening.
- `SourcePublisher::evidence_request` sets the ingest payload to
  `unit.unit.text.as_bytes().to_vec()`, the unit's whole text.
- The adapters (`opencode_units`, `pi_units`) copy native strings whole and
  contain no truncation.

`crates/daemon/src/source_capture.rs`:

- `classify` maps `UnitTooLarge` to the `units_unit_too_large` counter and a
  terminal memo entry.
- `CaptureJob::plan_document` skips a document read whose text exceeds
  `MAX_SOURCE_UNIT_BYTES` as `documents_oversize`, retaining nothing.

The Kernel's 64 MiB payload cap (`PayloadTooLarge` in
`crates/kernel/src/cas/ingest.rs`) is a separate, larger bound and is not the
cap this record relies on.

## Failure scenario

The cap is applied after truncation, so a spliced prefix is stored and
presented as an exact source. Or one oversized unit fails the whole batch and
silences its window.

## Timing windows and dependencies

None. Screening is a pure function of the unit's length.

## What a test must construct

A real Kernel store and publisher. A unit of exactly 65536 bytes, which
publishes with artifact bytes equal to its text. Then a batch of a small
sibling, a 65537-byte unit, and a second sibling: the oversized unit is
`UnitTooLarge`, both siblings publish, the evidence table gains no row for it,
and no receipt names it. Repeating the oversized unit refuses again and
leaves the tip unchanged.

Landed checks:

- `crates/daemon/tests/harness_sources.rs`
  `capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings`
  constructs exactly the above. It asserts `evidence_rows` returns
  `(1, 65536)` then `(3, 65536)`, so no evidence row exceeds the cap, and
  exactly four new publisher receipts for the two siblings.
- `capture_batch_refusals_take_no_admission_slot` places an oversized unit
  among 32 valid units and asserts all 32 publish with no `Deferred`.
- `crates/daemon/src/source_capture_tests.rs`
  `unreadable_documents_are_counted_and_retire_nothing` writes a 70 KiB
  document and asserts `documents_oversize`.

## Investigation log

### Q: Does the supervisor path have its own oversize test?

- Sources examined: `source_capture_tests.rs`, `classify`.
- Findings: no supervisor test sends a window with a unit above 64 KiB;
  the `units_unit_too_large` counter and its terminal memo entry have no
  test. The retention decision is made in `publish_screened`, which the
  integration test exercises directly.
- Missing evidence: a supervisor-level oversize unit test.
- Conclusion: unresolved, needs a transform-path test that asserts
  `units_unit_too_large` and sibling publication.

### Q: Does every capture evidence object equal its unit text?

- Sources examined: the U1 test, `evidence_request`, `inventory` in the test.
- Findings: the test asserts the at-cap artifact equals its text and that the
  sibling texts appear in the inventory. Ingest refuses rewritten bytes with
  `ExactBytesRewritten`, which `is_permanent` marks terminal.
- Missing evidence: none for the constructed cases.
- Conclusion: resolved with answer.

### Q: Where did the baseline citations move?

- Sources examined: `harness_sources.rs` at HEAD.
- Findings: `prepare` is now at `fn prepare`, and the batch-wide failure the
  baseline cited no longer exists: `publish_screened` screens per unit.
- Missing evidence: none.
- Conclusion: resolved with answer.
