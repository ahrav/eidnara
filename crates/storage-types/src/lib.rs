//! Shared storage descriptor and migration types.
//!
//! A host resolves its central storage configuration into one
//! [`StorageDescriptor`] per module and delivers that descriptor to the module.
//! The module hands the descriptor to the `storage` crate, which opens the
//! actual database.
//!
//! This crate stays dependency-light (serde only, no database driver) so a
//! wire crate that carries the descriptor can depend on it without pulling
//! SQLite into a thin daemon. The heavier `storage` crate re-exports these
//! types and provides the open/migrate mechanics.
//!
//! ## Design invariants
//!
//! - Backend variants are additive; module code hands descriptors to
//!   `storage` instead of branching on the backend.
//! - Database **isolation** is explicit, never derived from naming conventions,
//!   so descriptor semantics do not depend on how database names are built.
//! - The descriptor a module receives is fully **resolved and least-privilege**:
//!   it never carries central config or an admin credential. For postgres the DSN
//!   reaches only the module's own database.

use serde::{Deserialize, Serialize};

/// How many physical databases a module's storage spans.
///
/// `Isolation` is explicit rather than inferred from a database name, so
/// descriptor semantics do not depend on database naming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Isolation {
    /// One database for the whole module. A project-scoped module partitions its
    /// own rows internally (e.g. by a project key); it does not get a separate
    /// database per project.
    Module,
}

/// The backend a module's storage runs on.
///
/// Backend variants preserve existing descriptor meanings. Module code delegates
/// backend handling to `storage`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "backend")]
pub enum StorageBackend {
    /// A local sqlite file at `path` (absolute).
    Sqlite { path: String },
    /// A postgres database. `dsn` is a scoped, least-privilege runtime DSN that
    /// reaches only `database` (never an admin or `CREATEDB` DSN). No backend in
    /// this workspace opens this variant; `storage::open_sqlite` rejects it.
    Postgres { dsn: String, database: String },
}

/// `dsn` carries a credential, so `Debug` redacts it: any log line, panic
/// message, or assertion failure that formats a descriptor would otherwise
/// write the password out verbatim.
impl std::fmt::Debug for StorageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageBackend::Sqlite { path } => {
                f.debug_struct("Sqlite").field("path", path).finish()
            }
            StorageBackend::Postgres { database, .. } => f
                .debug_struct("Postgres")
                .field("dsn", &"<redacted>")
                .field("database", database)
                .finish(),
        }
    }
}

impl StorageBackend {
    /// A short, stable backend label used in lease-key namespacing and diagnostics
    /// (so the same logical scope under two backends maps to distinct locks).
    pub fn label(&self) -> &'static str {
        match self {
            StorageBackend::Sqlite { .. } => "sqlite",
            StorageBackend::Postgres { .. } => "postgres",
        }
    }
}

/// The resolved storage handle a host delivers to a module. The module passes
/// this to `storage` to open its database; it never sees central config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageDescriptor {
    /// The module this storage belongs to. Part of lease-key namespacing so two
    /// modules sharing a lease root cannot collide.
    pub module_id: String,
    /// A stable namespace for this module's storage, independent of backend
    /// naming. Used (with `module_id` and the backend label) to derive the
    /// single-writer lease key.
    pub storage_namespace: String,
    /// How many physical databases this storage spans.
    pub isolation: Isolation,
    pub backend: StorageBackend,
}

/// Build the per-module postgres database name: `eidnara_<slug>_<16hex>`.
///
/// The 16-hex suffix hashes `module_id`; `a-b` and `a_b` generate different names.
/// The 36-character slug limit keeps generated names within 63 bytes.
pub fn postgres_database_name(module_id: &str) -> String {
    const MAX_SLUG: usize = 36; // 63 - len("eidnara_") - len("_") - 16
    let slug: String = module_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(MAX_SLUG)
        .collect();
    format!("eidnara_{slug}_{}", fnv1a_hex(module_id))
}

/// The conventional SQLite store path under a data-home root:
/// `<data_home>/eidnara/<module_id>/store.db`. Trailing `/` characters in
/// `data_home` do not produce duplicate separators.
///
/// # Panics
///
/// Panics when `data_home` is empty, because the composed path would then start
/// at the filesystem root instead of the configured location. Panics when
/// `module_id` is not a single path component: empty, `.`, `..`, or containing
/// `/` or `\`. A module id must be one path component so each module maps to
/// exactly one directory under `<data_home>/eidnara/`. Rejecting rather than
/// encoding keeps every valid id's path byte-stable.
pub fn sqlite_store_path(data_home: &str, module_id: &str) -> String {
    assert!(!data_home.is_empty(), "data_home must not be empty");
    assert!(
        is_single_path_component(module_id),
        "module_id {module_id:?} is not a single path component"
    );
    format!(
        "{}/eidnara/{}/store.db",
        data_home.trim_end_matches('/'),
        module_id
    )
}

