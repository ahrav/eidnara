//! Emit the golden vectors for the storage descriptor and its derivation helpers.
//! `tests/golden/storage_vectors.json` is a byte-stable fixture: `tests/golden_vectors.rs`
//! asserts against these exact values, so a database name or store path cannot drift
//! without a reviewed fixture change. The module ids below are fixture content.
//!
//! Run: `cargo run -p storage-types --example golden-vectors`

use storage_types::{StorageDescriptor, postgres_database_name, sqlite_store_path};

fn main() {
    // Slug collisions exercise the hash suffix; overlong IDs exercise the slug bound.
    let ids = [
        "module-a",
        "module-b",
        "module-c",
        "module-d",
        "a-b",
        "a_b",
        "a-very-long-module-id-that-exceeds-the-postgres-identifier-byte-limit-by-a-lot",
    ];
    let data_home = "/data";

    let vectors: Vec<_> = ids
        .iter()
        .map(|id| {
            let descriptor = StorageDescriptor {
                module_id: (*id).to_string(),
                storage_namespace: "default".to_string(),
                isolation: storage_types::Isolation::Module,
                backend: storage_types::StorageBackend::Sqlite {
                    path: sqlite_store_path(data_home, id),
                },
            };
            serde_json::json!({
                "module_id": id,
                "postgres_database_name": postgres_database_name(id),
                "sqlite_store_path": sqlite_store_path(data_home, id),
                "sqlite_descriptor": descriptor,
            })
        })
        .collect();

    let doc = serde_json::json!({
        "data_home": data_home,
        "vectors": vectors,
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
