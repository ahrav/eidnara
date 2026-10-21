# Specification integration

Crosswalk from each stable slug to the specification section that motivates
it, the milestone that owns making it checkable, and the part that holds the
record. Ticket numbers are assigned by the ticketing step and are added here
when they exist; "existing mechanism" marks a record for behavior that is
implemented at HEAD `08de9051a` and that a milestone relies on or changes.

Specification: "Authoritative rules reach coding agents automatically" (issue
reference recorded in README.md after publication).

| Slug | Part | Origin | Specification section | Requirements | Milestone | Ticket |
| --- | --- | --- | --- | --- | --- | --- |
| `explicit-user-accepted-adr-self-approves-to-approved` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R1 | existing mechanism; reused by U3 and U5 | unassigned |
| `accepted-adr-requires-live-accepted-decision-subject` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R1, R3 | existing mechanism; reused by U3 and U5 | unassigned |
| `config-observed-promotes-only-on-exact-lineage-trigger` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R2 | existing mechanism; reused by U3 and U5 | unassigned |
| `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R1, R2, R12 | existing mechanism; reused by U3 and U5 | unassigned |
| `authority-loss-cascades-to-latest-dependents` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R7 | existing mechanism; reused by U3 and U5 | unassigned |
| `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R7, R8 | existing mechanism; reused by U3 and U5 | unassigned |
| `explicit-reject-moves-to-rejected-review-only-and-bars-succession` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R3 | existing mechanism; reused by U3 and U5 | unassigned |
| `plugin-commit-classes-are-closed-at-model-inference` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R1, R3, R4 | existing mechanism; reused by U3 and U5 | unassigned |
| `policy-digest-pins-revision-one` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R4 | existing mechanism; reused by U3 and U5 | unassigned |
| `scope-filter-excludes-a-term-on-an-unresolved-dimension` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R5 | existing mechanism; reused by U3 and U5 | unassigned |
| `sql-project-prefilter-is-a-superset-of-the-daemon-verdict` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R5 | existing mechanism; reused by U3 and U5 | unassigned |
| `served-rows-exclude-objects-invalidated-at-the-snapshot` | `kernel-authority-and-scope/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD1, KTD3 | R7, R8 | existing mechanism; reused by U3 and U5 | unassigned |
| `injectable-rows-require-visible-verdict-and-positive-decision-kind` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R12 | existing mechanism; changed by U2 | unassigned |
| `memory-trim-admits-in-serving-order-not-category-order` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R10 | existing mechanism; changed by U2 | unassigned |
| `memory-trim-cost-model-mirrors-rendered-block-structure` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R10 | existing mechanism; changed by U2 | unassigned |
| `memory-trim-clamps-malformed-budget-and-floors-the-integer` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R10 | existing mechanism; changed by U2 | unassigned |
| `snapshot-revision-digests-only-rendered-rows` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R11 | existing mechanism; changed by U2 | unassigned |
| `project-memory-truncated-flag-excludes-budget-trimming` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R11 | existing mechanism; changed by U2 | unassigned |
| `subagent-pass-is-never-hard-and-serves-no-m0` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R13 | existing mechanism; changed by U6 | unassigned |
| `additive-only-pass-still-composes-project-memory` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R13 | existing mechanism; changed by U6 | unassigned |
| `pi-context-handler-is-gated-by-compaction-on` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R13 | existing mechanism; changed by U6 | unassigned |
| `pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R6, R13 | existing mechanism; changed by U6 | unassigned |
| `memory-mark-command-is-model-free-and-confirms-before-writing` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R1 | existing pattern; mirrored by U4 | unassigned |
| `memory-seed-fixture-writes-repo-source-kind-under-code-observed` | `injection-and-harness/` | implemented | Constraints and Invariants, "Repository facts"; Implementation Decisions KTD5, KTD6, KTD8 | R13 (AE8 seam) | existing mechanism; changed by U6 | unassigned |
| `directive-record-self-approves-at-hardcoded-class-pair` | `write-channels/` | proposed | Requirements R1; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R1 | U3; exercised through U4 e2e | unassigned |
| `directive-explicit-user-unreachable-from-model-paths` | `write-channels/` | proposed | Requirements R1, R3; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R1, R3 | U3; exercised through U4 e2e | unassigned |
| `directive-operation-key-replay-is-idempotent` | `write-channels/` | proposed | Requirements R1, R7; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R1, R7 | U3; exercised through U4 e2e | unassigned |
| `directive-revise-supersedes-same-lineage-and-reapproves` | `write-channels/` | proposed | Requirements R7; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R7 | U3; exercised through U4 e2e | unassigned |
| `directive-revoke-cascades-and-stops-serving` | `write-channels/` | proposed | Requirements R7; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R7 | U3; exercised through U4 e2e | unassigned |
| `directive-confirmed-from-promotes-by-fresh-record` | `write-channels/` | proposed | Requirements R3; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R3 | U3; exercised through U4 e2e | unassigned |
| `directive-explicit-user-subject-guard-refuses-model-commit` | `write-channels/` | proposed | Requirements R9; KTD1, KTD3, KTD9; Acceptance AE1, AE2, AE6 | R9 | U3 (guard in kernel.commit) | unassigned |
| `repo-policy-rules-derive-from-head-tree-or-nothing` | `write-channels/` | proposed | Requirements R2; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R2 | U5 | unassigned |
| `repo-policy-bounds-fail-closed-per-file` | `write-channels/` | proposed | Requirements R2; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R2 | U5 | unassigned |
| `repo-policy-segmentation-is-deterministic` | `write-channels/` | proposed | Requirements R2, R13; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R2, R13 | U5 | unassigned |
| `repo-policy-unit-reconciliation-by-digest` | `write-channels/` | proposed | Requirements R8; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R8 | U5 | unassigned |
| `repo-policy-rule-verified-only-with-provenance-witness` | `write-channels/` | proposed | Requirements R2, R12; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R2, R12 | U5 | unassigned |
| `repo-policy-per-file-commit-atomic-and-pass-converges` | `write-channels/` | proposed | Requirements R8; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R8 | U5 | unassigned |
| `repo-policy-cached-head-oid-is-advisory-and-reconverges` | `write-channels/` | proposed | Requirements R8; KTD2, KTD3; Acceptance AE3, AE4, AE5 | R8 | U5 | unassigned |
| `scope-context-is-resolved-per-pass-from-named-sources` | `read-side/` | proposed | Requirements R6; KTD4; Acceptance AE3 | R6 | U1 | unassigned |
| `scope-rule-serves-only-when-every-carried-term-matches` | `read-side/` | proposed | Requirements R5; KTD4; Acceptance AE3 | R5 | U1 | unassigned |
| `scope-directory-ancestors-derive-only-from-paths-inside-project-root` | `read-side/` | proposed | Requirements R5, R6; KTD4; Acceptance AE3 | R5, R6 | U1 | unassigned |
| `packing-selection-never-admits-every-rendered-row-is-kernel-visible` | `read-side/` | proposed | Requirements R12; KTD5, KTD6; Acceptance AE7 | R12 | U2 | unassigned |
| `packing-required-order-is-total-and-identical-across-passes` | `read-side/` | proposed | Requirements R10, R13; KTD5, KTD6; Acceptance AE7 | R10, R13 | U2 | unassigned |
| `packing-reserve-is-computed-once-after-wrapper-and-disclosure` | `read-side/` | proposed | Requirements R10; KTD5, KTD6; Acceptance AE7 | R10 | U2 | unassigned |
| `packing-required-phase-is-a-prefix-ending-at-first-overbudget` | `read-side/` | proposed | Requirements R10; KTD5, KTD6; Acceptance AE7 | R10 | U2 | unassigned |
| `packing-first-rule-is-served-when-it-fits-budget-minus-wrapper` | `read-side/` | proposed | Requirements R10; KTD5, KTD6; Acceptance AE7 | R10 | U2 | unassigned |
| `packing-omitted-count-is-disclosed-reported-and-digested` | `read-side/` | proposed | Requirements R11; KTD5, KTD6; Acceptance AE7 | R11 | U2 | unassigned |
| `packing-user-directive-renders-under-user-directives-by-source-kind` | `read-side/` | proposed | Requirements R1, R12; KTD5, KTD6; Acceptance AE7 | R1, R12 | U2 | unassigned |
| `parity-directive-command-dispatch-is-host-user-only` | `read-side/` | proposed | Requirements R1; KTD7, KTD8; Acceptance AE1, AE8 | R1 | U4 | unassigned |
| `parity-directive-command-writes-only-after-ui-confirmation` | `read-side/` | proposed | Requirements R1; KTD7, KTD8; Acceptance AE1, AE8 | R1 | U4 | unassigned |
| `parity-identical-request-fields-yield-identical-rule-sets` | `read-side/` | proposed | Requirements R13; KTD7, KTD8; Acceptance AE1, AE8 | R13 | U6 | unassigned |
| `parity-pi-context-handler-registers-whenever-project-binds` | `read-side/` | proposed | Requirements R13; KTD7, KTD8; Acceptance AE1, AE8 | R13 | U6 | unassigned |
| `parity-is-subagent-true-suppresses-m0-and-pi-sends-false` | `read-side/` | proposed | Requirements R13; KTD7, KTD8; Acceptance AE1, AE8 | R13 | U6 | unassigned |

