# identity-edge-states-are-exercised

## Discovery trigger

The existing golden covers only a small shape set and receipt reuse can bypass
the fresh byte producer. Safety assertions need independent enabling evidence.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: METHOD, the discovery skill, plan U2, and latency-audit B1/W5.
Lenses: existing test strategy, every property lens, final wildcard.
Refinement source: fresh evaluator `ses_f6756093fffeVjNp36S3E8pKrM`, supplied
by the user after its four completed portfolio lenses.

## Evidence trail

1. `crates/daemon/src/transform.rs:14467-14490` runs a fixed projection
   golden. Static inspection finds absent/true tool and media coverage gaps.
2. Lines 177-200 distinguish reuse from fresh fallback. A test reaching the
   constructor may never execute the prospective common-byte fallback.
3. Lines 13813-13938 show available signed-zero and candidate-order fixtures.
4. `crates/daemon/src/codec/sidecar.rs:177-201` supplies stamp construction
   and parsing seams, but no aggregate exclusion marker is present.
5. `crates/daemon/src/lib.rs:16443-16461` requires supplied stored rows to
   reach meaningful history recovery. An empty store is not an upgrade probe.
6. `crates/daemon/src/history_summarizer_chunk.rs:418-430` requires synthetic and real
   inputs to witness exclusion without demanding a forbidden synthetic item.
7. `crates/daemon/src/wire.rs:1749-1784` demonstrates a shared-shell test seam.
   No claim is made that this existing test satisfies the proposed marker.

## Failure scenario

All safety checks are evaluated on text-only blocks in a fresh store.
No flag omission, forced fallback, namespace stripping, historical decoder,
durable receipt, or sibling alias is ever exposed. The suite is green but
cannot distinguish the relevant faulty implementations.

Each of twelve fixed markers is an independent `sometimes` assertion. This
record only rolls up their results; it creates no combined runtime predicate.
Each records constructed inputs or a selected operation before the associated
oracle. None requires an unexpected identity change or corruption to occur.

## Timing windows and dependencies

This is test-only reachability. Production has no obligation to synthesize all
edge cases in one turn. A finite campaign failure means missing construction
or a no-longer-reachable declared situation, not a proof of failed liveness.
No elapsed sleep substitutes for historical state or cache-reset construction.

## What a test must construct

Use the exact twelve predicates in `../fault-map.md`:

1. `typed_wire_identity_plugin_shapes`.
2. `typed_wire_identity_default_forms`.
3. `typed_wire_identity_fresh_and_sidecar`.
4. `typed_wire_identity_receipt_candidates`.
5. `typed_wire_identity_old_rows`.
6. `typed_wire_identity_synthetic_snapshot_inputs`.
7. `typed_wire_identity_cold_durable_state`.
8. `typed_wire_identity_shared_sibling_edit`.
9. `typed_wire_identity_typed_only_shapes`.
10. `typed_wire_identity_durable_hygiene`.
11. `typed_wire_identity_durable_anchor`.
12. `typed_wire_identity_byte_policy_inputs`.

Names are constant literals, never dynamic shape IDs. The shape matrix can be
a finite bitmap owned by the campaign. Plugin and typed-only matrices are
separate: the latter includes all seven output variants, including JSON,
error JSON, and execution denial with null reason, plus all seven block kinds.
Do not set a marker because an assertion passed or because a branch name ran.

## Investigation log

### Q: Do old catalogs or tests satisfy these markers already?

- Sources examined: latency-audit B1/W5 and scoped source checks.
- Findings: they contain related historical evidence, not this campaign's
  versioned shape partition, old/new artifact pair, or marker contract.
- Missing evidence: completed marker observations under the replacement.
- Conclusion: resolved, all twelve remain unexercised. Existing checks are
  separately inventoried with unaudited status.

### Q: Are all proposed situations feasible at the named seams?

- Sources examined: emitter, test-support receipts, history reader, and caches.
- Findings: constructors exist, but old-row fixtures and exposure of the new
  producer require implementation work. No form choice is made here.
- Missing evidence: implemented observations at the new typed-only, persisted,
  and threshold-sensitive seams identified by the completed fresh evaluation.
- Conclusion: unresolved implementation evidence goes to `/testing:test-strategy`.
  The four-lens evaluation is complete as supplied; no rerun is claimed.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`; replay envelopes removed. Markers constructed by the executed tests: typed-only false flags, explicit-false ingress, signed zeros, positional mismatch, repeated candidates, two provider namespaces, changed stamps (transform.rs and sidecar.rs tests), unknown-envelope normalization on decode, and a copy-on-write edit with an untouched sibling. Not constructed: a durable hygiene or lineage anchor reload under the owned model.

A persisted old-row replay is constructed by
`replay_basis_identity_rows_re_adopt_once_under_the_typed_basis` (transform.rs):
a `ModuleMeta` row without `block_identity_basis` carrying a covered
`block_identity_by_mid` vector hashed from replayed ingress bytes (unknown block
key, `provider_executed: false`) is re-adopted to the typed vector and stamped
`typed` instead of returning `IdentityDrift`; a vector whose `kind_tag` values
differ still rejects, and the stamped row rejects covered drift.
`module_meta_basis_stamp_defaults_split_stored_rows_from_built_metas`
(memory-store lib.rs) pins the stamp's serde defaults.
