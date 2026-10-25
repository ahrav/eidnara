# p2-new-citation-block-target-rule

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N3, restated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) for the digest-located rule (ADR-P2-001, OQ2-3 and OQ2-13).

## Evidence trail

`locate` (`crates/daemon/src/memory_capture/cite.rs:196`) runs one SHA-256 over the job text, finalizing a clone at each `\n` and at the end; a segment whose lowercase hex digest equals the next live block's `artifact_digest` locates that block, and the walk stops at the first block it cannot locate. The job text is the `\n`-join of the message's text items (`native_capture_fragments`), and each descriptor's bytes are one item's text, so an unredacted message locates every block. `target` (`:226`) takes the first occurrence of the quote: inside one located block gives `block`; across located adjacent blocks gives `span`, where the separator after a located block opens a quote in that block and the separator before one closes a quote in it, so the cited blocks joined with `\n` contain the quote; anything else (a quote in an unlocated block, or absent) gives `first`, the lowest live block. Each block's digest is decoded to bytes once, so the walk compares raw digests at each `\n`. `Citation::cite` (`:308`) writes `citation_block`, `citation_blocks`, and `citation_evidence` in ascending order, and `append_citations` (`:338`) writes one event per cited block in the same order.

## Failure scenario

Citing the first block while a later block holds the quote makes revalidation report a false mismatch.

## Timing windows and dependencies

None.

## What a test must construct

The end-to-end fixtures at `crates/daemon/src/source_capture_tests.rs:3496`: one block; the third of four items; a quote across two items; a quote starting at a separator; a message whose first block holds a secret, which source capture refuses, so that block has no descriptor. Unit tests at `crates/daemon/src/memory_capture/cite.rs:401` and `:412` cover inner newlines, an empty block, a stopped walk, and a partial located prefix; the property tests at `:504` and `:518` locate every block of random joins and check that a random quote of a fully located message cites blocks whose join holds it.

## Investigation log

### Q: Which rule covers a quote that touches a separator?

- Sources examined: the P6 verifier contract in #1135 and `target`.
- Findings: the verifier joins the cited blocks with `\n` and tests the quote
  as a substring. Citing only one side of a touched separator fails that
  test, so a separator joins the blocks on both sides into a `span`.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the walk stay within the 500 ms criterion?

- Sources examined: `citation_cost_measurement` (`crates/daemon/src/memory_capture.rs:1827`) on the CI profile.
- Findings: across three runs of 20 timed publishes each (the project's
  first publish, which also creates its domain and scope, is untimed), the
  publish closure of a 3-memory job running all 256 probes measured medians
  of 12 to 96 ms and maxima of 91 to 236 ms; the digest walk over 64 Ki
  newlines against a block it never locates measured medians of 157 to
  158 ms and maxima of 158 to 160 ms. The worst sum of the maxima is 395 ms,
  under the 500 ms criterion, with a 256-probe bound and a finalize at every
  `\n`.
- Missing evidence: none.
- Conclusion: resolved with answer: bound 256, finalize at every `\n`.
