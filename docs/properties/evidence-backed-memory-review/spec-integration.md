# Specification integration

Crosswalk from each stable slug to the specification section that motivates
it, the implementation ticket that owns it, and the code that carries it.
Ticket numbers here are tracking metadata, not names of anything in the tree.

| Slug | Specification section | Ticket | Code |
| --- | --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | Stop conditions; KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`resolve_descriptor`), `crates/daemon/src/memory_reviewer/selection.rs` (`PRODUCTION_SELECTION_OPEN`) |
| `canonical-resolution-refuses-changed-owner-and-target` | KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`proposal_target`), `crates/daemon/src/memory_reviewer/broker.rs` (`originating_decision`, `judge_canonical_source`) |
| `private-result-transfer-preserves-queue-expiry` | Constraints (Q27); U1 acceptance: crash between private Kernel result or hold transfer and receipt selection | #726 | `crates/kernel/src/memory_reviewer_hold.rs` (`transfer_execution_to_review`, `list_active_review_holds`), `crates/kernel/src/review_staging.rs` (`read_selected_review_input`), `crates/memory-store/src/memory_reviewer_jobs.rs` (`expire_memory_reviewer_work`), `crates/daemon/src/memory_reviewer/settlement.rs` (`read_selected_proposal`), `crates/daemon/src/memory_reviewer/lifecycle.rs` (`reconcile_review_holds`) |
| `receipt-selection-fences-private-generation-results` | Constraints (KTD8, Q26); U1 acceptance | #727 | `crates/daemon/src/memory_reviewer/settlement.rs` (`adopt`, `read_selected_proposal`) |
| `durable-private-result-recovers-without-model-refire` | Constraints (Q15, OQ13); U1 acceptance | #727 | `crates/kernel/src/review_staging.rs` (`ReviewDependencies`), `crates/context-core/src/memory_reviewer_policy_union.rs` (`decode`), `crates/daemon/src/memory_reviewer/dependencies.rs`, `crates/daemon/src/memory_reviewer/settlement.rs` (`adopt`) |
| `unknown-dispatch-does-not-authorize-resend` | Constraints (Q14, Q20); U1 acceptance | #727 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`resumes_dispatched_work`, `prepare`) |
| `uncited-owner-lineage-remains-read-authority` | R6; U1 acceptance | #727 | `crates/daemon/src/memory_reviewer/dependencies.rs` (`revalidate`), `crates/daemon/src/memory_reviewer/settlement.rs` (`read_selected_proposal`) |
| `observer-route-does-not-change-background-rosters` | U3 acceptance; OQ8 | #728 | `crates/daemon/src/lib.rs` (`RouteBindings::participating`, `latest_per_root`, `latest_for_root`) |
| `status-sanitizer-preserves-inclusive-integer-domain` | U3 acceptance; OQ9 | #729 | `packages/opencode-plugin/src/shared/host-client/exact-json.ts`, `connection.ts` (`consumeJson`), `client.ts` (`awaitRequest`, `routeOpen`, `hostStatus`) |
