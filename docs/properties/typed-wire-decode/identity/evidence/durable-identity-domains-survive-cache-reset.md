# durable-identity-domains-survive-cache-reset

## Discovery trigger

Plan KTD2 justifies changed daemon-built hashes through synthetic ingress
exclusion and process-local caches. The system wildcard tests that scope.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan KTD2, latency-audit B1/B2/B5/W5, and issue 350.
Lenses: state/persistence, lifecycle, failure recovery, system wildcard.

## Evidence trail

1. `crates/daemon/src/wire.rs:440-448,599-617` recognizes effective synthetic
   status and excludes synthetic ingress from identity vectors.
2. `crates/daemon/src/transform.rs:2126-2141` marks unflagged synthetic todo
   IDs in a pass-local projection view.
3. Lines 5163-5244 enforce persisted identity with specific exemptions and
   reject covered, anchored, or frozen drift; lines 5269-5298 adopt vectors.
4. `crates/memory-store/src/lib.rs:1600` persists those ingress identities.
5. Lines 1279-1319,1498 also persist frozen synthetic messages. Synthetic
   exclusion is not a promise that no synthetic data is durable.
6. Lines 1330-1334,1733-1736 define durable served receipts with hash and
   serialized length. `transform.rs:2048-2089` includes synthetic outputs.
7. `transform.rs:4923-4930` compares old/new served vectors and assigns the
   new vector into metadata. `divergence.rs:42-95` reports changed records.
8. `crates/daemon/src/lib.rs:3684-3689`, verified from HEAD, constructs empty
   process-local snapshot/output/native/projection caches.

## Failure scenario

An upgrade changes a durable served receipt but the implementation or test
treats the old vector as an empty cache. This hides a real first-divergence
event. Another failure lets unflagged synthetic replay enter ingress identity
enforcement, or changes an unchanged plugin vector after process restart.

This record separates ingress vectors, frozen synthetic messages, served
diagnostic receipts, and process-local caches. Two additional durable domains,
hygiene baselines and lineage anchors, have their own evidence files and exact
checks after fresh evaluation. They are not folded into served diagnostics.
This record does not demand
unchanged typed-output hashes where the plan expressly changes their basis.
It grants no semantic exception for a changed baseline, anchor, or policy.

## Timing windows and dependencies

Construct caches cold while preserving the actual metadata row. A clean new
store cannot witness this boundary. Warm/cold comparisons hold effective input
and metadata constant and exclude unrelated policy transitions.
Reachability is default-production: ordinary transforms load metadata and
populate served receipts without an opt-in differential mode.

## What a test must construct

- Stored covered/frozen P identities, preventing permissive tail re-adoption
  from concealing an unintended identity change.
- A nonempty prior served receipt vector and a stored synthetic todo pair.
- Fresh empty caches, followed by equivalent warm-cache construction.
- A replayed unflagged synthetic ID on a delta beside ordinary messages.
- Expected plugin vectors and expected new served receipts as distinct values.
- Record `typed_wire_identity_cold_durable_state` before recomputation.
- Do not equate process reopen with proof of crash durability.

## Investigation log

### Q: Are every changed daemon-built hash and length process-local?

- Sources examined: `ModuleMeta`, `served_output_fingerprints`, and cache setup.
- Findings: no. Durable served receipts include synthetic blocks, and frozen
  synthetic messages are also persisted. Ingress vectors exclude them.
- Missing evidence: the accepted first post-upgrade diagnostic result and
  its interaction with frozen-output byte promises.
- Conclusion: needs human input from the parent specification owner. Preserve
  the observed source distinction; do not silently reset the durable vector.

### Q: What happens to old covered explicit-false or unknown-field ingress?

- Sources examined: plan R3/KTD2 and `enforce_block_identity`.
- Findings: the plan accepts normalized identity changes, but old vectors can
  be persisted and covered/frozen drift can reject. No migration is specified.
- Missing evidence: whether such sessions exist and the intended handling.
- Conclusion: needs human input. The preservation property remains scoped to
  P; `/testing:test-strategy` and the invariant-guard reviewer receive this seam.
