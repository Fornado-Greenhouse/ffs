//! Schema migration runner. Steps forward one version at a time so
//! an existing task_24 database (v1) cleanly picks up later
//! additions without losing data.

use rusqlite::{Connection, params};

use super::schema::{V1_DDL, V2_DDL, V3_DDL, V4_DDL, V5_DDL, V6_DDL, V7_DDL};
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
            3 => V3_DDL,
            4 => V4_DDL,
            5 => V5_DDL,
            6 => V6_DDL,
            7 => V7_DDL,
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

        // Apply the migration runner — should step from v1 through
        // v2 and on to the current version.
        apply(&conn).expect("step to current");

        // Marker: v2 tables now exist.
        conn.execute_batch("SELECT 1 FROM quarantine_submissions LIMIT 0")
            .expect("v2 table exists");
        // Marker: v3 columns exist on the proposals table (task_36).
        conn.execute_batch("SELECT engine, model FROM quarantine_proposals LIMIT 0")
            .expect("v3 columns exist");
        // The v1 tables stayed: atoms is queryable.
        conn.execute_batch("SELECT 1 FROM atoms LIMIT 0")
            .expect("v1 table preserved");

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// A v2 database (task_29 shape, proposals without engine/model)
    /// must step to v3 without losing rows, and old rows read back
    /// with NULL engine/model.
    #[test]
    fn applying_v3_on_top_of_existing_v2_db_adds_nullable_columns() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch("BEGIN;").unwrap();
        conn.execute_batch(V1_DDL).unwrap();
        conn.execute_batch(V2_DDL).unwrap();
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![1, now_iso()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![2, now_iso()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO quarantine_submissions (id, source_uri, content_hash, content, tx_time, status)
             VALUES ('sub-1', 'file:///a.md', x'00', x'00', '2026-01-01T00:00:00Z', 'extracted')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO quarantine_proposals (submission_id, seq, predicate, claim, provenance, rationale)
             VALUES ('sub-1', 0, 'note', '{}', '[]', 'old row')",
            [],
        )
        .unwrap();
        conn.execute_batch("COMMIT;").unwrap();

        apply(&conn).expect("step to v3");

        let (engine, model): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT engine, model FROM quarantine_proposals WHERE submission_id = 'sub-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(engine.is_none());
        assert!(model.is_none());
    }

    /// A v5 database (task_45 shape) must step to v6 and gain the
    /// auto-filing column (task_39, ADR-029); pre-v6 rows read back
    /// with an empty list.
    #[test]
    fn applying_v6_on_top_of_existing_v5_db_adds_auto_accepted_column() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch("BEGIN;").unwrap();
        for ddl in [V1_DDL, V2_DDL, V3_DDL, V4_DDL, V5_DDL] {
            conn.execute_batch(ddl).unwrap();
        }
        for v in 1..=5u32 {
            conn.execute(
                "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
                params![v, now_iso()],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO quarantine_submissions(id, source_uri, content_hash, content, tx_time, status)
             VALUES ('s1', 'file:///a', X'00', X'00', '2026-01-01T00:00:00Z', 'extracted')",
            [],
        )
        .unwrap();
        conn.execute_batch("COMMIT;").unwrap();

        apply(&conn).expect("step to v6");

        let auto: String = conn
            .query_row(
                "SELECT auto_accepted_atom_hashes FROM quarantine_submissions WHERE id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(auto, "[]");
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
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

    /// A v3 database (task_36 shape) must step to v4 and gain the
    /// path-to-entity index table (task_38, ADR-030).
    #[test]
    fn applying_v4_on_top_of_existing_v3_db_adds_the_path_index_table() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch("BEGIN;").unwrap();
        conn.execute_batch(V1_DDL).unwrap();
        conn.execute_batch(V2_DDL).unwrap();
        conn.execute_batch(V3_DDL).unwrap();
        for v in 1..=3u32 {
            conn.execute(
                "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
                params![v, now_iso()],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT;").unwrap();

        apply(&conn).expect("step to v4");

        conn.execute_batch("SELECT family, basename, entity, display FROM path_index LIMIT 0")
            .expect("v4 path_index table exists");
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// A v4 database (task_38 shape) must step to v5 and gain the
    /// resolution columns and tables (task_45, ADR-030); rows written
    /// before v5 read back with NULL resolution fields.
    #[test]
    fn applying_v5_on_top_of_existing_v4_db_adds_resolution_columns_and_tables() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute_batch("BEGIN;").unwrap();
        for ddl in [V1_DDL, V2_DDL, V3_DDL, V4_DDL] {
            conn.execute_batch(ddl).unwrap();
        }
        for v in 1..=4u32 {
            conn.execute(
                "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
                params![v, now_iso()],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO quarantine_submissions(id, source_uri, content_hash, content, tx_time, status)
             VALUES ('s1', 'file:///a', X'00', X'00', '2026-01-01T00:00:00Z', 'extracted')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO quarantine_proposals(submission_id, seq, predicate, claim, provenance, rationale)
             VALUES ('s1', 0, 'note', '{}', '[]', 'r')",
            [],
        )
        .unwrap();
        conn.execute_batch("COMMIT;").unwrap();

        apply(&conn).expect("step to v5");

        let resolution: Option<String> = conn
            .query_row(
                "SELECT resolution FROM quarantine_proposals WHERE submission_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolution.is_none(), "pre-v5 rows read back NULL");
        conn.execute_batch("SELECT form, entity, count FROM resolution_priors LIMIT 0")
            .expect("priors table");
        conn.execute_batch(
            "SELECT key, submission_id, display, first_seen FROM nil_sightings LIMIT 0",
        )
        .expect("sightings table");
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }
}
