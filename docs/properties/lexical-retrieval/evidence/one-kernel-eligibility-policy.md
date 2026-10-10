# one-kernel-eligibility-policy

- `crates/daemon/tests/claim_eligibility.rs:184` compares the retrieval
  adapter, the daemon route, and the kernel at one snapshot.
- `:456` scans every workspace crate's `src` except `kernel` for
  `.judge_eligibility` and `::judge_eligibility`, allows only
  `daemon/src/kernel_routes/eligibility.rs`, `daemon/src/embedding_dispatch.rs`,
  and `retrieval/src/eligibility.rs`, requires both adapters to call them,
  checks that retrieval's `[dependencies]` name `kernel` and not `daemon`, and
  checks that the kernel's `judge` stays private
  (`crates/kernel/src/eligibility.rs:198`).
