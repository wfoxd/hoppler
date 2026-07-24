//! Schema and forward-only migrations.
//!
//! Schema version is tracked in SQLite's `PRAGMA user_version` — the idiomatic
//! per-database schema counter (an explicit `schema_version` table would be a
//! hand-rolled equivalent). Each [`MIGRATIONS`] entry upgrades from version `i`
//! to `i + 1`; entries are append-only and never edited or reordered.

use rusqlite::Connection;

use super::StoreError;

/// Forward-only migration steps. Index `i` upgrades the DB from version `i` to
/// `i + 1`. Append only — editing a shipped entry corrupts existing databases.
const MIGRATIONS: &[&str] = &[
    // v0 -> v1: initial Ring 0 schema.
    r#"
    CREATE TABLE contacts (
        id              INTEGER PRIMARY KEY,
        l1_pub          BLOB NOT NULL UNIQUE,
        l2_pub          BLOB NOT NULL,
        name            TEXT NOT NULL,
        colour          INTEGER NOT NULL,
        persona_version INTEGER NOT NULL,
        paired_at       INTEGER NOT NULL
    );
    CREATE TABLE threads (
        id         INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL UNIQUE REFERENCES contacts(id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE messages (
        id         INTEGER PRIMARY KEY,
        thread_id  INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        seq        INTEGER NOT NULL,
        msg_id     BLOB NOT NULL UNIQUE,
        body       BLOB NOT NULL,
        direction  INTEGER NOT NULL,
        state      INTEGER NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX idx_messages_thread_seq ON messages(thread_id, seq);
    CREATE TABLE blocklist (
        pseudonym       BLOB PRIMARY KEY,
        created_at      INTEGER NOT NULL,
        revoked_pairing INTEGER NOT NULL
    );
    CREATE TABLE transfers (
        id           INTEGER PRIMARY KEY,
        thread_id    INTEGER REFERENCES threads(id) ON DELETE SET NULL,
        direction    INTEGER NOT NULL,
        name         TEXT NOT NULL,
        size         INTEGER NOT NULL,
        mime         TEXT NOT NULL,
        state        INTEGER NOT NULL,
        root_hash    BLOB NOT NULL,
        chunk_bitmap BLOB NOT NULL,
        created_at   INTEGER NOT NULL
    );
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value BLOB NOT NULL
    );
    "#,
];

/// The schema version this build migrates to.
pub const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Apply every pending migration. Each step and its version bump commit
/// atomically, so an interruption never leaves a half-applied version. Refuses
/// to run against a database newer than this build understands.
pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    // user_version is a signed integer; a negative value is corrupt, not a
    // valid version to cast into usize.
    let current = current_version(conn)?;
    if current < 0 {
        return Err(StoreError::Db(format!("invalid schema version {current}")));
    }
    let mut version = current as usize;
    if version > MIGRATIONS.len() {
        return Err(StoreError::Db(format!(
            "database schema v{version} is newer than this build (v{})",
            MIGRATIONS.len()
        )));
    }

    while version < MIGRATIONS.len() {
        let step = MIGRATIONS[version];
        let next = version + 1;
        // Roll back explicitly if a step fails mid-batch, so the connection is
        // never left with an open transaction.
        conn.execute_batch("BEGIN;")?;
        let applied = conn
            .execute_batch(step)
            .and_then(|()| conn.execute_batch(&format!("PRAGMA user_version = {next};")));
        match applied {
            Ok(()) => conn.execute_batch("COMMIT;")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e.into());
            }
        }
        version = next;
    }
    Ok(())
}
