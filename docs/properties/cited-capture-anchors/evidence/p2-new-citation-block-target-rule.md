# p2-new-citation-block-target-rule

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N3, restated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) for the digest-located rule (ADR-P2-001, OQ2-3 and OQ2-13).

## Evidence trail

`locate` (`crates/daemon/src/memory_capture/cite.rs:125`) runs one SHA-256 over the job text, finalizing a clone at each `\n` and at the end; a segment whose lowercase hex digest equals the next live block's `artifact_digest` locates that block, and the walk stops at the first block it cannot locate. The job text is the `\n`-join of the message's text items (`native_capture_fragments`), and each descriptor's bytes are one item's text, so an unredacted message locates every block. `target` (`:150`) takes the first occurrence of the quote: inside one located block gives `block`, across located adjacent blocks gives `span`, anything else (a quote starting at a separator, in an unlocated block, or absent) gives `first`, the lowest live block. `Citation::cite` (`:216`) writes `citation_block`, `citation_blocks`, and `citation_evidence` in ascending order, and `append_citations` (`:246`) writes one event per cited block in the same order.

## Failure scenario

Citing the first block while a later block holds the quote makes revalidation report a false mismatch.

## Timing windows and dependencies

None.

## What a test must construct

The end-to-end fixtures at `crates/daemon/src/source_capture_tests.rs:3466`: one block; the third of four items; a quote across two items; a quote starting at a separator. Unit tests at `crates/daemon/src/memory_capture/cite.rs:309` and `:320` cover inner newlines, an empty block, a stopped walk, and a partial located prefix; the property test at `:365` locates every block of random joins.

## Investigation log

### Q: Does the walk stay within the 500 ms criterion?

- Sources examined: `citation_cost_measurement` (`crates/daemon/src/memory_capture.rs:1744`) on the CI profile.
- Findings: The whole publish closure of a 3-memory job with 256 probes measured a median of 20 ms and a maximum of 89 ms; the digest walk over 64 Ki newlines with a block it never locates measured a median of 193 ms and a maximum of 197 ms. The sum stays under 500 ms with a 256-probe bound.
- Missing evidence: none.
- Conclusion: resolved with answer: bound 256, finalize at every `\n`.
