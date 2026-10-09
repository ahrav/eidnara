# cf-tier-transitions-preserve-qualified-meaning

## Discovery trigger

The supplied plan's R2/R6 and tier table require action-changing qualifiers to
survive serving, not merely generation. A correct P1 can become an unsafe P4
heading. This is a contract claim; no semantic failure was executed here.
System lenses: architecture, state, safety, product. Property lenses: data
integrity, lifecycle, compatibility. Inspected 2026-09-19 at
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8` in `ahrav/eidnara`.

## Evidence trail

- [history_summarizer_validate.rs:320-341](../../../../../crates/daemon/src/history_summarizer_validate.rs#L320-L341)
  fills absent P2 with P1, absent P3 with P2 or P1, and absent P4 with empty text.
  Retained producer XML is needed to distinguish authored from repaired tiers.
- [history_summarizer_validate.rs:1656-1677](../../../../../crates/daemon/src/history_summarizer_validate.rs#L1656-L1677)
  checks P1-only acceptance and these exact fallback values; status unaudited.
- [m1_compose.rs:202-207](../../../../../crates/daemon/src/m1_compose.rs#L202-L207)
  selects rows beyond `folded_history_segment_seq`.
  [memory_render.rs:233-248](../../../../../crates/daemon/src/memory_render.rs#L233-L248)
  renders those new rows at explicit P1 without an age-based choice.
- [m0_compose.rs:183-233](../../../../../crates/daemon/src/m0_compose.rs#L183-L233)
  reloads durable history and composes the decayed baseline.
- [decay_render.rs:355-374,413-440](../../../../../crates/daemon/src/decay_render.rs#L355-L440)
  distinguishes missing bodies from present empty bodies. Missing requested
  tiers fall back toward denser populated tiers; empty bodies render headings.
- [decay_render.rs:385-403,419-427](../../../../../crates/daemon/src/decay_render.rs#L385-L427)
  uses flat content for legacy/non-tiered rows, with 1,200/420-character
  truncation at P2/P3 and heading-only output at P4.
- [transform.rs:21289-21394](../../../../../crates/daemon/src/transform.rs#L21289-L21394)
  checks seeded later publication, P1 m1 placement, covered-tail removal, and
  warm m0/m1 replay; status unaudited. It does not execute the producer.

Reachability is **default-production**: compaction defaults enabled in
[config.rs:139-157](../../../../../crates/daemon/src/config.rs#L139-L157), and
the composition functions above are ordinary serving code. Stored legacy input
is a compatibility variant, not a claim that fresh tierless output validates.

## Failure scenario

A case says pooling-first was rejected because redesign replaces it, and that
redesign is planned. P1 records both facts. A later heading says only
“Pooling-first redesign,” or a sparser body says redesign completed. Structural
rendering succeeds while the next action becomes unsupported.

Another false result labels repaired P3 as an authored compact paraphrase. The
provider receives copied P1, so meaning may survive but the claimed tier and
compression result are wrong. Legacy truncation can lose a final negation too.
These are distinct failures: semantic loss versus mislabeled representation.

## Timing windows and dependencies

Track initial m1, later m0, and warm reuse for the same accepted source range.
The first publication after an empty bootstrap need not use m1. A baseline
followed by later publication is the constructible m1 setup.
Raw tail, memory, and hints can hide a capsule failure; record them separately.
Only P4's stated omission exception can substitute a whole-invocation
disposition for its capsule obligation. Consume
[cf-unavailable-evidence-no-credit](../../recovery/catalog.md#cf-unavailable-evidence-no-credit)
for that decision, not a second validator. Unavailable gives no useful retention
or recovery credit. A scenario that explicitly permits unavailable evidence and
safe abstention may still be accepted; this is not a claim that P4 retained the
meaning. Do not weaken P1-P3 obligations or excuse lost required retention.
P4's exception never permits a stronger or contradictory title/body assertion.

## What a test must construct

1. Use source-backed obligations defined before candidate output. Give reviewed
   P1-P4 bodies distinctive text so deterministic checks identify what arrived.
2. Retain authored tier presence, parsed/healed values, accepted publication,
   selected tier, and effective body source. Do not derive expected selections
   by calling the renderer under test.
3. Reach m1 and P1-P4 m0 through composition and qualified provider captures;
   remove the accepted raw range from the live tail before compression credit.
4. Include missing P2/P3, empty P4, and legacy/non-tiered stored controls.
5. Review subject, scope, polarity, final status, uncertainty, material rationale,
   and evidence distinction on heading plus body. Accept valid paraphrases.

The companion situation markers assert publication and independently chosen
serving inputs, not a lost qualifier. All exercise remains **not yet**.

## Investigation log

### Q: Does missing P3 prove that P3 was authored or served sparsely?
- Sources examined: parser, renderer, and P1-only parser test cited above.
- Findings: absent tiers are copied during parsing; a provider capture cannot
  reconstruct their authored presence. Empty P4 is different from missing P3.
- Missing evidence: no combined provenance observation exists for the corpus.
- Conclusion: resolved with answer: retain all three representation stages.

### Q: Which P4 capsules and source obligations are acceptable?
- Sources examined: supplied plan R2, minimum-tier table, and C1-C6 examples.
- Findings: examples define materiality and forbidden transitions, not an
  approved annotation for every source/follow-up pair.
- Missing evidence: independent human approval of those annotations.
- Conclusion: needs human input. No string-matching semantic oracle is implied.
