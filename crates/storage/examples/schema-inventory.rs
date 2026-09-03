//! Generates `fixtures/schema/storage-inventory-v1.json` from a fresh SQLite store.
//!
//! Run: `cargo run -p storage --example schema-inventory`

use storage::{
    APPLICATION_ID, Isolation, StorageBackend, StorageDescriptor, USER_VERSION, open_sqlite,
    schema_inventory,
};

fn main() {
    let dir = std::env::temp_dir().join(format!("storage-inventory-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    let path = dir.join("store.db");
    let descriptor = StorageDescriptor {
        module_id: "inventory".to_string(),
        storage_namespace: "inventory".to_string(),
        isolation: Isolation::Module,
        backend: StorageBackend::Sqlite {
            path: path.to_string_lossy().into_owned(),
        },
    };
    let store = open_sqlite(&descriptor, "").expect("open a fresh store");
    let digest: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT baseline_sha256 FROM format_marker WHERE id = 0",
                [],
                |row| row.get(0),
            )
        })
        .expect("read the format marker");
    drop(store);
    let scratch = rusqlite::Connection::open(&path).expect("reopen the fresh file");
    let objects = schema_inventory(&scratch).expect("read the schema inventory");
    drop(scratch);
    let _ = std::fs::remove_dir_all(&dir);

    let doc = serde_json::json!({
        "application_id": APPLICATION_ID,
        "user_version": USER_VERSION,
        "baseline_sha256": digest,
        "objects": objects.iter().map(|o| serde_json::json!({
            "type": o.kind,
            "name": o.name,
            "tbl_name": o.table,
            "sql": o.sql,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
