# dense-rejected-leaders-never-starve-eligible-rows

## Discovery trigger

RP2.6 R2 and #610 AC2: ineligible rows cannot consume accepted capacity or
stop discovery of lower eligible rows.

## Evidence trail

- `Progress::judge_batch` (`crates/retrieval/src/dense/oracle.rs:1237`)
  offers only rows the kernel judged eligible.
- `Progress::judge_ranked` (`oracle.rs:1020`) judges the scored rows best
  first and stops once the set holds `R` eligible rows, or the rows run out.
  Every judged row ranks ahead of every unjudged row, so a full set admits
  none of the rest.
- `Unjudged::draw` (`oracle.rs:1385`) hands out the unjudged rows in rank
  order. A batch is sized by the admission rate, so the batch that fills the
  set can also judge rows ranked below its last member: with one page of
  forty, the second batch judges all 36 remaining rows, sixteen of them
  eligible rows below the pool.

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

### Q: Are the rows judged exactly the pool and the rejected rows above it?

- Sources examined: `Progress::judge_ranked`, `Admissions::ranked_batch`,
  and the forty-row page case of
  `a_rejected_prefix_longer_than_the_pool_and_the_batch_does_not_starve_the_eligible_suffix`.
- Findings: no. After a batch that admits nothing, `ranked_batch` asks for
  every remaining row up to the page, so the batch that fills the set can
  judge rows ranked below its last member. With pages of four the scan
  judges exactly 24 rows in six batches; with one page of forty it judges
  all 40 rows in two batches.
- Missing evidence: none.
- Conclusion: resolved with answer - no; a pool that fills was judged over
  itself, every rejected row above its last member, and the rest of the
  batch that filled it, fewer than `page_rows` extra rows.
