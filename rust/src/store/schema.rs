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
    CREATE INDEX idx_messages_thread_dir_seq ON messages(thread_id, direction, seq);
    CREATE INDEX idx_messages_state ON messages(state);
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
    // v1 -> v2: stop calling a pseudonym a Layer-1 key, and record pairing
    // where it actually happens.
    //
    // `contacts.l1_pub` never held a Layer-1 key. Until T11 there was no way to
    // obtain one — R0-F1 says Layer-1 crosses only via the F4 ceremony, and the
    // ceremony did not exist — so the column held whatever durable handle was
    // available: the peer's session pseudonym once a handshake proved one, and
    // a value derived from the rotating device id before that. `paired_at` was
    // filled in the same spirit, for contacts that had never been paired with.
    //
    // That was survivable while nothing else could go in the column. It stops
    // being survivable now, because a real Layer-1 key is about to exist, and
    // putting it in the same UNIQUE column as peer-chosen pseudonyms would put
    // two key spaces in one index — with row identity resting on an attacker
    // being unable to find a pseudonym colliding with someone's Layer-1 key.
    // That may well be true and nobody ever wrote it down as a thing being
    // relied on, which is the wrong way to hold a security property.
    //
    // So: the columns are renamed to what they contain, and pairing moves to
    // its own table, where a row existing *is* the fact that two people
    // completed a ceremony. Renames rather than a table rebuild — SQLite
    // updates dependent objects for `RENAME COLUMN`, whereas rebuilding
    // `contacts` would mean dropping a table `threads` references, with foreign
    // keys on, inside a transaction where `PRAGMA foreign_keys` is a no-op.
    r#"
    ALTER TABLE contacts RENAME COLUMN l1_pub TO pseudonym;
    ALTER TABLE contacts RENAME COLUMN paired_at TO first_seen;
    CREATE TABLE pairings (
        contact_id INTEGER PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
        l1_pub     BLOB NOT NULL UNIQUE,
        paired_at  INTEGER NOT NULL
    );
    "#,
    // v2 -> v3: where a thread's receive state lives (T12 slice 3).
    //
    // Two tables and not one, because they are two kinds of thing. `ratchets`
    // holds key material — the root, both chains, and every key still owed for
    // a message that has not arrived — in one opaque blob whose encoding
    // deliberately exists only in Rust. `inboxes` holds numbers: what has
    // arrived and what is still missing, which is what the screen needs to show
    // a gap rather than close over it.
    //
    // Keeping them apart means the blob can stay a blob. Folding the counters
    // into it would mean parsing key material to answer "what is this thread
    // waiting for", and parsing key material to draw a screen is how it ends up
    // somewhere it should not be.
    //
    // Both are keyed by thread and cascade with it: a deleted thread must not
    // leave a ratchet behind, which under R0-F9 would be key material outliving
    // the conversation it belongs to.
    r#"
    CREATE TABLE ratchets (
        thread_id  INTEGER PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
        state      BLOB NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE inboxes (
        thread_id INTEGER PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
        through   INTEGER NOT NULL,
        ahead     BLOB NOT NULL
    );
    "#,
    // v3 -> v4: what a queued message will go out as, kept until it goes.
    //
    // A message waiting for its peer used to be re-sealed at every reunion,
    // which drew a fresh message key each time. A frame the transport accepted
    // and then lost therefore left the receiver's chain one behind for good,
    // and `walk` refuses a gap past `MAX_SKIP` — so a long enough run of
    // unlucky reunions ends the conversation with no way back. Sealed once and
    // kept, a resend is byte-identical: it costs no key, and a receiver that
    // already had it sees the replay for what it is.
    //
    // Its own table, for the reason `ratchets` has one. This is ciphertext, and
    // `messages` is what the screen reads — a blob on that row would be
    // returned by every query that draws a conversation, to be ignored each
    // time. Keyed on `msg_id` rather than the rowid because `msg_id` is what a
    // resend, an acknowledgement and a gap are all matched on, and `UNIQUE`
    // already makes it a key.
    //
    // The row lives exactly as long as the message is `Queued`:
    // `set_message_state` drops it on the way out, and `ON DELETE CASCADE`
    // drops it with the message.
    r#"
    CREATE TABLE outbound_seals (
        msg_id     BLOB PRIMARY KEY REFERENCES messages(msg_id) ON DELETE CASCADE,
        wire       BLOB NOT NULL,
        created_at INTEGER NOT NULL
    );
    "#,
    // v4 -> v5: a contact may have more than one thread, because pairing again
    // starts a new conversation.
    //
    // `threads.contact_id` was `UNIQUE`, so a second ceremony reused the first
    // one's thread — and with it that thread's inbox position. A peer who wiped
    // (R0-F9), re-paired and started again at `seq = 1` was therefore dropped
    // as a duplicate of something said before it ever knew this device.
    //
    // Clearing the inbox instead would have been two lines and wrong: the
    // thread would hold old rows numbered 1..N and new rows numbered from 1
    // again, and `messages_for_thread` orders by `seq`, so a message from today
    // would sort into the middle of last year. A pairing is what numbering
    // belongs to, so a new pairing gets a new thread and the old one keeps
    // everything that was said in it.
    //
    // A rebuild, because SQLite cannot drop a constraint. The ids are carried
    // across unchanged, which is what keeps `messages`, `ratchets`, `inboxes`
    // and `transfers` pointing at the right rows — they reference `threads(id)`
    // by name, and the name resolves to the new table once it is in place.
    // `migrate` runs with foreign keys off for exactly this: `DROP TABLE
    // threads` with them on is an implicit delete of every row, and every one
    // of those references cascades.
    r#"
    CREATE TABLE threads_rebuilt (
        id         INTEGER PRIMARY KEY,
        contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL
    );
    INSERT INTO threads_rebuilt (id, contact_id, created_at)
        SELECT id, contact_id, created_at FROM threads;
    DROP TABLE threads;
    ALTER TABLE threads_rebuilt RENAME TO threads;
    CREATE INDEX idx_threads_contact ON threads(contact_id, id);
    "#,
    // v5 -> v6: what a block is bound to, and who it is about (T18b).
    //
    // A block writes **one row per handle** this device holds for a person, all
    // carrying the same `contact_id`. Which handle a given surface can offer
    // depends on which side dialled, and that flips with the rotating ids, so
    // recording only the strongest is not a weaker block — it is one invisible
    // to half the surfaces that have to enforce it.
    //
    // The strongest is not always the Layer-1-derived pseudonym R0-F10 names.
    // It can only be that when the peer dialled us — Noise IK proves the
    // *initiator's* static — so for roughly half of peers the best available
    // handle is their Layer-2 persona key, or a hash of the rung id if no
    // persona has ever been fetched.
    //
    // `kind` records which, per row, so anything reporting on a person's block
    // can take the strongest of theirs and say how durable it really is.
    //
    // Defaulted to 0, the weakest, on the principle that a row from before this
    // column existed cannot prove anything about itself — and because in
    // practice there are no such rows: nothing in any shipped build has ever
    // written to this table.
    //
    // `contact_id` is `ON DELETE SET NULL` and deliberately not `CASCADE`. A
    // block has to outlive the contact row it came from; cascading would make
    // forgetting somebody a way of unblocking them, which is the one direction
    // this must never fail in. Nullable because a block can outlive its contact
    // and because SQLite requires a NULL default when `ADD COLUMN` carries a
    // foreign key.
    r#"
    ALTER TABLE blocklist ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE blocklist ADD COLUMN contact_id INTEGER
        REFERENCES contacts(id) ON DELETE SET NULL;
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
    // Off for the duration, on again at the end, with a check in between. This
    // is SQLite's own recipe for a schema change and not a way around the
    // constraints: a step that rebuilds a table has to drop the old one, and
    // `DROP TABLE` with foreign keys on is an implicit delete of every row —
    // which for `threads` would cascade through `messages`, `ratchets`,
    // `inboxes` and `transfers` and take the whole database with it.
    //
    // The pragma is a no-op inside a transaction, so it has to be set out here
    // rather than in a step. `PRAGMA foreign_key_check` before it goes back on
    // is what makes this safe rather than merely quiet: a rebuild that lost an
    // id would leave rows pointing at nothing, and turning enforcement back on
    // would not notice.
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    // Read back, because setting it is not the same as it being off and the
    // difference is a database. `PRAGMA foreign_keys` is silently a no-op
    // inside a transaction, and there is no error to catch — the statement
    // succeeds and enforcement stays on. A rebuild then reaches `DROP TABLE
    // threads` with enforcement live, which SQLite treats as deleting every
    // row, and the cascade takes `messages`, `ratchets` and `inboxes` with it
    // while leaving `contacts` and `pairings` untouched.
    //
    // That is not a hypothetical shape. It is what two phones looked like after
    // v5: every contact still paired, every conversation gone. The tests here
    // could not produce it — not from v3, not from v4, not on a WAL database
    // with enforcement already on — which is the whole argument for checking
    // the state rather than trusting the statement.
    let enforcing: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    if enforcing != 0 {
        return Err(StoreError::Db(
            "refusing to migrate: foreign keys could not be turned off, and a \
             rebuild would delete every row it references"
                .into(),
        ));
    }
    let out = migrate_steps(conn);
    let checked = out.and_then(|()| {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let dangling = stmt.query_map([], |_| Ok(()))?.count();
        if dangling > 0 {
            return Err(StoreError::Db(format!(
                "migration left {dangling} rows referring to something that is not there"
            )));
        }
        Ok(())
    });
    // On again whatever happened, so a failed migration does not leave the
    // connection quietly unenforced for the rest of its life.
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    checked
}

