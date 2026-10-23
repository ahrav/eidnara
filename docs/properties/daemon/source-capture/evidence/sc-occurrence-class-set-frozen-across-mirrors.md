# sc-occurrence-class-set-frozen-across-mirrors

## Discovery trigger

U9 adds `RepositoryDocuments`, which changes a frozen identity contract held
in four places at once: the Kernel encoder, the eval-core mirror, the
retrieval `occurrences.class` CHECK, and the identity fixtures. Part A
recorded the five-class contract so U9 could prove the five encodings stay
byte-identical while the set grows.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/source_identity.rs`: `OCCURRENCE_ENCODING_VERSION = 2`;
  `MAX_IDENTITY_VALUE_BYTES = 512`; `OccurrenceClass::ALL` has six classes
  in the order `messages`, `canonical_claims`, `promoted_memory`,
  `git_commits`, `raw_tool_spans`, `repository_documents`;
  `identity_fields` gives `repository_documents` the fields
  `project_id, repository_id, relative_path`; `representations` gives
  `text`; `HARNESSES = ["opencode", "pi"]`; `OBJECT_FORMATS` sha1/40 and
  sha256/64; `encode_preserving_span` resolves the class first and returns
  `OccurrenceRefusal::UnknownClass` for any other code.
- `crates/eval-core/src/occurrence.rs`: the same six codes in the same
  order, with matching fields and representations.
- `crates/retrieval/baseline.sql`: `occurrences.class` CHECK lists the six
  codes. PR #1081 records `SCHEMA_VERSION` 7 to 8 with rebuild.
- `crates/kernel/tests/fixtures/search-projection/source-identity-fixtures.json`:
  24 records (U9 appended `rd1`).

Tests read at HEAD:

- `crates/kernel/tests/eval_identity.rs`,
  `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder`:
  asserts the contract version, encoding version, 512-byte cap, harnesses,
  object formats, and class codes in order are equal across eval-core and
  the Kernel; fields and representations agree per class; all 24 records
  encode through both encoders to their pinned occurrence and lineage ids;
  string refusals agree by name and order.
- `crates/retrieval/tests/schema_inventory.rs`,
  `the_check_vocabularies_equal_the_rust_enums`: the `occurrences.class`
  CHECK equals `OccurrenceClass::ALL` codes.
- `crates/kernel/tests/kernel_source_descriptors.rs`,
  `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals`
  iterates the classes against the fixtures, including `unknown_class` and
  `precedence_class_before_everything`.

## Failure scenario

One mirror gains or reorders a class, or changes a field order, and two
components derive different occurrence ids for one native source, so the
projection keys rows the Kernel never published or the evaluator scores the
wrong occurrence.

## Timing windows and dependencies

None at runtime. A binary older than projection schema 8 cannot re-encode a
`repository_documents` descriptor; PR #1081 states U10's producer activates
only after older binaries are retired.

## What a test must construct

The four mirrors read side by side, every fixture record through both
encoders, and a class name outside the set. No fault injection is needed.

## Investigation log

### Q: Are the five pre-U9 encodings byte-identical?
- Sources examined: pr-1081.md; the golden test above.
- Findings: the fixture diff only appends `rd1`; the existing 23 records
  still encode to their pinned ids at HEAD.
- Conclusion: resolved with answer.

### Q: Is `RepositoryDocuments` in scope (OQ1)?
- Sources examined: ticket-1081.md, pr-1081.md, spec.md OQ1.
- Findings: ticket-1081 names OQ1 as an owner gate and lists no blocker;
  pr-1081 states the class landed dark with no producer until U10. Neither
  file records a product-owner confirmation.
- Missing evidence: a written owner confirmation.
- Conclusion: resolved for landing in U9 per the brief; the confirmation
  record itself is unresolved, needs the owner's note on #1066.
