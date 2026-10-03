# dense-rejected-leaders-never-starve-eligible-rows

## Discovery trigger

RP2.6 R2 and #610 AC2: ineligible rows cannot consume accepted capacity or
stop discovery of lower eligible rows.

## Evidence trail

- `Progress::judge_batch` (`crates/retrieval/src/dense/oracle.rs:914`) offers
  only rows the kernel judged eligible.
- The walk continues until its pages run out or a bound stops it; a full set
  only raises the score a row needs to be selected.

## Failure scenario

Taking the unchecked top R and filtering afterwards returns nothing when the
R best rows are all hidden.

## Timing windows and dependencies

None.

## What a test must construct

- A hidden prefix longer than the pool and the page.

## Investigation log

### Q: Does the walk stop when the set is full?

- Sources examined: `oracle::walk`.
- Findings: no; it stops only at the end of the population or at a bound.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
