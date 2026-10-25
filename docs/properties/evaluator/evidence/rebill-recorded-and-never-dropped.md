# rebill-recorded-and-never-dropped

## Discovery trigger
Parent specification #1119 C18, D4, OQ7-2 (resolved in #1124: option (b), a required report
record); Part B record `rebill-recorded-and-never-dropped`.

## Evidence trail
- `crates/eval-core/src/report.rs:209` `SuiteBReport.prefix_rebill`, keyed as
  `arm_rates` is, under `eval-suite-b-report/v3` (`:34`).
- `crates/eval-core/src/report.rs:1041` `parse_report`: a stale `schema` is
  `SchemaMismatch`; an object without the key is `PrefixRebillMissing`
  (`:1054`).
- `crates/eval-core/tests/report.rs::a_report_refuses_missing_blocks_forbidden_claims_and_what_it_did_not_derive`
  maps the removed key to the named refusal and every other key to a shape
  error.
- `crates/eval-core/tests/manifest.rs::fixture_digests_are_frozen` and
  `::required_fields_are_sorted_and_equal_the_struct_field_set` hold the
  manifest unchanged.

## Failure scenario
A runner that drops the record under memory capture would publish a report
whose memory arm costs only its per-task usage.

## Timing windows and dependencies
None.

## What a test must construct
A report document with `prefix_rebill` removed under a capture-on family; a
key set other than `aged`, `fresh`.

Legal witness: a report with zero records for both arms parses and the
manifest digest equals its pinned constant. Smallest falsifier: a report
without the record parses with total 0.

## Investigation log
### Q: Why a named pre-decode check rather than serde's missing-field error?
- Sources examined: the manifest parser's `MissingField` precedent
- Findings: serde reports a missing field as a shape message; the ticket
  requires a dedicated variant
- Missing evidence: none.
- Conclusion: resolved with answer - `PrefixRebillMissing`
