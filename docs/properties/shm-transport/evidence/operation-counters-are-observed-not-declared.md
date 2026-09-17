# operation-counters-are-observed-not-declared

Status: invalidated. The benchmark cleanup removed the subject of this record.
No current gate-counter provenance or exercised-check claim is retained.

## Discovery trigger

The former hardware-envelope benchmark reported operation counters to a
selection gate. Discovery asked whether those counters observed operations
or merely repeated arm labels, flags, and iteration counts.

## Evidence trail

The cleanup removed `crates/shm-transport/src/evidence.rs`,
`crates/shm-transport/tests/evidence.rs`, the hardware-envelope benchmark, and
its manifest. The `OperationCounters` gate and its three arithmetic tests no
longer exist. They cannot supply a current release or instrumentation verdict.

The [pre-cleanup investigation](https://github.com/ahrav/eidnara/blob/705899ad28341043c163d2a137b076acbbc707e2/docs/properties/shm-transport/evidence/operation-counters-are-observed-not-declared.md)
remains available in version history. Its source offsets and successive
"At HEAD" notes refer to those historical trees, not this checkout.

## Failure scenario

A future gate could report zero operations despite performing copies or
allocations. This is a conditional measurement risk, not an observed defect
in the retained transport.

## Timing windows and dependencies

Reactivation requires a benchmark, an identified counter owner at each
operation site, and retained evidence. No current campaign is pending.

## What a test must construct

Any replacement must remove a real operation without changing its arm label
and assert that the observed counter changes. Reintroducing a report schema
alone would not establish instrumentation validity.

## Investigation log

### Q: Does this record still identify an executable gate?

- Sources examined: The cleanup diff and current tracked-file inventory.
- Findings: The gate, benchmark, manifest, and gate tests are removed.
- Missing evidence: No replacement gate or provenance witness is retained.
- Conclusion: invalidated. Retained transport checks do not inherit this
  benchmark's gate-counter claim.
