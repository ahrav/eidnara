# dense-stage-evidence-keeps-coverage-and-recall-apart

## Discovery trigger

RP2.6 R4 and #613 AC6: retain separate candidate-coverage and rescored
Recall@10 fields plus stage identity sets for each alpha.

## Evidence trail

- `CompressedRanking` returns `pool` (quantized identities and scores) and
  `rescored` apart.
- The sweep test builds one baseline from the eligible f32 reference and one
  record per alpha.

## Failure scenario

Reporting one number for both stages hides a pool miss behind a rescore
claim.

## Timing windows and dependencies

None.

## What a test must construct

- A corpus with unadmitted rows and every approved alpha.

## Investigation log

### Q: Must the two measures differ?

- Sources examined: #578 Further Notes.
- Findings: with exact rescore and `K >= 10` they can be necessarily equal;
  they stay separate fields.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