fn is_single_path_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// FNV-1a 64-bit, hex: a dependency-free deterministic hash for name disambiguation.
fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_collision_is_broken_by_the_hash() {
        let a = postgres_database_name("a-b");
        let b = postgres_database_name("a_b");
        assert_ne!(a, b, "distinct module ids must not share a database name");
        assert!(a.starts_with("eidnara_a_b_"));
        assert!(b.starts_with("eidnara_a_b_"));
    }

    #[test]
    fn database_name_fits_postgres_identifier_limit() {
        let long = "a-very-long-module-id-that-exceeds-the-postgres-identifier-byte-limit-by-a-lot";
        let name = postgres_database_name(long);
        assert!(name.len() <= 63, "db name {} is {} bytes", name, name.len());
    }

    #[test]
    fn sqlite_path_follows_convention() {
        assert_eq!(
            sqlite_store_path("/home/u/.local/share", "module-a"),
            "/home/u/.local/share/eidnara/module-a/store.db"
        );
        assert_eq!(sqlite_store_path("/data/", "m"), "/data/eidnara/m/store.db");
    }

    /// An empty data home would root the store at `/eidnara/` instead of the
    /// configured location.
    #[test]
    fn sqlite_path_rejects_an_empty_data_home() {
        let outcome = std::panic::catch_unwind(|| sqlite_store_path("", "module-a"));
        assert!(outcome.is_err(), "an empty data_home must be rejected");
    }

    /// A module id carrying a path component resolves outside `<data_home>/eidnara/`.
    #[test]
    fn sqlite_path_rejects_module_ids_that_escape_the_store_root() {
        for bad in ["../other", "a/b", "a\\b", "..", ".", ""] {
            let outcome = std::panic::catch_unwind(|| sqlite_store_path("/data", bad));
            assert!(outcome.is_err(), "module_id {bad:?} must be rejected");
        }
        // A dot inside a name is an ordinary character, not a traversal.
        assert_eq!(
            sqlite_store_path("/data", "a.b"),
            "/data/eidnara/a.b/store.db"
        );
    }

    // Descriptor wire shape is a contract; field or tag changes require updating
    // this golden JSON.
    #[test]
    fn sqlite_descriptor_golden_json() {
        let d = StorageDescriptor {
            module_id: "module-a".into(),
            storage_namespace: "route-state".into(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: "/data/eidnara/module-a/store.db".into(),
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(
            json,
            r#"{"module_id":"module-a","storage_namespace":"route-state","isolation":{"kind":"module"},"backend":{"backend":"sqlite","path":"/data/eidnara/module-a/store.db"}}"#
        );
        let back: StorageDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn postgres_descriptor_golden_json() {
        let d = StorageDescriptor {
            module_id: "module-a".into(),
            storage_namespace: "route-state".into(),
            isolation: Isolation::Module,
            backend: StorageBackend::Postgres {
                dsn: "postgres://routing:scoped@localhost/eidnara_module_a_0badc0de".into(),
                database: "eidnara_module_a_0badc0de".into(),
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: StorageDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    /// The debug form of a descriptor reaches logs and panic messages, so the
    /// DSN credential must never appear in it.
    #[test]
    fn debug_output_redacts_the_postgres_dsn() {
        let d = StorageDescriptor {
            module_id: "module-a".into(),
            storage_namespace: "route-state".into(),
            isolation: Isolation::Module,
            backend: StorageBackend::Postgres {
                dsn: "postgres://routing:hunter2@localhost/db".into(),
                database: "db".into(),
            },
        };
        let debug = format!("{d:?}");
        assert!(!debug.contains("hunter2"), "credential leaked: {debug}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("db"), "non-sensitive fields stay visible");
    }

    #[test]
    fn backend_label_is_stable() {
        assert_eq!(
            StorageBackend::Sqlite { path: "x".into() }.label(),
            "sqlite"
        );
        assert_eq!(
            StorageBackend::Postgres {
                dsn: "x".into(),
                database: "y".into()
            }
            .label(),
            "postgres"
        );
    }
}
