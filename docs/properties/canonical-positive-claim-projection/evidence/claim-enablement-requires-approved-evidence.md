# claim-enablement-requires-approved-evidence

## Discovery trigger

Specification acceptance 'Enablement': current approvals, prerequisites, RP2.9 evidence, and actual backend support are required together.

## Evidence trail

- `crates/daemon/src/projection_gates.rs`: `ProjectionHook` lists the hooks the evidence gate admits; none names claim retrieval or causality. `EmbeddingBootstrap` needs `CanonicalClaims` coverage among its classes (`:80-84`).
- `crates/daemon/src/search_lifecycle_owner.rs`: the maintenance loop calls `run_claim_slice` (`:2216`) before each lifecycle slice. `run_claim_slice` (`:551`) runs `ClaimMaterializer` (`:570`) with local-only egress and asks the gate for no admission, so claim source descriptors are published with no projection-gate admission.
- `crates/daemon/src/query_route.rs`: the query unit calls `SearchLifecycleOwner::pin` (`:2016`) before `execute`, whose claim validation reaches `validate_claims` (`:1707`). `pin` admits `EmbeddingBootstrap` through `SearchSelection::admit_reader` (`crates/daemon/src/search_replacement/selection.rs:430`), which refuses with `NoManifest` until an owner installs a search admission (`crates/daemon/src/projection_admission.rs:220`).
- `validate_claims` reads projection rows through `retrieval::claims::read_selected_claims` (`crates/retrieval/src/claims.rs:441`) and the kernel's claim facts through `judge_selected_for_surface` (`query_route.rs:1747`; `claims.rs:476`), which calls `KernelStore::judge_surface_eligibility_with_selected_claims` (`claims.rs:516`).
- `record_claim_causality` has no caller outside `crates/kernel` and the test crates (`rg record_claim_causality crates/`).

## Failure scenario

The claim slice publishes claim source descriptors on every daemon start with no admitted hook and no approved RP2.9 limits. Once an owner installs a search admission for the generic dense lanes, the query route also reads claim facts and serves claims, admitted only by `EmbeddingBootstrap`, which approves no claim-specific limits or evidence.

## Timing windows and dependencies

None.

## What a test must construct

A repository test that lists the callers of claim descriptor publication, the claim facts readers, and `record_claim_causality`, and asserts each sits behind an admitted claim hook. It cannot pass until a claim hook exists and the claim slice takes it.

## Investigation log

### Q: Which `ProjectionHook` variant gates claim retrieval and causality writes?

- Sources examined: the files in the evidence trail, the specification text, and the ticket bodies.
- Findings: no claim-specific variant exists. The claim slice takes no admission; the query route takes `EmbeddingBootstrap`.
- Missing evidence: an owner decision on whether `EmbeddingBootstrap` suffices for claim delivery and on which hook gates claim publication.
- Conclusion: needs human input
