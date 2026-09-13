# durable-lineage-anchor-preserves-validation

## Discovery trigger

Fresh evaluator `ses_f6756093fffeVjNp36S3E8pKrM` identifies a durable anchor
hash with enforcement beyond cache lookup or served divergence diagnostics.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: the supplied portfolio finding, plan R3/R4, and frozen identity
constraints. This record does not change the accepted lineage protocol.

## Evidence trail

1. `crates/daemon/src/transform.rs:2144-2196` recognizes a continuation
   summary as the last text block of the first candidate message and records
   its flat block ID, message ID, ordinal, and lowercase content hash.
2. `crates/memory-store/src/lib.rs:1699-1703` declares the durable anchor
   block ID and content hash in `ModuleMeta`.
3. Lines 10351-10367 assign the copied boundary, anchor ID/hash, and ordinal
   continuation base to target state. Served receipts are cleared separately.
4. `crates/daemon/src/transform.rs:2199-2259` requires a completed descent's
   identity/base, checked base-plus-one ordinal, present block, first live
   message, last text position, and exact stored hash.
5. Lines 2252-2258 compare actual lowercase flat hash with the stored string.
   This is a content-byte identity check, not equality-digest receipt reuse.
6. Lines 3100-3107 record validation failure. Lines 3957-3960 set
   `reconcile_pending`, `PassPlan::Defer`, and `lineage_anchor_mismatch`.
7. Lines 4614-4616 retain reconciliation. Lines 4782-4788 clear coverage only
   in the output metadata view, preventing trim on the failed-anchor path.
8. Tests at lines 28739, 28914, 29063, and 29121 cover related anchor cases.
   `crates/memory-store/src/lib.rs:26125` checks the copied durable fields.
   They are unaudited and have not run in this work.

## Failure scenario

An unchanged stored anchor decodes to the same typed text but loses an unknown
envelope field that participated in the old flat hash. Validation then fails,
and the transform defers and avoids trimming. This is stronger than a cache
miss even though the caller can still receive a response.

The contract preserves valid P anchors and genuine fail-closed controls.
It does not clear, rewrite, or bypass a stored identifier to make a new hash
look valid. Old non-P anchor compatibility remains an owner question.

## Timing windows and dependencies

A persisted completed descent is optional in production. When present, the
validator is authoritative, so `always-or-unreached` applies with an independent
campaign marker requiring that state. The record is default-production because
ordinary continued-lineage transforms invoke the validator.
Hold the ordinal base and effective synthetic classification constant.

## What a test must construct

- A baseline-written complete anchor tuple and a known valid P summary block.
- The same tuple after process-cache reset with unchanged input bytes.
- A synthetic head and unedited siblings without changing first-live/last-text
  anchor rules. Use the production effective synthetic view.
- Independent missing tuple, absent block, wrong ordinal, moved text position,
  and real content-mutation controls with their baseline validation outcomes.
- Observe validator errors and the actual Defer/reconcile/no-trim path, not
  an invented generic request rejection or forced HARD materialization.
- `typed_wire_identity_durable_anchor` as its own `sometimes` assertion.

## Investigation log

### Q: Is `anchor_content_hash` only an ephemeral lookup key?

- Sources examined: `ModuleMeta`, descent assignment, validator, and failure
  consumers at the HEAD locations above.
- Findings: no. It survives metadata storage and determines fail-closed output
  behavior. The served receipt vector is a different domain.
- Missing evidence: baseline/candidate execution with a stored anchor.
- Conclusion: resolved as source fact; preservation remains unexercised.

### Q: What authority permits replacing a legacy unknown-envelope anchor hash?

- Sources examined: plan R3/R4 and KTD2, plus exact-hash validation.
- Findings: normalization is accepted in the plan, but no durable anchor
  migration or relaxed enforcement outcome is specified.
- Missing evidence: affected legacy rows and an explicit owner resolution.
- Conclusion: needs human input before implementation. The lineage owner,
  `/testing:test-strategy`, and invariant-guard reviewer receive this conflict;
  the evaluator's recommendation does not authorize weakening the constraint.
