# block-byte-policy-outcomes-remain-stable

## Discovery trigger

Fresh evaluator `ses_f6756093fffeVjNp36S3E8pKrM` requests explicit token,
fold, and protected-tail outcomes rather than byte-hash coverage alone.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: the supplied portfolio finding, the plan's FlatBlock consumer
table, R4, and broad frozen behavior. W1 is invalidated and is not reactivated.

## Evidence trail

1. `crates/daemon/src/transform.rs:5885-5913` tokenizes full block bytes,
   accumulates tokens by ordinal in reverse order, and derives the protected
   floor. Lines 4592-4602 store the publication floor on applicable HARD passes.
2. `crates/daemon/src/lib.rs:17260-17290`, verified against HEAD, builds
   boundary blocks with `byte_size`, cached original-token counts keyed by
   content hash, and shared original bytes from the flat projection.
   Its token-cache gate at lines 2055-2080 requires both byte length and hash;
   a miss calls the tokenizer on the exact supplied bytes.
3. `crates/daemon/src/boundary.rs:966-1035` builds original-token prefix sums.
   Lines 1058-1108 use them for range and suffix boundaries.
4. Lines 376-426 resolve the protected tail from that index. Output types at
   279-305 carry fire/reason, consume-through ordinal, boundary, and progress.
5. `crates/daemon/src/transform.rs:6377-6408` maps flat byte length into
   `SelItem.byte_size`; tag-token count is a separate optional input.
6. `crates/daemon/src/selection.rs:900-925` rounds byte-derived token estimates.
   Lines 948-1011 use active-floor and reclaimed-token estimates to choose
   emergency reductions and stop at the reclaim target.
7. `SelectionOutcome` at lines 1023-1030 includes decisions and independent
   flags/counts; comparing only decisions can miss a state change.
8. `crates/daemon/src/lib.rs:18182`, boundary goldens, and selection
   differential tests provide source-visible seams, not replacement evidence.

## Failure scenario

Sorting or omitting a false field changes a daemon-built block's tokenizer
input or rounded byte estimate. A small numeric difference moves a protected
ordinal, changes whether a fold fires, or selects another reduction. Equal
typed payloads and consistent new hashes cannot detect this policy drift.

For P input, freeze the complete old numeric and decision outcomes. For
daemon-only byte changes, establish whether a real policy consumer sees them
and record any resulting conflict. An accepted byte change is not an accepted
token-budget, fold, or protection change.

## Timing windows and dependencies

Hold context, tags, core, clocks, finite budgets, and estimator identity fixed.
Compare cold/warm caches and decoded representations, not different workloads.
Reachability is default-production through boundary preparation, reduction
selection, and protected-floor persistence on applicable ordinary passes.
No performance measurement, benchmark, or elapsed-time assertion is needed.

## What a test must construct

- An emitter-derived P corpus near byte-rounding, token-suffix, trigger-budget,
  and emergency-rearm boundaries, with real call/result pairing.
- Exact old per-block lengths, estimator counts, prefix totals, complete
  boundary/trigger results, selection outcomes, and publication-floor results.
- Tagged versus untagged items so byte and tag-token inputs stay distinct.
- Independent baseline expectations, not expectations regenerated with the
  candidate canonical helper or candidate estimator.
- Separately classified daemon-built false-tool and reordered typed-block
  cases. Observe their actual consumers without inventing permitted drift.
- `typed_wire_identity_byte_policy_inputs` as its own `sometimes` marker.

## Investigation log

### Q: Does a common canonical hash producer prove policy preservation?

- Sources examined: boundary construction/index, selection byte estimates,
  protected-floor calculation, and the plan's consumer inventory.
- Findings: no. Hash agreement does not compare token numbers, thresholds, or
  independent outcome flags. Exact P bytes support preservation but do not
  replace exercising these consumers under fixed inputs.
- Missing evidence: a frozen old/new outcome corpus at those boundaries.
- Conclusion: unresolved, needs `/testing:test-strategy` and boundary/selection
  owners. Existing checks remain unaudited and all properties unexercised.

### Q: May daemon-only byte changes alter fold/protection decisions?

- Sources examined: plan accepted byte omissions and the consumer paths above.
- Findings: no semantic exception is stated. Reachability of each changed
  daemon-only value at each consumer still needs a concrete witness.
- Missing evidence: that witness and an owner resolution of any conflict.
- Conclusion: needs human input before implementation; reviewer recommendations
  cannot approve rebaselining a changed decision or reactivate invalidated W1.