fn migrate_steps(conn: &Connection) -> Result<(), StoreError> {
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
        let committed = applied.and_then(|()| conn.execute_batch("COMMIT;"));
        if let Err(e) = committed {
            // Roll back on either a failed step or a failed COMMIT, so the
            // connection is never left with an open transaction.
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e.into());
        }
        version = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bring a bare connection up to a chosen version, so a test can stand on
    /// an old schema rather than describing one.
    fn at_version(conn: &Connection, version: usize) {
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for step in &MIGRATIONS[..version] {
            conn.execute_batch(step).unwrap();
        }
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))
            .unwrap();
    }

    /// The same, on a WAL database with foreign keys already on.
    ///
    /// What the phones run, and the one difference from the test above that a
    /// desk cannot see: `Store::open` sets `journal_mode = WAL` before it
    /// migrates, and `PRAGMA foreign_keys` is a no-op inside a transaction. If
    /// enforcement survived into the rebuild, `DROP TABLE threads` would be an
    /// implicit delete of every row and the cascade would take the messages,
    /// the ratchets and the inboxes with it — leaving contacts and pairings
    /// untouched, which is exactly what an empty conversation list looks like.
    #[test]
    fn a_wal_database_survives_the_threads_rebuild_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        at_version(&conn, 3);
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute(
            "INSERT INTO contacts (id, pseudonym, l2_pub, name, colour, persona_version, first_seen)
             VALUES (1, ?1, ?2, 'Ada', 0, 1, 1)",
            rusqlite::params![&[7u8; 32][..], &[8u8; 32][..]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, contact_id, created_at) VALUES (5, 1, 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, seq, msg_id, body, direction, state, created_at)
             VALUES (5, 1, ?1, ?2, 0, 2, 3)",
            rusqlite::params![&[3u8; 16][..], &b"said before the upgrade"[..]],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let threads: i64 = conn
            .query_row("SELECT count(*) FROM threads", [], |r| r.get(0))
            .unwrap();
        let msgs: i64 = conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(threads, 1, "the rebuild emptied the threads table");
        assert_eq!(msgs, 1, "the cascade took the conversation");
    }

    /// A conversation survives the v5 rebuild.
    ///
    /// v5 rebuilds `threads` to drop a UNIQUE constraint, and four tables
    /// reference it. Every test until now started from a *fresh* schema at the
    /// latest version, so none of them ever carried a message across a
    /// migration — which is exactly the thing a table rebuild can lose.
    #[test]
    fn messages_survive_the_threads_rebuild() {
        let conn = Connection::open_in_memory().unwrap();
        at_version(&conn, 3);
        conn.execute(
            "INSERT INTO contacts (id, pseudonym, l2_pub, name, colour, persona_version, first_seen)
             VALUES (1, ?1, ?2, 'Ada', 0, 1, 1)",
            rusqlite::params![&[7u8; 32][..], &[8u8; 32][..]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, contact_id, created_at) VALUES (5, 1, 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, seq, msg_id, body, direction, state, created_at)
             VALUES (5, 1, ?1, ?2, 0, 2, 3)",
            rusqlite::params![&[3u8; 16][..], &b"said before the upgrade"[..]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ratchets (thread_id, state, updated_at) VALUES (5, ?1, 4)",
            rusqlite::params![&b"keys"[..]],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let threads: i64 = conn
            .query_row("SELECT count(*) FROM threads WHERE id = 5", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(threads, 1, "the thread did not come across");
        let msgs: i64 = conn
            .query_row(
                "SELECT count(*) FROM messages WHERE thread_id = 5",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msgs, 1, "the conversation was lost in the rebuild");
        let ratchets: i64 = conn
            .query_row(
                "SELECT count(*) FROM ratchets WHERE thread_id = 5",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ratchets, 1, "the thread's keys were lost in the rebuild");
    }

    /// The v2 migration has to move data, not just columns.
    ///
    /// What was in `contacts.l1_pub` was never a Layer-1 key, so it belongs in
    /// `pseudonym` — and the rows must come out claiming **no** pairing, because
    /// none of them was ever paired. A migration that carried `paired_at`
    /// across would leave every existing contact asserting a ceremony that did
    /// not happen, which is the thing this change exists to stop.
    #[test]
    fn v1_contacts_become_unpaired_contacts_keyed_by_pseudonym() {
        let conn = Connection::open_in_memory().unwrap();
        at_version(&conn, 1);
        conn.execute(
            "INSERT INTO contacts (id, l1_pub, l2_pub, name, colour, persona_version, paired_at)
             VALUES (1, ?1, ?2, 'Alice', 255, 3, 1700)",
            rusqlite::params![&[7u8; 32][..], &[8u8; 32][..]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, contact_id, created_at) VALUES (1, 1, 1701)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), LATEST_VERSION);

        let (pseudonym, name, first_seen): (Vec<u8>, String, i64) = conn
            .query_row(
                "SELECT pseudonym, name, first_seen FROM contacts WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            pseudonym,
            vec![7u8; 32],
            "the old key did not become the pseudonym"
        );
        assert_eq!(name, "Alice");
        assert_eq!(first_seen, 1700, "the old timestamp was lost");

        let pairings: i64 = conn
            .query_row("SELECT count(*) FROM pairings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pairings, 0, "migration invented a pairing");
    }

    /// The rename must not sever `threads.contact_id`, which is the reason this
    /// migration renames columns rather than rebuilding the table.
    #[test]
    fn the_rename_leaves_foreign_keys_intact() {
        let conn = Connection::open_in_memory().unwrap();
        at_version(&conn, 1);
        conn.execute(
            "INSERT INTO contacts (id, l1_pub, l2_pub, name, colour, persona_version, paired_at)
             VALUES (1, ?1, ?2, 'Alice', 0, 1, 1)",
            rusqlite::params![&[7u8; 32][..], &[8u8; 32][..]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, contact_id, created_at) VALUES (1, 1, 2)",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();

        // No dangling references anywhere in the database...
        let mut check = conn.prepare("PRAGMA foreign_key_check").unwrap();
        assert_eq!(check.query_map([], |_| Ok(())).unwrap().count(), 0);
        // ...and the cascade still reaches, which a rewritten reference would
        // silently have stopped doing.
        conn.execute("DELETE FROM contacts WHERE id = 1", [])
            .unwrap();
        let threads: i64 = conn
            .query_row("SELECT count(*) FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(threads, 0, "deleting a contact left its thread behind");
    }

    /// The columns the record queries name, spelled out.
    ///
    /// This replaces a test that compared a fresh database against an upgraded
    /// one. That test could not fail: a fresh `migrate` replays every step in
    /// the same order the upgrade path finishes with, so the two are equal by
    /// construction, and mutation testing showed it passing happily against a
    /// migration with a junk table added. What is actually worth pinning is
    /// that the migrated shape is the one `records.rs` queries — a rename typo
    /// here is a runtime failure everywhere, and nowhere at compile time.
    #[test]
    fn the_migrated_schema_has_the_columns_the_record_queries_name() {
        fn columns(conn: &Connection, table: &str) -> Vec<String> {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        }

        let conn = Connection::open_in_memory().unwrap();
        at_version(&conn, 1);
        migrate(&conn).unwrap();

        assert_eq!(
            columns(&conn, "contacts"),
            [
                "id",
                "pseudonym",
                "l2_pub",
                "name",
                "colour",
                "persona_version",
                "first_seen"
            ]
        );
        assert_eq!(
            columns(&conn, "pairings"),
            ["contact_id", "l1_pub", "paired_at"]
        );
        assert_eq!(
            columns(&conn, "ratchets"),
            ["thread_id", "state", "updated_at"]
        );
        assert_eq!(columns(&conn, "inboxes"), ["thread_id", "through", "ahead"]);
    }

    /// A database that predates the receive-state tables gets them, and keeps
    /// what was in it.
    ///
    /// The upgrade path matters more here than the tables do: someone running
    /// this build already has conversations, and a migration that dropped them
    /// to make room for a ratchet would be the worst possible way to add
    /// persistence to a chat app.
    #[test]
    fn an_older_database_gains_the_receive_tables_without_losing_its_threads() {
        let conn = Connection::open_in_memory().unwrap();
        at_version(&conn, 2);
        conn.execute_batch(
            "INSERT INTO contacts (id, pseudonym, l2_pub, name, colour, persona_version, first_seen)
             VALUES (1, x'01', x'02', 'Alice', 0, 1, 100);
             INSERT INTO threads (id, contact_id, created_at) VALUES (7, 1, 200);
             INSERT INTO messages (thread_id, seq, msg_id, body, direction, state, created_at)
             VALUES (7, 1, x'aa', x'bb', 0, 0, 300);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        assert_eq!(current_version(&conn).unwrap(), LATEST_VERSION);
        let messages: i64 = conn
            .query_row(
                "SELECT count(*) FROM messages WHERE thread_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(messages, 1, "the upgrade lost an existing conversation");
        // The new tables exist and are empty, which is what "no ratchet yet"
        // has to look like for a thread that predates them.
        let ratchets: i64 = conn
            .query_row("SELECT count(*) FROM ratchets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ratchets, 0);
    }
}
