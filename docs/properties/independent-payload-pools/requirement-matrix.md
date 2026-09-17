# Requirement-to-evidence matrix

Maps every requirement, decision, and milestone of #524 to the catalog records
that carry its evidence and to the task that owns any gap. Update the status
column as each task lands or a record retires. Status values: `landed` (every
named record is `yes`), `partial`, `open`, and `retired` (no longer in scope).
Unchanged rows retain the introducing snapshot's status.

| Item | Records | Owner of gaps | Recorded status |
| --- | --- | --- | --- |
| R1 | `descriptor-capacity-independent-of-payload` | - | landed |
| R2 | `released-block-reuse-preserves-held-bytes`, `payload-identity-authorizes-reuse` | - | landed |
| R3 | `completion-cell-final-owner-once`, `class-allocation-conservation` | - | landed |
| R4 | `retained-mapping-lifetime`, `wake-failure-preserves-published-ownership`, `request-conversion-completion-ownership` | a test that pauses the production inbound copy under Cancel, route close, or shutdown | partial |
| R5 | `owned-lease-thread-boundary` | - | landed |
| R6 | `shared-copy-source-access`, `private-decode-input-stability` | safe-over-unsafe then unsafe-review for the shifted-overlap and JavaScript-writer exclusions | partial |
| R7 | `application-frame-interoperability`, `send-outcome-no-generic-replay` | combined matrix for native stop/restart | partial |
| R8 | `sole-identifiers-before-activation`, `validated-setup-geometry` | - | landed |
| R9 | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement` | late-finalizer witness on a detachment-capable runtime | partial |
| R10 | `complete-capacity-admission`, `bounded-refusal-and-recovery`, `response-retention-isolation` | - | landed |
| R11 | `reserved-progress-under-data-exhaustion`, `capacity-wake-progress` | - | landed |
| R12 | `capacity-model-conservation` (invalidated) | none; research model removed | retired |
| R13 | `single-replacement-surface` | last task reruns | landed at this revision |
| KTD1 | `class-allocation-conservation`, `direct-serialization-commit-boundary` | - | landed |
| KTD2 | `descriptor-private-snapshot`, `payload-identity-authorizes-reuse`, `validated-setup-geometry` | `descriptor-private-snapshot` handoff (unsafe-review, invariant-test-review) | partial |
| KTD3 | `completion-cell-final-owner-once`, `worker-drop-forbidden-operations`, `capacity-wake-progress`, `wake-failure-preserves-published-ownership` | - | landed |
| KTD4 | `owned-lease-thread-boundary`, `shared-copy-source-access`, `private-decode-input-stability`, `request-conversion-completion-ownership` | the `shared-copy-source-access` review handoff; a test that pauses the production inbound copy | partial |
| KTD5 | `retained-mapping-lifetime`, `partial-setup-reclaims-only-unexposed-resources` | last task (descriptor-duplication failure injection) | partial |
| KTD6 | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement`, `response-retention-isolation` | late-finalizer witness on a detachment-capable runtime | partial |
| KTD7 | `complete-capacity-admission`, `terminal-credit-follows-storage`, `reserved-publication-order`, `terminal-encoding-reserve-bound`, `reclamation-diagnostics-meaning` | - | landed |
| KTD8 | `sole-identifiers-before-activation`, `single-replacement-surface` | - | landed |
| U1 contract | `docs/payload-pool-protocol.md`, `sole-identifiers-before-activation`, `structural-rejection-before-dispatch` | a host-level structural-rejection witness beyond `validate_inbound_header` | partial |
| U2 allocation | `class-allocation-conservation`, `descriptor-capacity-independent-of-payload`, `released-block-reuse-preserves-held-bytes` | - | landed |
| U3 owned leases | `completion-cell-final-owner-once`, `retained-mapping-lifetime`, `owned-lease-thread-boundary`, `worker-drop-forbidden-operations` | - | landed |
| U4 runtime input | `private-decode-input-stability`, `request-conversion-completion-ownership`, `reserved-publication-order` | a test that pauses the production inbound copy under Cancel, route close, or shutdown | partial |
| U5 native and TypeScript | `native-alias-closure-before-transfer`, `partial-close-token-conservation`, `environment-finalizer-confinement`, `response-retention-isolation` | late-finalizer witness on a detachment-capable runtime | partial |
| U6 Rust client and accounting | `response-retention-isolation`, `bounded-refusal-and-recovery`, `reclamation-diagnostics-meaning` | - | landed |
| U7 validation and deletion | `single-replacement-surface`, `fuzz-adapter-current-contract`, `malformed-fixture-valid-baseline`, `real-process-current-layout-witness` | last task | partial |
| Gate integrity | `integration-gate-dependency-selection`, `unsafe-witness-selection`, `acceptance-artifact-provenance` | artifact digest tied to source revision | partial |
