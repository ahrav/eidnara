# Final wildcard pass

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
This pass follows the model, property, and existing-coverage readback passes.
Sources: [catalog source register](../catalog.md#source-register).

## A whole-text hash does not prove whole-text inference

The wire contract explicitly hashes the whole text while truncating its model
input (`docs/host-wire-protocol.md:472`). A correct hash and a valid unit vector
therefore cannot substitute for exact untruncated preflight. The count oracle
must compare an independently pinned token sequence, including special-token
behavior and excluding batch padding, rather than infer coverage from a hash.
This refines the first two records without creating a duplicate fingerprint
record.

## Ready and complete name different boundaries

`JobState::Ready` contains resident vectors and a ResultLease
(`crates/host-runtime/src/synapse/jobs.rs:91-106`). The word does not imply a
durable product write. `embedding-complete-requires-durable-vector` deliberately
observes reopened storage instead of a ready poll or an attempted SQL write.

## Logical stop and physical stop can disagree

Synapse joins started native calls after request expiry
(`crates/host-runtime/src/synapse/mod.rs:669-722`, `:1148-1161`). A shared
EvalBudget must not let expired work report a successful query or release its
physical reservations early. A finite native-call drain bound is not established
by that ownership. The supervisor record keeps this as an unresolved production
decision rather than assigning a timeout that cannot stop the native runtime.

## Garbage collection can erase a retry explanation

A GC scan can select an old identity before a delayed completion arrives.
Revalidate deletion eligibility at mutation, fence the late result, and preserve
the current identity's pending work. Durable GC eligibility and reference sets
are proposed; the existing result-page lease is only an in-memory analogy.
The retention/grace policy belongs to the owner and RP2.9.

## Portfolio boundary

Nine records concentrate on embedding-specific deltas. Their seven safety and
two bounded-liveness records use `always`; independent `sometimes` precondition
markers live in the fault map. The central independent evaluation and local
dispositions are retained in [portfolio-evaluation.md](../portfolio-evaluation.md).
The shared acceptance record requires every declared enabled scenario dimension;
this distribution alone is not coverage evidence. These edits are not a second
independent review.
