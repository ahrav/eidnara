# dense-rejected-leaders-never-starve-eligible-rows

## Discovery trigger

RP2.6 R2 and #610 AC2: ineligible rows cannot consume accepted capacity or
stop discovery of lower eligible rows.

## Evidence trail

- `Progress::judge_batch` (`crates/retrieval/src/dense/oracle.rs:1228`)
  offers only rows the kernel judged eligible.
- `Progress::judge_ranked` (`oracle.rs:1011`) judges the scored rows best
  first and stops only once the set holds `R` eligible rows and the best
  unjudged row ranks below them, or the rows run out.

## Failure scenario

Taking the unchecked top R and filtering afterwards returns nothing when the
R best rows are all hidden.

## Timing windows and dependencies

None.

## What a test must construct

- A hidden prefix longer than the pool and the page.

## Investigation log

### Q: Does the walk stop when the set is full?

- Sources examined: `oracle::walk_ranked`.
- Findings: no; the walk stops only at the end of the population or at a
  bound, and judgment stops only once the rows that rank below the full set
  are all that remain.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
