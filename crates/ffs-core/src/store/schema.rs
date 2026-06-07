//! SQL schema for the SQLCipher atom store. ADR-016 specifies the table
//! layout; this module is the canonical source for the v1 DDL.

/// Initial v1 schema. Idempotent: each `CREATE TABLE` and `CREATE INDEX`
/// uses `IF NOT EXISTS` so `apply()` can be called against a fresh DB or
/// against a partial state without failing.
pub const V1_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version    INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS atoms (
    content_hash   BLOB    PRIMARY KEY,
    entity_id      TEXT    NOT NULL,
    predicate      TEXT    NOT NULL,
    author         BLOB    NOT NULL,
    valid_from     TEXT    NOT NULL,
    valid_to       TEXT,
    tx_time        TEXT    NOT NULL,
    classification TEXT    NOT NULL,
    supersedes     BLOB,
    signature      BLOB    NOT NULL,
    envelope       BLOB    NOT NULL
);

CREATE INDEX IF NOT EXISTS atoms_entity_predicate_tx
    ON atoms(entity_id, predicate, tx_time DESC);
CREATE INDEX IF NOT EXISTS atoms_predicate_tx
    ON atoms(predicate, tx_time DESC);
CREATE INDEX IF NOT EXISTS atoms_supersedes
    ON atoms(supersedes) WHERE supersedes IS NOT NULL;
CREATE INDEX IF NOT EXISTS atoms_author_tx
    ON atoms(author, tx_time DESC);

CREATE TABLE IF NOT EXISTS classifications (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    atom_hash  BLOB    NOT NULL REFERENCES atoms(content_hash),
    tier       TEXT    NOT NULL,
    classifier BLOB    NOT NULL,
    tx_time    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS classifications_atom_hash ON classifications(atom_hash);
CREATE INDEX IF NOT EXISTS classifications_tier      ON classifications(tier);

CREATE TABLE IF NOT EXISTS capabilities (
    capability_hash BLOB    PRIMARY KEY,
    grantor         BLOB    NOT NULL,
    grantee         BLOB    NOT NULL,
    actions         TEXT    NOT NULL,
    scope           TEXT    NOT NULL,
    valid_from      TEXT    NOT NULL,
    valid_to        TEXT,
    tx_time         TEXT    NOT NULL,
    superseded_by   BLOB
);
CREATE INDEX IF NOT EXISTS capabilities_grantee ON capabilities(grantee);

CREATE TABLE IF NOT EXISTS provenance (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    atom_hash   BLOB    NOT NULL REFERENCES atoms(content_hash),
    source_kind TEXT    NOT NULL,
    source_uri  TEXT    NOT NULL,
    source_hash BLOB    NOT NULL
);
CREATE INDEX IF NOT EXISTS provenance_atom_hash ON provenance(atom_hash);

CREATE TABLE IF NOT EXISTS entities (
    entity_id       TEXT PRIMARY KEY,
    canonical_label TEXT
);

CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(
    content_hash UNINDEXED,
    payload,
    tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS federation_peers (
    peer_id           BLOB PRIMARY KEY,
    endpoint          TEXT NOT NULL,
    cert_fingerprint  BLOB NOT NULL,
    last_pull         TEXT,
    bridge_capability BLOB
);

CREATE TABLE IF NOT EXISTS working_set (
    projection_path  TEXT PRIMARY KEY,
    materialized_at  TEXT NOT NULL,
    last_render_hash BLOB,
    pinned           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ingest_quarantine (
    submission_id     INTEGER PRIMARY KEY AUTOINCREMENT,
    source_uri        TEXT    NOT NULL,
    submitted_at      TEXT    NOT NULL,
    proposed_envelope BLOB    NOT NULL,
    submitter         BLOB    NOT NULL,
    status            TEXT    NOT NULL DEFAULT 'pending'
);
CREATE INDEX IF NOT EXISTS ingest_quarantine_status ON ingest_quarantine(status);
"#;

/// V2 (task_29): SQLCipher-backed `IngestQuarantine` implementation.
/// The v1 placeholder `ingest_quarantine` table didn't match the
/// runtime trait shape, so this DDL adds two new tables
/// (`quarantine_submissions` + `quarantine_proposals`) that mirror
/// the existing `Submission` + `Proposal` struct layout. The v1
/// placeholder table stays untouched for backwards compatibility
/// and is intentionally unused by the new code.
pub const V2_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS quarantine_submissions (
    id                   TEXT    PRIMARY KEY,
    source_uri           TEXT    NOT NULL,
    content_hash         BLOB    NOT NULL,
    content              BLOB    NOT NULL,
    tx_time              TEXT    NOT NULL,
    status               TEXT    NOT NULL,
    failure_reason       TEXT,
    accepted_atom_hashes TEXT    NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS quarantine_submissions_status_tx
    ON quarantine_submissions(status, tx_time DESC);

CREATE TABLE IF NOT EXISTS quarantine_proposals (
    submission_id TEXT    NOT NULL REFERENCES quarantine_submissions(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    predicate     TEXT    NOT NULL,
    claim         TEXT    NOT NULL,
    provenance    TEXT    NOT NULL,
    rationale     TEXT    NOT NULL,
    PRIMARY KEY (submission_id, seq)
);
"#;
