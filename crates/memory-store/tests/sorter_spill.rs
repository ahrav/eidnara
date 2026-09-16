//! Runs alone in its own process: SQLite's allocation statistics, which the workspace
//! builds off, can only be switched on before the library's first initialization.

use memory_store::{MemoryStore, PAGE_CACHE_BUDGET_BYTES};
use rusqlite::params;

/// SQLite bounds a sorter's in-memory list at `cache_size` pages and spills excess rows
/// to the temp store only when the temp store uses files; with the temp store in memory
/// every sorted row stays in the heap. A read callback sorting four budgets of rows
/// retains about one budget when the bound applies.
#[test]
fn a_transient_sort_past_the_page_cache_budget_spills_instead_of_growing_the_heap() {
    const PAYLOAD_BYTES: i64 = 4096;
    assert!(
        storage::enable_library_memory_statistics(),
        "SQLite was initialized before this test switched its statistics on; keep this test alone in its binary"
    );
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open_for_test(dir.path(), "sorter-spill");
    let rows = 4 * PAGE_CACHE_BUDGET_BYTES / PAYLOAD_BYTES;
    let (before, retained) = store
        .with_conn_for_test(|conn| {
            let mut statement = conn.prepare(
                "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1)
                 SELECT payload FROM (SELECT n, randomblob(?2) AS payload FROM seq)
                  ORDER BY (n * 7919) % 10007, n",
            )?;
            let before = storage::library_memory_used();
            let mut sorted = statement.query(params![rows, PAYLOAD_BYTES])?;
            sorted.next()?.expect("the sort yields its first row");
            Ok((before, storage::library_memory_used() - before))
        })
        .unwrap();
    assert!(
        before > 0,
        "the statistics read zero; the oracle is not live"
    );
    assert!(
        retained <= 2 * PAGE_CACHE_BUDGET_BYTES,
        "sorting {} bytes retained {retained} bytes; the sorter did not spill at the {} byte page-cache budget",
        rows * PAYLOAD_BYTES,
        PAGE_CACHE_BUDGET_BYTES
    );
}