## Reused records from other catalogs

| Slug | Catalog | Why it applies here |
| --- | --- | --- |
| `bound-project-scope-cannot-be-widened-by-candidate` | `canonical-positive-claim-projection/` | the mandatory Project term that every rule scope keeps (R5) |
| `retrieved-content-cannot-upgrade-write-authority` | `canonical-positive-claim-projection/` | record content grants no class; a directive look-alike stays `model_inference` (R3) |
| `supporting-authority-is-preserved-not-recomputed` | `canonical-positive-claim-projection/` | the stored approval the cascade demotes on revoke (R7) |
| `stale-projection-cannot-authorize-current-use` | `canonical-positive-claim-projection/` | a revoked directive is denied at the next read (R7) |
| `future-correction-keeps-kernel-maturity-and-retrieval` | `evidence-backed-memory-review/` | a review-path revise inherits no maturity (R3) |
| `packing-required-phase-precedes-optional-work` | `selection-packing/required/` | the required stage KTD5 charges rules through (R10) |
| `packing-budget-is-an-integer-never-clamped` | `selection-packing/required/` | integer budget; the memory trim clamp is a precedent not to inherit (R10) |
| `packing-required-bytes-are-charged-untruncated` | `selection-packing/required/` | exactly-at-limit succeeds, one over fails (R10) |
| `packing-optional-scan-skips-and-continues` | `selection-packing/grouping/` | the optional phase rule (R10) |
| `canonical-read-staleness-is-distinguishable-from-emptiness` | `daemon/transform/` | withheld read composes no block; `memory_enabled` gating (R12) |
| `dec-a-memory-injection-budget-documented-range-has-no-implementing-code` | `daemon/decisions/` | the 4,000 token default and its clamp (R10) |
| `cf-memory-credit-requires-admitted-visible-content` | `compression-fidelity/delivery/` | memory credit requires an included object from the pinned read (R12) |
