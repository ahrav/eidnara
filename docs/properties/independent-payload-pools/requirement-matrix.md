# Requirement-to-evidence matrix

Maps every requirement, decision, and milestone of #524 to the catalog records
that carry its evidence and to the task that owns any gap. Update the status
column as each task lands. Status values: `landed` (every named record is
`yes`), `partial`, `open`.

| Item | Records | Owner of gaps | Status at the tree of this catalog's introducing commit |
| --- | --- | --- | --- |
| R1 | `descriptor-capacity-independent-of-payload` | - | landed |
| R2 | `released-block-reuse-preserves-held-bytes`, `payload-identity-authorizes-reuse` | - | landed |
| R3 | `completion-cell-final-owner-once`, `class-allocation-conservation` | - | landed |
| R4 | `retained-mapping-lifetime`, `wake-failure-preserves-published-ownership`, `request-conversion-completion-ownership` | #548 | partial |
| R5 | `owned-lease-thread-boundary` | - | landed |
| R6 | `shared-copy-source-access`, `private-decode-input-stability` | #548 | partial |
| R7 | `application-frame-interoperability`, `send-outcome-no-generic-replay` | #548, #552, #550 | partial |
| R8 | `sole-identifiers-before-activation`, `validated-setup-geometry` | - | landed |
| R9 | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement` | #550 | partial |
| R10 | `complete-capacity-admission`, `bounded-refusal-and-recovery`, `response-retention-isolation` | #548, #552, #550 | partial |
| R11 | `reserved-progress-under-data-exhaustion`, `capacity-wake-progress` | #548, #552, #550 | partial |
| R12 | `capacity-model-conservation` | - | landed |
| R13 | `single-replacement-surface` | last task reruns | landed at this revision |
| KTD1 | `class-allocation-conservation`, `direct-serialization-commit-boundary` | #548 | partial |
| KTD2 | `descriptor-private-snapshot`, `payload-identity-authorizes-reuse`, `validated-setup-geometry` | - | landed |
| KTD3 | `completion-cell-final-owner-once`, `worker-drop-forbidden-operations`, `capacity-wake-progress`, `wake-failure-preserves-published-ownership` | - | landed |
| KTD4 | `owned-lease-thread-boundary`, `shared-copy-source-access`, `private-decode-input-stability`, `request-conversion-completion-ownership` | #548 | partial |
| KTD5 | `retained-mapping-lifetime`, `partial-setup-reclaims-only-unexposed-resources` | #548 | partial |
| KTD6 | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement`, `response-retention-isolation` | #550 | partial |
| KTD7 | `complete-capacity-admission`, `terminal-credit-follows-storage`, `reserved-publication-order`, `terminal-encoding-reserve-bound`, `reclamation-diagnostics-meaning` | #548, #552, #550 | partial |
| KTD8 | `sole-identifiers-before-activation`, `single-replacement-surface` | - | landed |
| U1 contract | `docs/payload-pool-protocol.md`, `sole-identifiers-before-activation`, `structural-rejection-before-dispatch` | - | landed |
| U2 allocation | `class-allocation-conservation`, `descriptor-capacity-independent-of-payload`, `released-block-reuse-preserves-held-bytes` | - | landed |
| U3 owned leases | `completion-cell-final-owner-once`, `retained-mapping-lifetime`, `owned-lease-thread-boundary`, `worker-drop-forbidden-operations` | - | landed |
| U4 runtime input | `private-decode-input-stability`, `request-conversion-completion-ownership`, `reserved-publication-order` | #548 | open |
| U5 native and TypeScript | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement`, `response-retention-isolation` | #550 | open |
| U6 Rust client and accounting | `response-retention-isolation`, `bounded-refusal-and-recovery`, `reclamation-diagnostics-meaning` | #552, #548 | open |
| U7 validation and deletion | `single-replacement-surface`, `fuzz-adapter-current-contract`, `malformed-fixture-valid-baseline`, `real-process-current-layout-witness` | last task | partial |
| Gate integrity | `integration-gate-dependency-selection`, `unsafe-witness-selection`, `acceptance-artifact-provenance` | #550 for artifact identity | partial |
