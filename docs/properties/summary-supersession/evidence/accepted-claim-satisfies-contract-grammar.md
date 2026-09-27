# accepted-claim-satisfies-contract-grammar

## Discovery trigger

Specification #835 Property Catalog record
`accepted-claim-satisfies-contract-grammar`, derived at `265df096`, with its
authority in D-4: `check_claim_set` checks key grammar, value bound,
citation and value substring, anchor, then last-wins dedup, the cap of 8,
and `idx` as array position. D-2 fixes the grammar and bounds. Milestone
ticket #838 names the faults: uppercase and dotless keys, a 129-byte value
after unescaping, a value not in its cited span, and duplicate keys.

## Evidence trail

`crates/daemon/src/history_summarizer_citations.rs::is_claim_key` accepts a
key of at most `CLAIM_KEY_MAX_BYTES` that contains a dot and whose
dot-separated parts are non-empty runs of `[a-z0-9_-]`, which is the D-2
grammar.

`crates/daemon/src/history_summarizer_citations.rs::check_claim_set` drops a
candidate, in order, for a bad key or a value over `CLAIM_VALUE_MAX_BYTES`;
a cite that does not split into exactly one citation with no trailing text;
an alias the frozen table does not resolve; a cited range that is not a span
of the presented bytes containing the value; and an alias ordinal outside
every accepted segment. It keeps the anchor only when it is non-empty,
within `CLAIM_ANCHOR_MAX_BYTES`, and contained in the segment's trimmed
`p1`. A later claim for a key removes the earlier one, then each segment is
truncated to `CLAIMS_PER_SEGMENT`. The position in the vector is the index.

`crates/daemon/src/history_summarizer_validate.rs::parse_claims` unescapes
each child element before the checks, so the bounds count unescaped bytes.

After acceptance, the store drops a claim whose key the secret scanner
rewrites (`crates/memory-store/src/lib.rs::prepare_claims`, commit
`0231f2f4`), so a stored key is never a redacted placeholder. The
rewritten-key case in
`crates/memory-store/tests/production_redaction.rs::history_segment_content_redacts_and_new_message_identities_reject`
exercises this.

Tests: `crates/daemon/src/history_summarizer_citations.rs::tests::each_claim_rule_drops_only_its_own_claim`
checks eleven candidates and asserts five kept with the exact anchors and
`Accepted { kept: 5, dropped: 6, anchor_missing: 3 }`.
`tests::a_segment_keeps_the_last_claim_per_key_then_the_first_eight` asserts
keys `k.1` through `k.8`, that the later `k.8` wins, and counts
`kept: 8, dropped: 3`. `tests::cite_acceptance_is_value_inside_the_cited_span`
is a proptest asserting acceptance equals
`presented.get(start..end).is_some_and(|span| span.contains(value))`.
`crates/daemon/src/history_summarizer_citations_golden.rs::claims_attach_to_the_accepted_segment_their_cite_names`
checks attachment by cite ordinal, an escaped value, a retraction, the
memory-disabled path, and a cite past the accepted segments.
`crates/daemon/src/history_summarizer_validate.rs::tests::the_value_bound_counts_unescaped_bytes`
keeps a 128-byte unescaped value and drops a 129-byte one.
`tests::the_provisional_last_segment_carries_its_claims` shows a claim citing
a discarded segment leaves with it and returns when the segment is
force-kept.

## Failure scenario

The model emits `postgres.port = 5433` citing a message that says 5432, or
cites a message whose whole text contains the value but whose cited span
does not. A whole-message containment check would keep it, and M2 would
render the invented value as the correction.

## Timing windows and dependencies

None. The check is pure over the candidates, the frozen alias table, and the
accepted segment ranges after discard-last.

## What a test must construct

One candidate per rule, each violating only that rule, in one call, so a
kept set that differs names the broken rule. A value lying inside the
presented text but outside the cited range. Duplicate keys and more than
eight distinct keys in one segment. A value whose escaped form exceeds 128
bytes but whose unescaped form does not.

## Investigation log

### Q: Does the implemented order match D-4?

- Sources examined: `check_claim_set`, D-4 in `/tmp/goal/835.md`.
- Findings: the order is key, value bound, citation and span, segment
  membership, anchor, dedup, cap, index. D-4 lists segment membership inside
  the citation rule; the code checks it after the span, which yields the
  same kept set because both drop the claim.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the element shape match the D-2 contract?

- Sources examined: D-2, `parse_claims`, the `## Claims` section of
  `crates/daemon/testdata/history_summarizer-system-prompt.txt`.
- Findings: D-2 writes the key as an attribute, `<claim key="...">`. The
  implemented prompt and parser both use a `<key>` child element. They
  agree with each other, so the model is judged by the rule it is given.
- Missing evidence: no recorded decision amending D-2 was found.
- Conclusion: unresolved, needs the spec text updated or the difference
  recorded as intended.

### Q: Do the named tests pass at HEAD?

- Sources examined: nextest run at `c38af85a` filtering the six tests.
- Findings: all pass.
  A mutation run replaced the span check in `check_claim_set` with
  `frozen.presented.contains(value)`. With that change
  `cite_acceptance_is_value_inside_the_cited_span` failed, and the file was
  restored.
- Missing evidence: none.
- Conclusion: resolved with answer: the tests pass, and the proptest fails
  against a whole-message containment check.
