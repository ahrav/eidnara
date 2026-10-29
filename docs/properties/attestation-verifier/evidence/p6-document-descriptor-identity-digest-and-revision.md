# p6-document-descriptor-identity-digest-and-revision

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S5. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

The capture document step builds the identity from `project_id`, `repository_id`, and `relative_path`, skips text above `MAX_SOURCE_UNIT_BYTES` (`crates/daemon/src/source_capture.rs:1297`), compares `sha256_hex` (`crates/daemon/src/project_docs.rs:144`) with the live row's `artifact_digest` (`crates/daemon/src/source_capture.rs:1325`), and proposes the lineage head plus one. The read cap is `MAX_PROJECT_DOC_BYTES` (`crates/daemon/src/project_docs.rs:23`). The verifier's `MAX_BLOB_BYTES` equals the read cap. The tests are `sha256_hex_is_the_kernel_payload_id` (`crates/daemon/src/project_docs.rs:446`), `every_request_in_a_batch_proves_its_buffer_against_the_artifact` (`crates/kernel/tests/kernel_source_descriptors.rs:1180`), `repository_document_revisions_exceed_the_historical_head_including_retired_rows` (`:2007`), and `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (`crates/daemon/src/source_capture_tests.rs:1992`).

## Failure scenario

A descriptor digest computed over other bytes than the published ones.

## Timing windows and dependencies

None.

## What a test must construct

A document published, edited, and republished.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
