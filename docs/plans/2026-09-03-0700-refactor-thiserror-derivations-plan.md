---
title: Replace Manual Error Derivations - Plan
type: refactor
date: 2026-09-03
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# Replace Manual Error Derivations - Plan

## Goal Capsule

- **Objective:** Error types remain behaviorally identical while their formatting and source-chain contracts become easier to maintain.
- **Means:** Derive `thiserror::Error` for custom error types that manually implement both `Display` and `Error` (KTD1).
- **Authority:** Existing display text, source chains, tests, and public type shapes take precedence over macro convenience.
- **Execution profile:** Make the change in a new worktree based on `u3/cleanup-3-harness-closure-fs-policy`.
- **Stop conditions:** Stop if a type cannot preserve its exact display or source behavior with `thiserror` attributes without changing its public representation.
- **Tail ownership:** The implementer owns formatting, lint, test, documentation, and repository checks before shipping.

## Product Contract

### Summary

Replace manual `Display` and `Error` implementations in `host-runtime` and `shm-transport` with the workspace's existing `thiserror` dependency. Preserve all observable behavior and leave display-only value types unchanged.

### Problem Frame

The two wave-U3 crates contain 28 custom error types across 23 files with handwritten formatting and error implementations. This repeats boilerplate already handled by `thiserror`, which the workspace uses in existing crates.

### Requirements

- R1. Every custom type that manually implements `std::error::Error` and `Display` in `host-runtime` or `shm-transport` uses `thiserror::Error` instead.
- R2. Every formatted error string remains byte-for-byte compatible for the same value.
- R3. Every existing `Error::source` result remains equivalent, including variants that intentionally expose no source.
- R4. Public type names, variants, fields, construction sites, conversion behavior, and `Debug` output remain unchanged. A conversion attribute may replace a manual conversion only when it also preserves `Error::source` behavior.
- R5. Types that implement `Display` but not `Error`, including `SendOutcome` and `CleanupFailure`, remain manually formatted and outside this refactor.
- R6. Both affected crates declare the existing workspace `thiserror` dependency without introducing another error library or changing its workspace version.

### Scope Boundaries

- Do not redesign the error taxonomy or alter messages.
- Do not replace error types with opaque wrappers.
- Do not migrate unrelated manual formatting implementations.
- Refresh only the `Cargo.lock` hash owned by `migration/waves/U2/receipt.json`; do not change unrelated receipt entries, registry families, or impact records.

## Planning Contract

### Key Technical Decisions

- KTD1. **Use `#[derive(thiserror::Error)]` on existing error types.** The workspace already pins `thiserror = "2"`, so this removes boilerplate without adding a new dependency family.
- KTD2. **Translate formatting literally.** Use `#[error(...)]` attributes that preserve each current match arm's text and field formatting rather than simplifying messages.
- KTD3. **Preserve source edges explicitly.** `thiserror` treats fields named `source` as sources, and `#[from]` and `#[error(transparent)]` also add source edges. Do not use those forms on a variant whose current `source()` returns `None`.
- KTD4. **Keep display-only types manual.** `thiserror::Error` is for error contracts, not a replacement for every `Display` implementation.

### Assumptions

- `u3/cleanup-3-harness-closure-fs-policy` is the intended base because it contains the wave-U3 snapshot plus the completed cleanup stack, while the current worktree has uncommitted work that must stay isolated.
- The selected base has pre-existing `check:repo` failures. Capture them before editing and require the final run to introduce no additional failure.

## Implementation Units

### U1. Migrate shared-memory transport errors

- **Goal:** Replace manual error boilerplate in `shm-transport` while preserving formatting and nested source chains.
- **Requirements:** R1, R2, R3, R4, R6
- **Dependencies:** None
- **Files:**
  - `crates/shm-transport/Cargo.toml`
  - `crates/shm-transport/src/arena.rs`
  - `crates/shm-transport/src/backend/ring.rs`
  - `crates/shm-transport/src/descriptor.rs`
  - `crates/shm-transport/src/lease.rs`
  - `crates/shm-transport/src/lifecycle.rs`
  - `crates/shm-transport/src/profile.rs`
  - Inline test modules in the listed source files
  - `Cargo.lock`
