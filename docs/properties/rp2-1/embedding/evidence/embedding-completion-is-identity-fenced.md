# embedding-completion-is-identity-fenced

## Discovery trigger

P1 line 149 rejects stale fingerprint/dimension/hash results, and line 156
counts vectors only for the current revision and model. The combined claim
requires identity checks at product completion, not merely request admission.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only: no production RP2.1 completion transaction exists.

## Evidence trail

- `crates/host-runtime/src/synapse/protocol.rs:781-805` checks requested model,
  fingerprint, and table epoch against the served lane before inference.
- `protocol.rs:873-885` verifies each input hash against exact text bytes.
- `crates/host-runtime/src/synapse/jobs.rs:108-127` retains item ID/hash and
  requested dimension, not a canonical source revision or current-model view.
- `jobs.rs:502-535` ignores late publication to a completed local job and
  rejects wrong item count or dimensions. These are narrower local fences.
- `jobs.rs:640-654` returns stored ID/hash metadata paired with result vectors.
- `crates/host-runtime/src/synapse/inference.rs:222-240` validates vector
  dimensions, finiteness, and norm; numerical validity does not imply freshness.
- `crates/host-runtime/src/synapse/bundle.rs:652-702` excludes model name from
  fingerprint. Equal fingerprints therefore do not prove equal model labels.
- `crates/kernel/src/envelope.rs:395-438` rewrites `domains.name` through
  operator remediation and carries the loaded object into PendingChange
  without changing its revision. This affects embedding only if the approved
  input mapping depends on that field. K already includes the input hash.

## Failure scenario

Dispatch captures revision r and model m. The occurrence advances to r+1 or
the active identity changes before the result arrives. A driver checks only
the result dimension and writes it as the current occurrence's vector.
The same failure occurs after deletion, after a payload hash change, or when
two occurrences share identical bytes and only one is still current.
A fingerprint-only comparison also misses a model-name change that leaves
the fingerprint unchanged. The plan requires that model comparison explicitly.
If remediation changes mapped input bytes at the same revision, a driver
comparing only revision or a stale projected hash can accept the old result.
A proper comparison with authoritative current mapped bytes detects the change.

## Timing windows and dependencies

The current-K comparison belongs in the same serialization boundary as the
product completion mutation. A pre-dispatch or pre-transaction read can go stale.
The harness must distinguish request identity, returned metadata, and current
authoritative mapped input instead of deriving all three from the candidate
result or its stale projection. The
[projection remediation record](../../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
owns invalidation when a mapped source field is remediated.
Projection owns occurrence/revision/tombstone semantics. This record consumes
them and does not create another source of truth for canonical eligibility.
Identity mismatch can obsolete old work but cannot complete the newer identity.

## What a test must construct

Park a valid result after dispatch. Independently change one of revision,
model, fingerprint, dimensions, epoch, or input hash, then release the result.
Run deletion and same-payload/different-occurrence cases separately.
If the approved mapping uses the remediated field, change it through operator
remediation with the revision held unchanged. Compare independent authoritative
mapped bytes before and after; the old input hash cannot establish current(K).
For the model case keep fingerprint and dimension equal to isolate the check.
Use valid vector shape when testing other identity fields so a shape rejection
does not mask a missing revision or model comparison.
Observe the product write boundary and compare current vector/completion state
to an independent pre-result snapshot. A matching control case must complete.
`rp21_embedding_identity_changed_with_result_held` records the update window;
its input-hash dimension includes applicable same-revision remediation. No new
marker is needed, and acceptance/rejection is not part of its precondition.

## Investigation log

### Q: Do existing JobTable fences already reject a stale source revision?

- Sources examined: `jobs.rs:108-127`, `:502-557`; `protocol.rs:781-805`.
- Findings: Local checks bind the job and served lane. They do not consult
  canonical occurrence revision or a later product identity transition.
- Missing evidence: None for that boundary distinction.
- Conclusion: Resolved. Reuse the local checks and add the product property.

### Q: Which atomic predicate supplies current(K)?

- Sources examined: P1 KTD4 and U3/U4; P2 occurrence-identity shared contract.
- Findings: The plan specifies the identity content and stale-result outcome,
  but the projection mutation and obsolete-state representation are proposed.
- Missing evidence: A compare-and-write API and authoritative tombstone view.
- Conclusion: Needs human input. The property fixes the forbidden outcome,
  not the transaction schema or a duplicate identity service.

### Q: Does operator remediation leave every embedding identity unchanged?

- Sources examined: `crates/kernel/src/envelope.rs:395-438`, K's input-hash
  definition, and the central G1 finding with the user's qualification.
- Findings: The domain revision stays unchanged, but authoritative name bytes
  change. A dependent mapping therefore changes the current input hash. The
  approved mapping may exclude domain names entirely.
- Missing evidence: Which approved embedding inputs depend on remediated fields.
- Conclusion: The unchanged-K claim is rejected. Mapping applicability needs
  human input; affected completion compares authoritative current bytes/hash.
  No new key/version or broad erasure policy follows from this refinement.
