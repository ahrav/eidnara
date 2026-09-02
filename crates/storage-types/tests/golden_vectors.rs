//! `tests/golden/storage_vectors.json` pins database names, store paths, and
//! descriptor wire shapes. It is byte-stable: regenerate it with
//! `cargo run -p storage-types --example golden-vectors` only as part of a
//! reviewed contract change.

use storage_types::{postgres_database_name, sqlite_store_path, StorageDescriptor};
use serde_json::Value;

const VECTORS: &str = include_str!("golden/storage_vectors.json");

#[test]
fn helpers_reproduce_the_golden_vectors() {
    let doc: Value = serde_json::from_str(VECTORS).expect("parse golden vectors");
    let data_home = doc["data_home"].as_str().expect("data_home");
    let vectors = doc["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "golden vectors must not be empty");

    for v in vectors {
        let id = v["module_id"].as_str().expect("module_id");

        assert_eq!(
            postgres_database_name(id),
            v["postgres_database_name"].as_str().unwrap(),
            "postgres_database_name drift for module_id {id}"
        );
        assert_eq!(
            sqlite_store_path(data_home, id),
            v["sqlite_store_path"].as_str().unwrap(),
            "sqlite_store_path drift for module_id {id}"
        );

        // Serde field names and enum tags define the wire contract; serialization
        // must preserve the fixture after parsing.
        let descriptor: StorageDescriptor =
            serde_json::from_value(v["sqlite_descriptor"].clone()).expect("descriptor parses");
        let reserialized = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(
            reserialized, v["sqlite_descriptor"],
            "descriptor shape drift for module_id {id}"
        );
    }
}

#[test]
fn golden_vectors_break_slug_collisions() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let by_id: std::collections::HashMap<&str, &str> = doc["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["module_id"].as_str().unwrap(),
                v["postgres_database_name"].as_str().unwrap(),
            )
        })
        .collect();
    let a = by_id.get("a-b").expect("fixture has a-b");
    let b = by_id.get("a_b").expect("fixture has a_b");
    assert_ne!(a, b, "a-b and a_b must map to distinct database names");
}
