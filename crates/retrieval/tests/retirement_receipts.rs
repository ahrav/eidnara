use kernel::ConsumerObligation;
use retrieval::ProjectionError;
use retrieval::retirement::{RetirementReceipt, record_receipt, verify_receipt};
use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

fn open_store(dir: &std::path::Path) -> SqliteStore {
    open_sqlite(
        &StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "search-projection".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir.join("search.sqlite").to_string_lossy().into_owned(),
            },
        },
        retrieval::BASELINE,
    )
    .unwrap()
}

fn receipt(through: i64) -> RetirementReceipt<'static> {
    RetirementReceipt {
        old_consumer: "old-consumer",
        old_generation: "old-generation",
        old_family: "old-family",
        selected_family: "selected-family",
        kernel_incarnation: "kernel",
        through,
    }
}

fn obligation(identity: &str, commit_seq: i64) -> ConsumerObligation {
    ConsumerObligation {
        kind: "source".to_string(),
        identity: identity.to_string(),
        artifact_digest: "a".repeat(64),
        commit_seq,
        invalidated_commit_seq: None,
    }
}

fn through_and_dispositions(store: &SqliteStore) -> (i64, Vec<String>) {
    store
        .with_conn(|conn| {
            let through = conn.query_row(
                "SELECT through_commit_seq FROM retirement_receipts",
                [],
                |row| row.get(0),
            )?;
            let identities = conn
                .prepare("SELECT identity FROM retirement_dispositions ORDER BY identity")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((through, identities))
        })
        .unwrap()
}

#[test]
fn a_later_certification_supersedes_an_earlier_receipt_and_nothing_else_does() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    let first = [obligation("early", 3)];
    let later = [obligation("early", 3), obligation("late", 6)];
    store
        .with_conn_fenced(|conn| {
            record_receipt(conn, &receipt(5), &first, 1).unwrap();
            assert!(verify_receipt(conn, &receipt(5), &first).unwrap());
            assert!(!verify_receipt(conn, &receipt(7), &later).unwrap());
            record_receipt(conn, &receipt(7), &later, 2).unwrap();
            assert!(verify_receipt(conn, &receipt(7), &later).unwrap());
            assert!(matches!(
                verify_receipt(conn, &receipt(5), &first),
                Err(ProjectionError::MutationConflict)
            ));
            let other = RetirementReceipt {
                selected_family: "another-family",
                ..receipt(9)
            };
            assert!(matches!(
                verify_receipt(conn, &other, &later),
                Err(ProjectionError::MutationConflict)
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        through_and_dispositions(&store),
        (7, vec!["early".to_string(), "late".to_string()])
    );
}
