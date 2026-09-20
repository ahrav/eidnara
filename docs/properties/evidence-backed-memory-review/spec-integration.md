# Specification integration

Crosswalk from each stable slug to the specification section that motivates
it, the implementation ticket that owns it, and the code that carries it.
Ticket numbers here are tracking metadata, not names of anything in the tree.

| Slug | Specification section | Ticket | Code |
| --- | --- | --- | --- |
| `production-classes-reach-policy-eligible-proposal` | Stop conditions; KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`resolve_descriptor`), `crates/daemon/src/memory_reviewer/selection.rs` (`PRODUCTION_SELECTION_OPEN`) |
| `canonical-resolution-refuses-changed-owner-and-target` | KTD7; U6 acceptance | #725 | `crates/daemon/src/memory_reviewer/coordinator.rs` (`proposal_target`), `crates/daemon/src/memory_reviewer/broker.rs` (`originating_decision`, `judge_canonical_source`) |
