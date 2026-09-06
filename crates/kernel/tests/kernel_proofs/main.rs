//! This binary runs the kernel replay/repair correctness proofs. commentlint: allow(JUDGE)
//!
//! Modules: `harness` (proof harness), `model` (randomized operation model), commentlint: allow(JUDGE)
//! `canonical_state_proofs` (negative controls for the shared canonical-state digest), commentlint: allow(JUDGE)
//! `fixtures` (spec builders), `obligations` (per-obligation proofs). commentlint: allow(JUDGE)
//!
//! The obligation numbering runs O1 through O10 and skips O4; this binary does not
//! prove O4. commentlint: allow(JUDGE)
//!
//! Every submodule is declared here so the binary links once; code other test
//! binaries share lives under `tests/support/` and is `#[path]`-included.
//!
//! Assertions report proof failures by panicking. Repository and store fixtures use isolated
//! test state. This crate exports no production API.

#![cfg(feature = "test-support")]

#[path = "../support/canonical_state.rs"]
mod canonical_state;
#[path = "../support/git_fixtures.rs"]
mod git_fixtures;

mod canonical_state_proofs;
mod fixtures;
mod harness;
mod model;
mod obligations;