- **Approach:** Add the workspace dependency. Add characterization assertions before replacing the implementations. Derive `Error` for `ArenaError`, `ProducerError`, `RingError`, `DescriptorError`, `LeaseError`, `LifecycleError`, `ProfileError`, and `AdmissionError`. Preserve the existing nested sources on `ProducerError` and `RingError`; all other listed error types keep empty source chains. Keep each type's manual `Debug` implementation because it intentionally delegates to `Display`.
- **Patterns to follow:** Existing `thiserror` derives in `crates/lease/src/lib.rs` and `crates/storage/src/lib.rs`.
- **Test scenarios:**
  - Construct every variant and confirm its formatted text and `Debug` text are unchanged.
  - Confirm `ProducerError::Arena`, `ProducerError::Ring`, `RingError::Descriptor`, and `RingError::Lease` expose the same nested source as before.
  - Confirm variants without an existing source still return `None`.
- **Verification:** The crate compiles with all features, its unit and integration tests pass, and no manual `Error` implementation remains in its production source.

### U2. Migrate host runtime errors

- **Goal:** Replace manual error boilerplate in `host-runtime`, including its test support error, without changing public behavior.
- **Requirements:** R1, R2, R3, R4, R5, R6
- **Dependencies:** U1
- **Files:**
  - `crates/host-runtime/Cargo.toml`
  - `crates/host-runtime/src/auth.rs`
  - `crates/host-runtime/src/broca/subprocess.rs`
  - `crates/host-runtime/src/client.rs`
  - `crates/host-runtime/src/composite.rs`
  - `crates/host-runtime/src/config.rs`
  - `crates/host-runtime/src/connection_file.rs`
  - `crates/host-runtime/src/generation.rs`
  - `crates/host-runtime/src/handler.rs`
  - `crates/host-runtime/src/harness_closure.rs`
  - `crates/host-runtime/src/instance.rs`
  - `crates/host-runtime/src/ring_transport.rs`
  - `crates/host-runtime/src/runtime.rs`
  - `crates/host-runtime/src/setup_socket.rs`
  - `crates/host-runtime/src/synapse/bundle.rs`
  - `crates/host-runtime/src/synapse/inference.rs`
  - `crates/host-runtime/src/wire.rs`
  - `crates/host-runtime/tests/support/process_resources.rs`
  - `Cargo.lock`
- **Approach:** Add the workspace dependency. Add characterization assertions before replacing the implementations. Derive `Error` for the 19 production error types and `ObserveError`. Preserve explicit source edges in `AuthError`, `ConnectionFileError`, `InstanceError`, and `SetupError`. Keep `client::SendOutcome` and `CleanupFailure` as display-only types.
- **Patterns to follow:** U1 and existing workspace `thiserror` derives.
- **Test scenarios:**
  - Construct every unit, tuple, and struct variant and confirm exact formatted output, including path, identifier, count, and nested-error interpolation.
  - Confirm I/O and JSON variants expose the same source values in `AuthError` and `ConnectionFileError`.
  - Confirm the I/O variants in `InstanceError` and `SetupError` preserve their sources.
  - Confirm source-less wrappers such as `CallError`, `ClientError`, `HostError`, `GenerationError::Instance`, and `AuthError::Random` remain source-less.
  - Confirm `client::SendOutcome` and `CleanupFailure` retain their existing manual `Display` behavior and do not implement `Error` through this change.
- **Verification:** The crate compiles with all features, all host-runtime tests pass, and a focused search finds no manual `Error` implementations in the migrated files.

### U3. Run repository compatibility gates

- **Goal:** Prove the macro migration is formatting-clean, lint-clean, and behavior-preserving across the workspace.
- **Requirements:** R2, R3, R4
- **Dependencies:** U1, U2
- **Files:**
  - `migration/waves/U2/receipt.json`
- **Approach:** Refresh the receipt hash for the changed lockfile. Format the workspace, run Clippy and tests with all features, build documentation with warnings denied, run the no-default-features check, and compare repository-check output with a baseline captured from the selected base.
- **Test expectation:** U1 and U2 add the characterization assertions required to prove formatting, `Debug`, and source-chain parity.
- **Verification:** Every Cargo command in the Verification Contract passes. Repository checks introduce no failure absent from the selected-base baseline.

## Verification Contract

- `bun install --frozen-lockfile`
- Capture `bun run check:repo` output on the selected base, then require the final run to add no new failure.
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --all-targets --all-features --locked`
- `cargo test --workspace --doc --all-features --locked`
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked`
- `cargo check --workspace --no-default-features --locked`
- Confirm no manual `impl std::error::Error for` or equivalent remains in the migrated production or test-support files.

## Definition of Done

- Both affected manifests use the workspace `thiserror` dependency.
- All 28 targeted error types derive `thiserror::Error`.
- Exact `Display`, `Debug`, and source-chain behavior remain unchanged.
- Display-only types remain outside the migration.
- Cargo gates pass and repository checks show no regression from the selected-base baseline.
- The final diff contains no abandoned experiments or unrelated current-worktree changes.
