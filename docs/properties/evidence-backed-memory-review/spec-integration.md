# Specification integration

Crosswalk from each stable slug to the specification section that motivates
it, the implementation ticket that owns it, and the code that carries it.
Ticket numbers here are tracking metadata, not names of anything in the tree.

| Slug | Specification section | Ticket | Code |
| --- | --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | Stop conditions; KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`resolve_descriptor`), `crates/daemon/src/memory_reviewer/selection.rs` (`PRODUCTION_SELECTION_OPEN`) |
| `canonical-resolution-refuses-changed-owner-and-target` | KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`proposal_target`), `crates/daemon/src/memory_reviewer/broker.rs` (`originating_decision`, `judge_canonical_source`) |
| `private-result-transfer-preserves-queue-expiry` | Constraints (Q27); U1 acceptance: crash between private Kernel result or hold transfer and receipt selection | #726 | `crates/kernel/src/memory_reviewer_hold.rs` (`transfer_execution_to_review`, `list_active_review_holds`), `crates/kernel/src/review_staging.rs` (`read_selected_review_input`), `crates/memory-store/src/memory_reviewer_jobs.rs` (`expire_memory_reviewer_work`), `crates/daemon/src/memory_reviewer/settlement.rs` (`read_selected_proposal`), `crates/daemon/src/memory_reviewer/lifecycle.rs` (`reconcile_review_holds`) |
