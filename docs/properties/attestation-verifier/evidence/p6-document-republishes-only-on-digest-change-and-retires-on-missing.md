# p6-document-republishes-only-on-digest-change-and-retires-on-missing

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S5. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (`crates/daemon/src/source_capture_tests.rs:1992`), `a_missing_document_retires_only_the_captures_own_lineage` (`:2172`), `a_renamed_document_retires_the_old_path_and_publishes_the_new` (`:2237`), `unreadable_documents_are_counted_and_retire_nothing` (`:2350`), and `every_stale_lineage_of_a_path_retires_before_the_current_one_publishes` (`:3009`) cover each clause. The specification's lines were 28 lines earlier.

## Failure scenario

An unreadable document retires its descriptor and every decision citing it turns unverifiable.

## Timing windows and dependencies

A document edited or removed between passes.

## What a test must construct

Each document state across two passes.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.
