//! Schema migration runner. v1 only at MVP; future tasks bump the
//! `SCHEMA_VERSION` constant in [`crate::store`] and add a migration step.

use rusqlite::{Connection, params};

use super::schema::V1_DDL;
use super::{SCHEMA_VERSION, StoreError};

/// Apply schema migrations idempotently. On a fresh database, creates v1.
/// On an existing database at v1, no-ops. On a database at a higher
/// version than [`SCHEMA_VERSION`], returns
/// `StoreError::UnsupportedSchemaVersion`.
pub fn apply(conn: &Connection) -> Result<(), StoreError> {
    // The schema_version table itself must exist before we can read from it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT    NOT NULL
        );",
    )?;

    let current: Option<u32> = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .ok();

    if let Some(v) = current
        && v > SCHEMA_VERSION
    {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: v,
            supported: SCHEMA_VERSION,
        });
    }

    if current.is_none() || current == Some(0) {
        // Fresh database — apply v1 in a single transaction.
        conn.execute_batch("BEGIN;")?;
        conn.execute_batch(V1_DDL)?;
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![1, now_iso()],
        )?;
        conn.execute_batch("COMMIT;")?;
    }

    Ok(())
}

fn now_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
