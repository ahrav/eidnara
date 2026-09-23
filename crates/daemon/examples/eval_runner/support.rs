//! The fixture and surface helpers the campaign shell shares with the
//! daemon's evaluator tests, included from their one source.

#[path = "../../tests/support/direct_host.rs"]
pub mod direct_host;
#[path = "../../tests/support/embedding_fixtures.rs"]
pub mod embedding_fixtures;
#[path = "../../tests/support/eval_surface.rs"]
pub mod eval_surface;
#[path = "../../tests/support/publish.rs"]
pub mod publish;
