//! The frozen field inventory in docs/properties/search-projection/projection-schema.md
//! is read independently of the baseline SQL and compared with what a freshly
//! opened store actually contains.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Column {
    name: String,
    kind: String,
    not_null: bool,
    primary_key: bool,
    /// Column-level constraints other than NOT NULL and PRIMARY KEY,
    /// whitespace-normalized.
    constraint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Table {
    strict: bool,
    columns: Vec<Column>,
    /// Table-level constraints and index definitions, whitespace-normalized.
    constraints: Vec<String>,
    indexes: Vec<(String, String)>,
}

fn normalize(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The inventory the document freezes.
fn documented() -> BTreeMap<String, Table> {
    let text = std::fs::read_to_string(
        repo_root().join("docs/properties/search-projection/projection-schema.md"),
    )
    .unwrap();
    assert!(text.contains("Every table below is `STRICT`."));
    let mut tables: BTreeMap<String, Table> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## `") {
            let name = name.trim_end_matches('`').to_string();
            tables.insert(
                name.clone(),
                Table {
                    strict: true,
                    ..Table::default()
                },
            );
            current = Some(name);
            section = "";
            continue;
        }
        let Some(table) = current.as_ref().and_then(|name| tables.get_mut(name)) else {
            continue;
        };
        if line.starts_with("Table constraints:") {
            section = "constraints";
            continue;
        }
        if line.starts_with("Indexes:") {
            section = "indexes";
            continue;
        }
        if line.starts_with("| `") {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 5, "{line}");
            table.columns.push(Column {
                name: cells[0].trim_matches('`').to_string(),
                kind: cells[1].to_string(),
                not_null: cells[2] == "yes",
                primary_key: cells[3] == "yes",
                constraint: normalize(cells[4].trim_matches('`')),
            });
        } else if let Some(item) = line.strip_prefix("- `") {
            let item = item.trim_end_matches('`');
            match section {
                "constraints" => table.constraints.push(normalize(item)),
                "indexes" => {
                    let (name, definition) = item.split_once("` on `").unwrap();
                    let (definition, trailer) =
                        definition.split_once('`').unwrap_or((definition, ""));
                    let unique = trailer.contains("(unique");
                    table.indexes.push((
                        name.to_string(),
                        format!(
                            "{}{}",
                            if unique { "UNIQUE " } else { "" },
                            normalize(definition)
                        ),
                    ));
                }
                _ => panic!("list item outside a section: {line}"),
            }
        }
    }
    tables
}

fn open(dir: &Path, baseline: &str) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir
                    .join("search")
                    .join("search.sqlite")
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        baseline,
    )
    .unwrap()
}

