# p2-new-citation-block-target-rule

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N3, restated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) for the digest-located rule (ADR-P2-001, OQ2-3 and OQ2-13).

## Evidence trail

`locate` (`crates/daemon/src/memory_capture/cite.rs:198`) places each live block after the previous block's separator, spanning the descriptor's `byte_length`; the block is located when that range ends at a `\n` or at the end of the job text and its SHA-256 equals the block's `artifact_digest`, and the walk stops at the first block it cannot locate. A segment whose digest equals an artifact digest has that artifact's length, so this locates the same blocks as a SHA-256 finalized at every `\n` (`locating_by_length_matches_the_newline_walk`) and hashes each text byte at most once. The job text is the `\n`-join of the message's text items (`native_capture_fragments`), and each descriptor's bytes are one item's text, so an unredacted message locates every block. `target` (`:230`) takes the first occurrence of the quote: inside one located block gives `block`; across located adjacent blocks gives `span`, where the separator after a located block opens a quote in that block and the separator before one closes a quote in it, so the cited blocks joined with `\n` contain the quote; anything else (a quote in an unlocated block, or absent) gives `first`, the lowest live block. Each block's digest is decoded to bytes once and compared raw. `Citation::cite` (`:349`) writes `citation_block`, `citation_blocks`, and `citation_evidence` in ascending order, and `append_citations` (`:379`) writes one event per cited block in the same order.

## Failure scenario

Citing the first block while a later block holds the quote makes revalidation report a false mismatch.

## Timing windows and dependencies

None.

## What a test must construct

The end-to-end fixtures at `crates/daemon/src/source_capture_tests.rs:3496`: one block; the third of four items; a quote across two items; a quote starting at a separator; a message whose first block holds a secret, which source capture refuses, so that block has no descriptor. Unit tests at `crates/daemon/src/memory_capture/cite.rs:472` and `:502` cover inner newlines, an empty block, a stopped walk, and a partial located prefix; the property tests at `:918` and `:932` locate every block of random joins and check that a random quote of a fully located message cites blocks whose join holds it.

## Investigation log

### Q: Which rule covers a quote that touches a separator?

- Sources examined: the P6 verifier contract in #1135 and `target`.
- Findings: the verifier joins the cited blocks with `\n` and tests the quote
  as a substring. Citing only one side of a touched separator fails that
  test, so a separator joins the blocks on both sides into a `span`.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the walk stay within the 500 ms criterion?

- Sources examined: `citation_cost_measurement` (`crates/daemon/src/memory_capture.rs:1841`) on the CI profile.
- Findings: each run times 20 publishes (the project's first publish,
  which also creates its domain and scope, is untimed) and 20 walks. Runs
  alternated this code with `f6f08f1ca`, whose walk finalized a SHA-256 at
  every `\n` and whose lookup issued one statement per index, on one host
  in two rounds of four runs. The publish closure of a 3-memory job reading
  all 256 indexes measured medians of 6.2 to 6.5 ms and maxima of 13 to
  16 ms in the quieter round (host load average 22), against 9.2 to 18.2 ms
  and 15 to 36 ms at `f6f08f1ca`; at load average 27, both measured medians
  of 57 to 134 ms and maxima of up to 267 ms. The walk over 64 Ki newlines
  against one block that spans the text and never matches measured medians
  of 45 µs and maxima of 106 to 112 µs, against 157 to 158 ms. The worst
  sum of the maxima is 267 ms, under the 500 ms criterion.
- Missing evidence: none.
- Conclusion: resolved with answer: bound 256; blocks are placed by their
  descriptors' byte lengths.
