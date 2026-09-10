# rp21-export-predecode-bounds

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

The resource pass follows allocation order rather than returned vector length.
P, lines 73 and 82, rejects truncation after whole-snapshot materialization.
I, line 46, repeats the pre-materialization requirement. P, lines 118-121,
names row/decoded-byte bounds but leaves production values to RP2.9.

## Evidence trail

- `crates/kernel/src/slice/read.rs:162-173` loads whole decision and
  observation vectors before returning a snapshot.
- `crates/kernel/src/slice/read.rs:210-215` collects all selected decisions.
- `crates/kernel/src/slice/read.rs:275-295` first obtains payload `Vec<u8>`,
  then parses JSON. A check after this function cannot prevent that allocation.
- `crates/kernel/src/slice/read.rs:315-340` does the same for observations.
- `crates/kernel/src/slice/read.rs:140-158` already exposes
  `decision_payload_sizes_as_of` for a specified set of IDs.
- `crates/kernel/src/slice/read.rs:254-270` reads `length(decision_payload)`
  without decoding. This is a reuse opportunity, not an all-class pager.
- `crates/kernel/tests/kernel_slice.rs:562-609` compares a live decision's
  reported size with serialized payload bytes. Status: unaudited.
- `crates/kernel/src/outbox.rs:456-476` also collects payload bytes under a
  row-count limit alone; bounded rows do not imply bounded bytes.
- No production export page admission or decoded-allocation charge exists.
  `test-only` denotes this absent production target, not the existing size API.
- `crates/host-runtime/examples/perf_host.rs:5-33` wraps `System` with unsafe
  `GlobalAlloc`. It counts cumulative allocation requests/bytes, not live
  decoded heap; deallocation does not subtract its requested size.
- `crates/kernel/src/lib.rs:5` forbids unsafe code. There is no identified
  export-specific decoder-entry/live-heap observer within an approved boundary.

## Failure scenario

A page requests a small row count but the first row holds a very large JSON
payload. Loading then truncating the vector has already paid the allocation.
Silently skipping that row keeps memory down but violates exact export.

An encoded-byte budget alone also does not prove a decoded-heap budget.
Nested values and object/string overhead can expand into more live memory.
The catalog keeps both quantities explicit; it does not invent an expansion
factor or call encoded bytes a measurement of decoded memory.
A logical charge is a potential implementation approach, not physical-heap
proof. The perf example is a pattern to assess, not an allocator prescription
or authorization to change kernel lint policy or add an unsafe dependency.

## Timing windows and dependencies

Size observation must cover the same key, revision, and S as materialization.
A separate metadata read needs the export retention contract to prevent
source disappearance from becoming a successful omission. The page admission
order must be observable before payload copying and decoder entry.
SQLite cache, transport buffers, parsed values, and local transaction bytes
are distinct limits; this property does not claim a bound on total RSS.

## What a test must construct

1. Supply exact-cap and over-cap rows whose sizes are known independently.
2. Supply a next row that fits alone but not in the remaining page budget.
3. Observe metadata lookup, admission, materialization, and decoder entry.
4. Require explicit over-cap failure without payload allocation/decode.
5. Require the residual-budget row on a later page rather than its omission.
6. Observe live decoded high water through a permitted, independently reviewed
   boundary on JSON shapes with different expansion. This observer is missing;
   cumulative perf counters or logical charge alone cannot clear the claim.
7. Record `rp21_export_oversize_row_present` and
   `rp21_export_page_budget_edge` from fixture sizes and requested keys.

## Investigation log

### Q: Must discovery propose another payload-size helper?

- Sources examined: slice/read size API, matching size test, P current-code map.
- Findings: The decision size-query helper already exists and avoids payload
  decoding. Its ID list and returned metadata are still caller supplied.
- Missing evidence: All-class page integration and bounds before full loads.
- Conclusion: Resolved with answer: reuse existing size functionality where
  applicable; do not duplicate it or relabel it a complete exporter.

### Q: What are the byte units and approved production limits?

- Sources examined: P Bounds; I approval rule; L, lines 79-84 and 131-136.
- Findings: Numeric values are deliberately unset. Approval requires units,
  outcome/timing definitions, identities, and limits before enablement.
- Missing evidence: Row/single-row/encoded/decoded limits and decoded charge
  rules, including metadata and overlapping buffers.
- Conclusion: Needs human input through RP2.9. No numeric cap or allocation
  multiplier is inferred from existing tests or serving thresholds.

### Q: Which observation and lint boundary measures physical decoded heap?

- Sources examined: Kernel lib lint and perf-host allocator above; P Bounds.
- Findings: Kernel forbids unsafe code; the separate example uses unsafe code
  and measures cumulative allocation requests, not per-export live heap.
- Missing evidence: A permitted observer, attribution scope, deallocation/
  overlapping-buffer accounting, and RP2.9 acceptance of the measurement.
- Conclusion: Needs human input. This catalog names the missing boundary
  without prescribing an allocator, new unsafe dependency, or lint change.
