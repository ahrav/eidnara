#![allow(dead_code)]

pub mod applied;
pub mod dense_projection;
#[cfg(unix)]
pub mod direct_host;
pub mod embedding_fixtures;
#[cfg(feature = "test-support")]
pub mod eval_cassette;
#[cfg(feature = "test-support")]
pub mod eval_ledger;
#[cfg(feature = "test-support")]
pub mod eval_reviewer_peer;
#[cfg(all(unix, feature = "test-support"))]
pub mod eval_surface;
pub mod flock;
pub mod git_repo;
pub mod kernel_daemon;
pub mod memory_reviewer_corpus;
pub mod memory_reviewer_publish;
pub mod packing;
pub mod projection_gate;
pub mod query_route;
pub mod tls_peer;
pub mod vector_reads;
pub mod vector_store;
