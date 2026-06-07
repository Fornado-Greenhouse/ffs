//! Schema migration runner. Steps forward one version at a time so
//! an existing task_24 database (v1) cleanly picks up later
//! additions without losing data.

use rusqlite::{Connection, params};

use super::schema::{V1_DDL, V2_DDL};
use super::{SCHEMA_VERSION, StoreError};

/// Apply schema migrations idempotently.
///
/// - Fresh database → applies v1, then steps to v2, etc.
/// - Existing v1 database → applies v2 only (additive: new tables,
///   no schema-rewrites of existing data).
/// - Database at a version higher than [`SCHEMA_VERSION`] → returns
///   `StoreError::UnsupportedSchemaVersion`. (Used when downgrading
///   to an older binary — fails loudly rather than silently
///   ignoring data the older binary can't understand.)
pub fn apply(conn: &Connection) -> Result<(), StoreError> {
    // The schema_version table itself must exist before we can read from it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT    NOT NULL
        );",
    )?;

    let mut current: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get::<_, Option<u32>>(0).map(|v| v.unwrap_or(0))
        })
        .unwrap_or(0);

    if current > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }

    // Step forward one version at a time. Each step runs in its own
    // transaction so a partial failure leaves the schema at the
    // last successful version rather than half-applied.
    while current < SCHEMA_VERSION {
        let next = current + 1;
        let ddl = match next {
            1 => V1_DDL,
            2 => V2_DDL,
            other => {
                return Err(StoreError::UnsupportedSchemaVersion {
                    found: other,
                    supported: SCHEMA_VERSION,
                });
            }
        };
        conn.execute_batch("BEGIN;")?;
        conn.execute_batch(ddl)?;
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![next, now_iso()],
        )?;
        conn.execute_batch("COMMIT;")?;
        current = next;
    }

    Ok(())
}

fn now_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory_then_apply() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        apply(&conn).expect("apply");
        conn
    }

    #[test]
    fn fresh_db_lands_at_current_schema_version() {
        let conn = open_in_memory_then_apply();
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn v1_then_v2_step_creates_both_atoms_and_quarantine_tables() {
        let conn = open_in_memory_then_apply();
        // v1 created the atoms table.
        conn.execute_batch("SELECT 1 FROM atoms LIMIT 0")
            .expect("atoms");
        // v2 created the quarantine tables.
        conn.execute_batch("SELECT 1 FROM quarantine_submissions LIMIT 0")
            .expect("quarantine_submissions");
        conn.execute_batch("SELECT 1 FROM quarantine_proposals LIMIT 0")
            .expect("quarantine_proposals");
    }

    #[test]
    fn migration_is_idempotent_across_calls() {
        let conn = open_in_memory_then_apply();
        apply(&conn).expect("second apply must be a no-op");
        apply(&conn).expect("third apply must be a no-op");
        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
            .unwrap();
        // Each successful migration inserts exactly one row.
        assert_eq!(count, SCHEMA_VERSION);
    }

    #[test]
    fn applying_v2_on_top_of_existing_v1_db_adds_only_new_tables() {
        // Simulate an existing task_24 database: apply v1 manually,
        // insert a marker row, then call apply() to bring it to v2.
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch("BEGIN;").unwrap();
        conn.execute_batch(V1_DDL).unwrap();
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![1, now_iso()],
        )
        .unwrap();
        conn.execute_batch("COMMIT;").unwrap();

        // Apply the migration runner — should step from v1 to v2.
        apply(&conn).expect("step to v2");

        // Marker: v2 tables now exist.
        conn.execute_batch("SELECT 1 FROM quarantine_submissions LIMIT 0")
            .expect("v2 table exists");
        // The v1 tables stayed: atoms is queryable.
        conn.execute_batch("SELECT 1 FROM atoms LIMIT 0")
            .expect("v1 table preserved");

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn database_at_future_version_refuses_to_open() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![99, now_iso()],
        )
        .unwrap();

        let err = apply(&conn).expect_err("future version must error");
        assert!(matches!(
            err,
            StoreError::UnsupportedSchemaVersion {
                found: 99,
                supported: SCHEMA_VERSION
            }
        ));
    }
}
