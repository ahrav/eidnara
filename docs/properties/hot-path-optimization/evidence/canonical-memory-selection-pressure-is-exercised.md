# canonical-memory-selection-pressure-is-exercised

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Ordinary small canonical-memory examples cannot distinguish pre-cap filtering
from a premature row limit. Row and byte pressure need separate witnesses.

## Evidence trail

- [read.rs:26-30][caps] defines 8192 rows and one eighth of the wire-body limit
  as the read-byte budget.
- [read.rs:163-169][heap] drops the serving-order-last retained row on overflow.
- [read.rs:479-526][cutoff] filters before heap admission and then applies the
  cumulative payload cutoff to the retained newest-first rows.
- [kernel_routes.rs:1958][row-test] and [2112][byte-test] contain generic cap
  tests. Their existence is not this campaign's pressure evidence. Their
  over-cap and over-budget reads carry no `object_ids`, so `newest_page`
  serves them and neither puts pressure on the heap or the cutoff above.

## Failure scenario

A candidate reader with the wrong limit placement passes every comparison
because no fixture ever exceeds the limit or puts useful rows behind noise.
An output-based marker is also wrong: correct exclusion may avoid truncation.

## Timing windows and dependencies

The pressure witness is derived from seeded input, not the candidate's counters.
Other-domain rows must be admitted candidates if the test claims they pressure
the query result; merely filling object_registry with unadmitted rows is weak.
The production read is reachable under default memory settings; no feature
switch is needed to construct a sufficiently large project.

## What a test must construct

Seed more than 8192 admitted visible candidates and place an eligible memory
target beyond rank 8192 in unfiltered serving order: created_commit_seq
descending, then object_id ascending. Legitimate pre-cap exclusion must put
that target inside the eligible cap. Keep the entire raw stored payload total
below 8 MiB so byte limiting cannot explain a missing target. Lexical SQL order
is not the oracle's serving rank; a lexical adversary may be an additional case.
Use independent seed counts, stored lengths, and eligibility. The constant
marker is this record's slug and never demands correct output truncation.
This record remains unexercised and the existing checks are unaudited.

## Investigation log

### Q: Can byte pressure substitute for this row-pressure witness?

- Sources examined: [The distinct caps][caps] and [payload cutoff][cutoff].
- Findings: The independent review identifies a gap in the disjunctive marker:
  byte pressure alone leaves premature row limiting unchallenged.
- Missing evidence: No independent row-pressure witness is recorded or run.
- Conclusion: Resolved. This record requires row pressure, while
  [K4][byte-record] separately requires byte pressure. Both are campaign gates.

[caps]: ../../../../crates/daemon/src/kernel_routes/read.rs#L26-L30
[heap]: ../../../../crates/daemon/src/kernel_routes/read.rs#L163-L169
[cutoff]: ../../../../crates/daemon/src/kernel_routes/read.rs#L479-L526
[row-test]: ../../../../crates/daemon/tests/kernel_routes.rs#L1958
[byte-test]: ../../../../crates/daemon/tests/kernel_routes.rs#L2112
[byte-record]: ../catalog.md#canonical-memory-byte-pressure-is-exercised
