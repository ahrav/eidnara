# p2-new-citation-block-target-rule

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation. OQ2-3 and OQ2-13 resolved by the U3 design tournament: digest-located block runs. Owner: the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

The job text is the `\n`-join of the message's text blocks (`crates/daemon/src/memory_capture.rs:315`), and each descriptor's artifact bytes equal one block's text. U3 locates blocks by digest and cites `block`, `span`, or `first`.

## Failure scenario

A citation to the first block while another block contains the quote.

## Timing windows and dependencies

None.

## What a test must construct

A two-block message with the quote in the second block; a quote spanning the join; a quote with a redaction placeholder.

## Investigation log

### Q: Does code for this obligation exist at the branch tip?

- Sources examined: the surfaces named in the trail, read at the branch that adds this part.
- Findings: No. The record stays `proposed` until its owner lands the behavior and the check.
- Missing evidence: the implementation.
- Conclusion: unresolved, needs the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).