/// Reads the store inventory through SQLite's table metadata and schema SQL.
/// Storage infrastructure tables are outside the projection inventory.
fn stored(baseline: &str) -> BTreeMap<String, Table> {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), baseline);
    store
        .with_conn_unfenced(|conn| {
            let mut tables: BTreeMap<String, Table> = BTreeMap::new();
            let mut names = conn.prepare(
                "SELECT name,sql FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?;
            let rows = names
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (name, sql) in rows {
                if storage::INFRASTRUCTURE_TABLES.contains(&name.as_str()) {
                    continue;
                }
                // Table-level constraints are the body items that name no column.
                let body = sql[sql.find('(').unwrap() + 1..sql.rfind(')').unwrap()].to_string();
                let mut parts = Vec::new();
                let mut depth = 0;
                let mut current = String::new();
                for ch in body.chars() {
                    match ch {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        ',' if depth == 0 => {
                            parts.push(std::mem::take(&mut current));
                            continue;
                        }
                        _ => {}
                    }
                    current.push(ch);
                }
                parts.push(current);
                let parts: Vec<String> = parts.iter().map(|part| normalize(part)).collect();
                let is_table_constraint = |part: &str| {
                    part.starts_with("CHECK(")
                        || part.starts_with("PRIMARY KEY(")
                        || part.starts_with("UNIQUE(")
                };
                let constraints = parts
                    .iter()
                    .filter(|part| is_table_constraint(part))
                    .cloned()
                    .collect();
                let declared_constraints: BTreeMap<String, String> = parts
                    .iter()
                    .filter(|part| !is_table_constraint(part))
                    .map(|part| {
                        let mut words = part.splitn(3, ' ');
                        let column = words.next().unwrap().to_string();
                        let _kind = words.next();
                        let rest = words.next().unwrap_or("");
                        let rest = rest
                            .replace("NOT NULL", "")
                            .replace("PRIMARY KEY", "");
                        (column, normalize(&rest))
                    })
                    .collect();
                let mut info = conn.prepare(&format!("PRAGMA table_info({name})"))?;
                let columns = info
                    .query_map([], |row| {
                        let column: String = row.get(1)?;
                        Ok(Column {
                            constraint: declared_constraints
                                .get(&column)
                                .cloned()
                                .unwrap_or_default(),
                            name: column,
                            kind: row.get(2)?,
                            not_null: row.get::<_, i64>(3)? != 0,
                            primary_key: row.get::<_, i64>(5)? != 0,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut index_rows = conn.prepare(
                    "SELECT name,sql FROM sqlite_schema WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL ORDER BY name",
                )?;
                let indexes = index_rows
                    .query_map([&name], |row| {
                        let sql: String = row.get(1)?;
                        // Everything after the table name: the column list and,
                        // for a partial index, the WHERE clause.
                        let unique = sql.starts_with("CREATE UNIQUE INDEX");
                        let definition = sql[sql.find(" ON ").unwrap() + 4..]
                            .split_once('(')
                            .map(|(_, rest)| format!("({rest}"))
                            .unwrap();
                        Ok((
                            row.get::<_, String>(0)?,
                            format!(
                                "{}{}",
                                if unique { "UNIQUE " } else { "" },
                                normalize(&definition)
                            ),
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let strict = conn.query_row(
                    "SELECT strict FROM pragma_table_list WHERE schema='main' AND name=?1",
                    [&name],
                    |row| row.get(0),
                )?;
                tables.insert(
                    name,
                    Table {
                        strict,
                        columns,
                        constraints,
                        indexes,
                    },
                );
            }
            Ok(tables)
        })
        .unwrap()
}

/// SQLite reports the declared flag; a STRICT table's primary key is NOT NULL
/// whether or not the declaration spells it, so the document says "yes" there.
fn with_implied_not_null(mut tables: BTreeMap<String, Table>) -> BTreeMap<String, Table> {
    for table in tables.values_mut() {
        for column in &mut table.columns {
            if column.primary_key {
                column.not_null = true;
            }
        }
        table.indexes.sort();
    }
    tables
}

fn compare(documented: &BTreeMap<String, Table>, stored: &BTreeMap<String, Table>) -> Vec<String> {
    let mut differences = Vec::new();
    for name in documented.keys().chain(stored.keys()) {
        match (documented.get(name), stored.get(name)) {
            (Some(doc), Some(db)) => {
                if doc != db && !differences.iter().any(|d: &String| d.starts_with(name)) {
                    differences.push(format!("{name}: documented {doc:?} != stored {db:?}"));
                }
            }
            (Some(_), None) => differences.push(format!("{name}: documented but not stored")),
            (None, Some(_)) => differences.push(format!("{name}: stored but not documented")),
            (None, None) => unreachable!(),
        }
    }
    differences.sort();
    differences.dedup();
    differences
}

#[test]
fn the_baseline_matches_the_frozen_inventory_field_for_field() {
    let documented = with_implied_not_null(documented());
    assert_eq!(documented.len(), 10, "every baseline table is documented");
    let stored = with_implied_not_null(stored(retrieval::BASELINE));
    assert_eq!(compare(&documented, &stored), Vec::<String>::new());
    // The inventory gives every persistence field of the contract a home.
    for (table, column) in [
        ("projection_identity", "kernel_incarnation_id"),
        ("projection_identity", "tokenizer_fingerprint"),
        ("projection_identity", "vector_dimension"),
        ("projection_identity", "generation_epoch"),
        ("occurrences", "tuple"),
        ("occurrences", "payload_id"),
        ("occurrence_tombstones", "invalidated_commit_seq"),
        ("projection_checkpoint", "checkpoint_commit_seq"),
        ("embedding_jobs", "attempts"),
        ("embedding_jobs", "state"),
        ("occurrence_vectors", "vector"),
        ("retirement_receipts", "generation_id"),
    ] {
        assert!(
            documented[table].columns.iter().any(|c| c.name == column),
            "{table}.{column}"
        );
    }
    let state = documented["embedding_jobs"]
        .columns
        .iter()
        .find(|c| c.name == "state")
        .unwrap();
    for disposition in [
        "pending",
        "admitted",
        "embedded",
        "published",
        "obsolete",
        "failed",
    ] {
        assert!(
            state.constraint.contains(&format!("'{disposition}'")),
            "the {disposition} disposition is spelled in the schema"
        );
    }
}

/// The `CHECK` lists are exactly the Rust vocabularies, so a variant added or
/// respelled on one side fails here rather than at write time.
#[test]
fn the_check_vocabularies_equal_the_rust_enums() {
    let stored = stored(retrieval::BASELINE);
    let check = |table: &str, column: &str| {
        stored[table]
            .columns
            .iter()
            .find(|c| c.name == column)
            .unwrap()
            .constraint
            .clone()
    };
    let list = |values: &[&str]| {
        values
            .iter()
            .map(|value| format!("'{value}'"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let sensitivities: Vec<&str> = kernel::Sensitivity::ALL
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(
        check("occurrences", "sensitivity"),
        format!("CHECK(sensitivity IN ({}))", list(&sensitivities))
    );
    let classes: Vec<&str> = kernel::source_identity::OccurrenceClass::ALL
        .iter()
        .map(|c| c.code())
        .collect();
    assert_eq!(
        check("occurrences", "class"),
        format!("CHECK(class IN ({}))", list(&classes))
    );
    let reasons: Vec<&str> = retrieval::TombstoneReason::ALL
        .iter()
        .map(|r| r.as_str())
        .collect();
    assert_eq!(
        check("occurrence_tombstones", "reason"),
        format!("CHECK(reason IN ({}))", list(&reasons))
    );
}

#[test]
fn an_omitted_field_or_constraint_fails_the_inventory() {
    let documented = with_implied_not_null(documented());
    let missing_strict = retrieval::BASELINE.replacen(") STRICT", ")", 1);
    assert_ne!(missing_strict, retrieval::BASELINE);
    let differences = compare(&documented, &with_implied_not_null(stored(&missing_strict)));
    assert_eq!(
        differences.len(),
        1,
        "removing STRICT must fail the inventory: {differences:?}"
    );
    assert!(differences[0].starts_with("projection_identity:"));

    let missing_column = retrieval::BASELINE.replace("    persisted_at INTEGER NOT NULL,\n", "");
    assert_ne!(missing_column, retrieval::BASELINE);
    let differences = compare(&documented, &with_implied_not_null(stored(&missing_column)));
    assert_eq!(differences.len(), 1);
    assert!(
        differences[0].starts_with("occurrences:"),
        "{differences:?}"
    );

    let missing_constraint =
        retrieval::BASELINE.replace("    CHECK((span_start IS NULL)=(span_end IS NULL)),\n", "");
    assert_ne!(missing_constraint, retrieval::BASELINE);
    let differences = compare(
        &documented,
        &with_implied_not_null(stored(&missing_constraint)),
    );
    assert_eq!(differences.len(), 1, "{differences:?}");
    assert!(differences[0].starts_with("occurrences:"));
    assert!(differences[0].contains("(span_start IS NULL)=(span_end IS NULL)"));

    let not_unique = retrieval::BASELINE.replace(
        "CREATE UNIQUE INDEX idx_vector_generations_selected",
        "CREATE INDEX idx_vector_generations_selected",
    );
    assert_ne!(not_unique, retrieval::BASELINE);
    let differences = compare(&documented, &with_implied_not_null(stored(&not_unique)));
    assert_eq!(differences.len(), 1, "{differences:?}");
    assert!(differences[0].starts_with("vector_generations:"));

    let missing_table = retrieval::BASELINE
        .replace(
            "CREATE TABLE retirement_receipts(",
            "CREATE TABLE retirement_receipts_renamed(",
        )
        .replace(
            "ON retirement_receipts(generation_id,receipt_id)",
            "ON retirement_receipts_renamed(generation_id,receipt_id)",
        );
    let differences = compare(&documented, &with_implied_not_null(stored(&missing_table)));
    assert_eq!(differences.len(), 2, "{differences:?}");
}

#[test]
fn a_store_opened_from_the_baseline_reports_the_pinned_connection_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), retrieval::BASELINE);
    let (journal, synchronous, foreign_keys): (String, i64, bool) = store
        .with_conn_unfenced(|conn| {
            Ok((
                conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
                conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?,
                conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 2, "FULL");
    assert!(foreign_keys);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = std::fs::metadata(dir.path().join("search"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(parent, 0o700, "owner-only directory");
    }
}
