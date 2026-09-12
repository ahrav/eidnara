# synapse-local-memory-reservation-is-honest

## Discovery trigger

The local input pool originally used only the worst queued-input charge. A
completed job shrinks its charge but keeps two request-key copies plus each item
identifier and content hash. Those retained bytes can coexist with newly queued
input and therefore belong in the same local pool and host declaration.

## Evidence trail

- `crates/host-runtime/src/synapse/jobs.rs` defines the runtime input charge in
  `job_input_bytes_for_shape` and the completed-job remainder in
  `retained_input_bytes`.
- `InputShape::allocated` measures capacities owned by decoded wire strings.
- `InputShape::exact` represents the exact-capacity strings built by the
  in-process path and refuses an item identity above `MAX_ITEM_ID_BYTES`.
- `crates/host-runtime/src/synapse/bundle.rs` computes
  `max_queued_input_bytes` from the same runtime input rule.
- `max_retained_input_bytes` multiplies the runtime retained rule by
  `max_retained_jobs` with checked arithmetic.
- `validate_serving_limits` rejects overflow and a combined local-input capacity
  above the semaphore permit limit.
- `crates/host-runtime/src/synapse/mod.rs` builds the local byte budget from
  `checked_local_input_capacity`, the sum of the queued and retained maxima.
- `constructor_local_input_capacity` uses zero only to keep construction
  non-panicking until initialization can return its typed configuration error.
- `declared_local_input_capacity` maps the same invalid arithmetic to
  `u64::MAX`, not zero.
- `SynapseComponent::resources` saturating-adds the retained-result cap, so the
  declaration remains conservative for every `u64` input.
- Local submission probes retained key and payload identity before charging or
  copying the text. Admission repeats the identity check under the job-table
  lock, so concurrent equal submissions reuse one job and conflicting payloads
  remain conflicts.
- Local polling derives the same key from the borrowed item identity and text
  digest without constructing a temporary full-text `BatchItem`.
- `crates/host-runtime/examples/synapse_host.rs` sums the actual composite
  resource declarations when it derives the host resident-byte budget.
- `crates/host-runtime/examples/synapse_perf.rs` adds the model and retained
  result declarations above the normal host resident budget.

The formulas count logical owned string bytes and retained vector bytes. Their
comments expressly exclude struct layout, allocator slack, and hash-table bucket
overhead. This record does not claim an allocator-level RSS bound.

## Failure scenario

One job completes. Its text is released, but its request key, item identifier,
and content hash remain available for polling. A new job then consumes the full
configured queued-input allowance. If the local pool contains only the queued
allowance, valid admission fails early despite the host having declared that the
configuration fits. If the host declaration also omits retained metadata, the
host can admit more ingress memory than its resident budget covers.

An independent configuration failure exists at integer boundaries. Checked
construction arithmetic can fail before bundle initialization. Mapping that
failure to a zero resource declaration would let host startup underreserve before
initialization rejects the component.

## Timing windows and dependencies

The overlap begins when `publish_ready` shrinks a job's charge to retained
metadata and ends when polling retention, eviction, expiry, or shutdown removes
the job. No cancellation or external scheduler is needed to create it.

The pre-initialization declaration is read before bundle validation. It must be
truthful even for limits that validation later rejects.

## What a test must construct

1. Configure at least one queued job and one retained job.
2. Compute the queue and retained maxima through the production helpers.
3. Assert the internal capacity equals their checked sum.
4. Assert the component declaration adds the retained-result maximum.
5. Complete a local job and observe that only its retained metadata remains
   charged until the job table is cleared.
6. Hold the complete local pool and prove submission returns `Full` before the
   engine runs; release it and prove the same input can run.
7. Supply an overlong local item identity and prove `unsupported_shape` occurs
   before allocation or inference.
8. Construct overflowing and `u64::MAX` limits, then assert a `u64::MAX`
   declaration and a typed initialization failure without panic.
9. Retain one local job, saturate the remaining local-input budget, and assert
   an identical replay returns the retained job instead of `Full`.
10. Retain a conflicting payload under the same key, saturate the local-input
    budget, and assert conflict takes precedence over `Full`.

## Investigation log

### Q: Does the retained-input formula include allocator overhead?

- Sources examined: `jobs.rs` charge comments and formulas; `bundle.rs` sizing
  helpers; `mod.rs` resource declaration.
- Findings: The formula covers string contents/capacities and vector bytes. It
  excludes struct and hash-bucket overhead.
- Missing evidence: No allocator or RSS campaign is part of this property.
- Conclusion: Resolved. The guarantee is a logical resident reservation, not a
  process-RSS ceiling.

### Q: Can invalid limits advertise zero retained bytes?

- Sources examined: `constructor_local_input_capacity`,
  `declared_local_input_capacity`, `resources`, and the unvalidated-limit tests.
- Findings: The constructor fallback is zero so initialization can return an
  error; the externally consumed declaration is `u64::MAX` on the same failure.
- Missing evidence: None for the arithmetic contract.
- Conclusion: Resolved with answer: invalid arithmetic fails conservatively.
