# claims-constants-match-prompt-fixture

## Discovery trigger

Specification #835 Property Catalog record
`claims-constants-match-prompt-fixture`, derived at `265df096`, from D-2:
`CLAIMS_PER_SEGMENT = 8` sits beside the facts caps, and one test asserts
the prompt fixture states the same numbers. Milestone ticket #838 carries it
as the acceptance criterion that the fixture states 8 claims, a 64-byte key,
a 128-byte value, and a 200-byte anchor, compared by a test to the
constants.

## Evidence trail

`crates/daemon/src/history_summarizer_citations.rs` defines
`CLAIMS_PER_SEGMENT` (8), `CLAIM_KEY_MAX_BYTES` (64), `CLAIM_VALUE_MAX_BYTES`
(128), and `CLAIM_ANCHOR_MAX_BYTES` (200). `check_claim_set` and
`is_claim_key` read these constants and no literal copies of the numbers.

`crates/daemon/src/history_summarizer_prompt.rs::HISTORY_SUMMARIZER_SYSTEM_PROMPT`
is `include_str!("../testdata/history_summarizer-system-prompt.txt")`, so the
test reads the bytes the daemon sends, not a second copy.

`crates/daemon/src/history_summarizer_citations.rs::tests::claims_constants_match_the_prompt_fixture`
slices the prompt from its `## Claims` heading and finds each rule by line
prefix. It asserts the `` - `<key>`: `` line contains
`at most {CLAIM_KEY_MAX_BYTES} bytes`, the `` - `<value>`: `` line contains
`at most {CLAIM_VALUE_MAX_BYTES} bytes`, the `` - `<anchor>`: `` line
contains `at most {CLAIM_ANCHOR_MAX_BYTES} bytes`, and the `- At most` line
starts with `- At most {CLAIMS_PER_SEGMENT} claims per history_segment.`.
The fixture has one `## Claims` heading, and each rule line states the
matching number.

## Failure scenario

A maintainer raises `CLAIM_VALUE_MAX_BYTES` to 256 and leaves the prompt at
128. The model keeps values short for no reason. The reverse, a prompt that
promises 256 while the validator keeps 128, drops every long value the model
was told it could emit, and the drop shows only as `dropped` counts in the
diagnostic line.

## Timing windows and dependencies

None. Both sides are compile-time constants.

## What a test must construct

The compiled prompt text and the four constants. The test must locate each
rule by a stable prefix rather than a line number, so prose edits elsewhere
in the prompt do not break it, and must fail when either side changes alone.

## Investigation log

### Q: Does the test read the prompt the daemon sends?

- Sources examined: `history_summarizer_prompt.rs` lines 91 and 92, the
  test body.
- Findings: the constant is an `include_str!` of the fixture and the test
  reads the constant.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Can the substring match accept a wrong number?

- Sources examined: the test's `contains` calls.
- Findings: the match includes the words `at most` before the number and
  `bytes` after it, so a number with extra leading or trailing digits does
  not match. The test does not check that the prompt states no second,
  contradicting number for the same rule elsewhere, and it does not check
  the `<cite>` rule's "exactly one citation", which has no constant.
- Missing evidence: none needed for the four stated limits.
- Conclusion: resolved with answer; the check covers the four constants
  only.

### Q: Does the test pass at HEAD?

- Sources examined: nextest run at `c38af85a`.
- Findings: `claims_constants_match_the_prompt_fixture` passes.
- Missing evidence: none.
- Conclusion: resolved with answer.
