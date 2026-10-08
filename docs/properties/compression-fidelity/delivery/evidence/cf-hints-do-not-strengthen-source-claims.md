# cf-hints-do-not-strengthen-source-claims

## Discovery trigger

R2/R3/R6 apply to every visible capsule, including a hint resurrecting omitted
history. Failure, product, data-integrity, resource, and wildcard lenses identify
qualifier loss and unsupported recovery language as separate claim leads.
Inspected 2026-09-19 at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.

## Evidence trail

- [transform.rs:109-114](../../../../../crates/daemon/src/transform.rs#L109-L114)
  sets 100 candidates, 24 lexical tokens, three results, two matched tokens,
  an 80-unit fragment cap, and an 800-unit total cap.
- [transform.rs:8190-8237](../../../../../crates/daemon/src/transform.rs#L8190-L8237)
  requires an eligible new authored tail, no prior decision/frontier exclusion,
  no stacked augmentation, and sufficient sanitized prompt length.
- [transform.rs:8286-8315](../../../../../crates/daemon/src/transform.rs#L8286-L8315)
  concatenates title, flat content, and all populated tiers from stored history.
  It does not restrict selection to the body actually rendered in m0.
- [transform.rs:8339-8392](../../../../../crates/daemon/src/transform.rs#L8339-L8392)
  requires two matched terms and one appearing in fewer than half the candidate
  rows. Only the top score is compared with the configured threshold.
- [transform.rs:8609-8634,8648-8657](../../../../../crates/daemon/src/transform.rs#L8609-L8657)
  applies Ultra compression, normalizes whitespace, and truncates fragments in
  UTF-16 units. It also drops formatted lines of at most two bytes: an empty
  fragment produces only `- `. Nonempty one- or two-character fragments survive
  this filter. If every line drops, the whole hint is absent. The footer tells the agent
  it may run `eidnara_search` to search project memory for the fragments'
  topic; #926 replaced the earlier “retrieve full context” wording.
- [terse_text_compression.rs:41-72,848-856](../../../../../crates/daemon/src/terse_text_compression.rs#L41-L72)
  lists filler/hedging phrases that the
  [compression pass](../../../../../crates/daemon/src/terse_text_compression.rs#L848-L856)
  removes. Filler-only selected content is a construction lead for total drop,
  not evidence of a material semantic failure.
- [transform.rs:22965-22984](../../../../../crates/daemon/src/transform.rs#L22965-L22984)
  asserts one lexical query across repeated empty decisions; status unaudited.
- [opencode-transform-adapter.ts:477](../../../../../packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts#L477)
  defaults auto-search enabled. The daemon
  [gate](../../../../../crates/daemon/src/transform.rs#L3510-L3511) excludes
  subagent requests. Reachability is **default-production** for eligible main
  requests; the disabled variant needs explicit configuration.

## Failure scenario

A stored segment names an option first and rejects it later. It reaches P5,
but its concatenated text matches a new user request. The hint's 80-unit prefix
keeps the option while dropping rejection or planned status. A misleading
summary repeated in several tiers can also dominate the fragment's prefix.
Repetition is not independent evidence of acceptance.

The existing footer makes a recovery promise. Whether a registered tool can
fulfill it must come from the separate capability audit. The presence of a
history-backed hint does not by itself prove that the tool retrieves history.
No observed semantic incident or successful recovery is claimed here.

## Timing windows and dependencies

Hint decisions can freeze before later requests; a new test must use a new
eligible tail rather than reuse an already empty decision. Singleton candidate
fixtures cannot satisfy the rarity condition. Use genuine distractors and the
existing gate instead of weakening thresholds.
Cross-tier concatenation occurs before compression and truncation, so retain
the selected source material as well as the delivered fragment. A provider
capture alone does not identify which tier supplied a discarded qualifier.

## What a test must construct

1. Publish an omitted case row plus distractors sufficient for the rarity gate.
   Use a source with a material qualifier near the fragment cutoff.
2. Confirm the compressed normalized fragment exceeds 80 UTF-16 units and the
   ranking/tail preconditions hold. Do not assert that qualifier loss occurred
   as a situation marker; that belongs to the independent semantic check.
3. Capture the hint-bearing request after accepted-range replacement, and a
   matched hints-disabled request. Separately construct a selected snippet that
   normalizes to empty after compression; capture the request even if no hint
   remains. Keep that drop outcome separate from the long-fragment outcome.
4. Exclude decisive raw-tail duplicates and review the whole invocation against
   the source. Consume the recovery-owned canonical disposition for any lost
   obligation rather than crediting hidden source bytes.
5. Consume capability-owner evidence for any discovery/recovery claim, using
   only visible information and the declared call/output budget.

The current fixed wrapper and three 80-unit fragments total at most 470 UTF-16
units, computed from the source strings, below 800. The old inventory's 458 is
stale. Do not add a production situation demanding the total cap fire.
All exercise remains **not yet**.

## Investigation log

### Q: Is a missing hint evidence that truncation or preservation is safe?
- Sources examined: tail gate, ranking, persisted empty-decision test.
- Findings: absent hints can result from a reused decision, insufficient
  matched terms, no discriminating candidate, or total post-compression fragment
  drop. These require separate observations; one absent hint proves none alone.
- Missing evidence: a qualifying omitted-row hint in the final request.
- Conclusion: resolved with answer: require independent selection preconditions.

### Q: Which truncated capsule remains semantically safe?
- Sources examined: plan C1/C4, hint compression and formatting functions.
- Findings: size and UTF-16 checks do not judge polarity, status, or evidence.
- Missing evidence: reviewed source/candidate pairs and capability observations.
- Conclusion: needs human input for semantic judgments; recovery semantics stay
  with their separate owner. Do not increase production hint work or budgets.
