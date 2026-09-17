# packing-optional-bounds-refuse-at-limit-plus-one

## Discovery trigger

RP2.8 acceptance row AC8 requires each bound to saturate at its approved value
and refuse at value plus one with a typed reason; the U3 ticket assigns the
fused-candidates, parents, spans-per-parent, payload-loads, payload-bytes, and
per-item-maximum bounds to the optional phase.

## Evidence trail

- `crates/retrieval/src/packing/scan.rs` `admit_optional_set` checks every
  bound in row order before any byte is loaded and returns `BoundExceeded`
  with the bound and the crossing position.
- `crates/daemon/src/packing/mod.rs` `prepare_optional` lowers a caller
  fused-candidates bound above `MAX_ELIGIBILITY_CANDIDATES` to that cap and
  checks the request count against it before the read hold, then calls
  `admit_optional_set` after eligibility and before the load hold, mapping
  each refusal to `PreparationRefusal::OptionalBound`.
- `crates/retrieval/tests/packing_grouping.rs` admits a four-span set at the
  exact limits and refuses each bound tightened by one; the daemon tests show
  the refusal leaving `payload_loads()` at the required count and a set one
  past the kernel cap refused with `FusedCandidates` at the cap before any
  optional event.

## Failure scenario

An unbounded optional set loads every candidate payload before the budget
scan and exhausts memory or the deadline.

## Timing windows and dependencies

None.

## What a test must construct

- A set sized exactly to every bound, then each bound reduced by one alone.
- A request count one past the kernel batch cap under a caller bound above
  it, with no rows persisted, so the refusal is attributable to the cap and
  not to a read.

## Investigation log

### Q: Which bounds are checked before any read, and which before any load?

- Sources examined: `crates/daemon/src/packing.rs` `prepare_optional`:
  `admit_fused_candidates` on the request count before the duplicate filter
  and the read hold; `admit_optional_set` on the rows that survived exclusion
  after judgment and before the load hold;
  `crates/retrieval/src/packing/scan.rs` for the order of the five checks
  inside `admit_optional_set`.
- Findings: the fused-candidates bound sees the raw request list, so
  duplicates count toward it; the other five see live rows only, so an
  excluded row never crosses a parent, span, load, or byte bound.
- Missing evidence: none.
- Conclusion: resolved with answer - one bound before reads, five before
  loads, as the guarantee states.

### Q: Why is the kernel batch cap folded into the fused-candidates bound?

- Sources examined: `crates/kernel/src/eligibility.rs` `check_bounds`, which
  refuses more than `MAX_ELIGIBILITY_CANDIDATES` with
  `KernelError::InvalidInput` after the rows are read; `prepare_required`,
  which lowers its payload-loads bound to the same cap before reading; review
  at the U3 change, which showed a caller bound above the cap admitting a set
  the kernel then refused after every row was read.
- Findings: lowering the caller bound to the cap refuses the set as
  `FusedCandidates` at the cap before any read;
  `a_fused_set_beyond_the_kernel_batch_cap_is_refused_before_any_read` shows
  the refusal with no optional event traced.
- Missing evidence: none.
- Conclusion: resolved with answer - the effective bound is the smaller of the
  caller's and the kernel's, mirroring the required phase.

### Q: Are the numeric limits approved?

- Sources examined: RP2.8 #629's ownership statement ("RP2.9 owns approved
  numeric limits and evidence"); the `OptionalBounds` fixture values in both
  test files.
- Findings: every bound is caller-supplied with no default; the tests use
  fixture values and the kernel cap is the only production constant involved.
- Missing evidence: the approved values.
- Conclusion: needs human input - RP2.9 owns the numbers; this record claims
  the limit-plus-one shape only.
