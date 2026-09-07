//! The module probes SQLite off-path and validates kernel writer contracts.
//!
//! "Direct format" names the on-disk layout the kernel writes itself: the
//! `application_id` and `user_version` pragmas plus the `kernel_format_marker`
//! row whose digest binds them to the schema digest.

use rusqlite::Connection;
use sha2::{Digest, Sha256};

/// `KERNEL_APPLICATION_ID` sets `PRAGMA application_id` to ASCII `EIDN`.
///
/// Same value as `storage::APPLICATION_ID`; the kernel keeps its own opener,
/// so the constant is restated here rather than imported.
pub const KERNEL_APPLICATION_ID: u32 = 0x4549_444E;

/// `DIRECT_FORMAT_EPOCH` sets `PRAGMA user_version` to `1` for the direct format.
pub const DIRECT_FORMAT_EPOCH: i64 = 1;

/// Domain-separation prefix for direct-format marker digests.
const FORMAT_MARKER_DIGEST_PROTOCOL: &str = "eidnara-direct-format-marker-v1";

/// Minimum supported SQLite release, carrying the complete WAL-reset race fix
/// (<https://www.sqlite.org/wal.html#walresetbug>).
const MIN_SUPPORTED_SQLITE_VERSION: [u64; 3] = [3, 51, 3];

/// SQLite build identity used by the runtime compatibility gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteEngineIdentity {
    /// Version returned by `sqlite_version()`.
    pub sqlite_version: String,
    /// Source identity returned by `sqlite_source_id()`.
    pub sqlite_source_id: String,
}

/// Reads both values required by the runtime compatibility gate.
fn read_sqlite_engine_identity(conn: &Connection) -> rusqlite::Result<SqliteEngineIdentity> {
    conn.query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
        Ok(SqliteEngineIdentity {
            sqlite_version: row.get(0)?,
            sqlite_source_id: row.get(1)?,
        })
    })
}

/// Isolates identity probing from store connections by using in-memory SQLite.
pub fn probe_sqlite_engine_identity_off_path() -> rusqlite::Result<SqliteEngineIdentity> {
    let conn = Connection::open_in_memory()?;
    read_sqlite_engine_identity(&conn)
}

/// Formats three numeric components without normalization.
fn format_dotted_version(version: [u64; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

/// Parses the version forms accepted by the compatibility gate.
///
/// Missing patch numbers become zero.
fn parse_dotted_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(raw) => raw
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?,
        None => 0,
    };
    Some([major, minor, patch])
}

fn is_well_formed_source_id(source_id: &str) -> bool {
    let bytes = source_id.as_bytes();
    // The validator rejects a multibyte character crossing byte 20 because `split_at(20)` requires a character boundary.
    if bytes.len() < 20 + 40 || !source_id.is_char_boundary(20) {
        return false;
    }
    let (stamp, hash) = source_id.split_at(20);
    let stamp_ok = stamp.char_indices().all(|(index, c)| match index {
        4 | 7 => c == '-',
        10 => c == ' ',
        13 | 16 => c == ':',
        19 => c == ' ',
        _ => c.is_ascii_digit(),
    });
    let hash_ok = (40..=64).contains(&hash.len())
        && hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
    stamp_ok && hash_ok
}

/// The gate returns every WAL-reset-safety failure; an empty vector passes.
/// The gate requires the engine identity; a wrapper version alone cannot pass.
#[must_use]
pub fn evaluate_sqlite_runtime_gate(identity: &SqliteEngineIdentity) -> Vec<String> {
    let mut reasons = Vec::new();
    match parse_dotted_version(&identity.sqlite_version) {
        Some(version) if version >= MIN_SUPPORTED_SQLITE_VERSION => {}
        _ => reasons.push(format!(
            "SQLite {} is below the supported floor {}",
            identity.sqlite_version,
            format_dotted_version(MIN_SUPPORTED_SQLITE_VERSION)
        )),
    }
    if !is_well_formed_source_id(&identity.sqlite_source_id) {
        reasons.push(format!(
            "sqlite_source_id() '{}' is not a recognized SQLite source identity",
            identity.sqlite_source_id
        ));
    }
    reasons
}

/// The verifier checks the connection contract after applying PRAGMAs.
/// The contract requires enforced foreign keys, expected WAL mode, a busy timeout, and a declared synchronous mode.
/// The verifier returns every violation; an empty vector passes.
pub(crate) fn verify_sqlite_connection_contract(
    conn: &Connection,
    expect_wal: bool,
    min_busy_timeout_ms: i64,
) -> rusqlite::Result<Vec<String>> {
    let mut violations = Vec::new();
    let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        violations.push("foreign_keys is disabled".to_string());
    }
    let journal_mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if expect_wal && !journal_mode.eq_ignore_ascii_case("wal") {
        violations.push(format!("journal_mode is '{journal_mode}', expected 'wal'"));
    }
    let busy_timeout_ms: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    if busy_timeout_ms < min_busy_timeout_ms {
        violations.push(format!(
            "busy_timeout {busy_timeout_ms}ms is below the required {min_busy_timeout_ms}ms"
        ));
    }
    let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    if !(1..=3).contains(&synchronous) {
        violations.push(format!(
            "synchronous mode {synchronous} is not in the declared set [1, 2, 3]"
        ));
    }
    Ok(violations)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Hashes marker fields as fixed, newline-separated protocol lines, bound to
/// [`KERNEL_APPLICATION_ID`].
pub fn compute_marker_digest(
    format_epoch: i64,
    database_incarnation_id: &str,
    component_manifest_digest: &str,
    created_at_ms: i64,
) -> String {
    let lines = [
        FORMAT_MARKER_DIGEST_PROTOCOL.to_string(),
        format!("application_id={KERNEL_APPLICATION_ID}"),
        format!("format_epoch={format_epoch}"),
        format!("database_incarnation_id={database_incarnation_id}"),
        format!("component_manifest_digest={component_manifest_digest}"),
        format!("created_at_ms={created_at_ms}"),
    ];
    sha256_hex(&lines.join("\n"))
}
