//! Typed access to the store's tables. These `impl Store` blocks live in a
//! child module so they can reach `Store`'s private connection.
//!
//! Timestamps (`*_at`) are passed in by the caller (the core owns the clock),
//! keeping the store deterministic and unit-testable.

use rusqlite::{params, OptionalExtension};

use super::{Store, StoreError};

/// Message direction relative to this device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Incoming,
    Outgoing,
}

impl Direction {
    fn to_i64(self) -> i64 {
        match self {
            Direction::Incoming => 0,
            Direction::Outgoing => 1,
        }
    }

    fn from_i64(v: i64) -> Result<Self, StoreError> {
        match v {
            0 => Ok(Direction::Incoming),
            1 => Ok(Direction::Outgoing),
            _ => Err(StoreError::Db(format!("bad direction {v}"))),
        }
    }
}

/// Delivery state of a message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageState {
    Queued,
    Sent,
    Delivered,
}

impl MessageState {
    fn to_i64(self) -> i64 {
        match self {
            MessageState::Queued => 0,
            MessageState::Sent => 1,
            MessageState::Delivered => 2,
        }
    }

    fn from_i64(v: i64) -> Result<Self, StoreError> {
        match v {
            0 => Ok(MessageState::Queued),
            1 => Ok(MessageState::Sent),
            2 => Ok(MessageState::Delivered),
            _ => Err(StoreError::Db(format!("bad message state {v}"))),
        }
    }
}

/// State of a file transfer (tech spec §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransferState {
    Offered,
    Active,
    Complete,
    Failed,
}

impl TransferState {
    fn to_i64(self) -> i64 {
        match self {
            TransferState::Offered => 0,
            TransferState::Active => 1,
            TransferState::Complete => 2,
            TransferState::Failed => 3,
        }
    }

    fn from_i64(v: i64) -> Result<Self, StoreError> {
        match v {
            0 => Ok(TransferState::Offered),
            1 => Ok(TransferState::Active),
            2 => Ok(TransferState::Complete),
            3 => Ok(TransferState::Failed),
            _ => Err(StoreError::Db(format!("bad transfer state {v}"))),
        }
    }
}

/// A file transfer and its resumable chunk bitmap (tech spec §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub id: i64,
    pub thread_id: Option<i64>,
    pub direction: Direction,
    pub name: String,
    pub size: i64,
    pub mime: String,
    pub state: TransferState,
    pub root_hash: [u8; 32],
    pub chunk_bitmap: Vec<u8>,
    pub created_at: i64,
}

/// Fields for inserting a new transfer.
pub struct NewTransfer {
    pub thread_id: Option<i64>,
    pub direction: Direction,
    pub name: String,
    pub size: i64,
    pub mime: String,
    pub state: TransferState,
    pub root_hash: [u8; 32],
    pub chunk_bitmap: Vec<u8>,
    pub created_at: i64,
}

/// A paired identity (a Circle seed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: i64,
    pub l1_pub: [u8; 32],
    pub l2_pub: [u8; 32],
    pub name: String,
    pub colour: u32,
    pub persona_version: u32,
    pub paired_at: i64,
}

/// Fields for inserting a new contact.
pub struct NewContact {
    pub l1_pub: [u8; 32],
    pub l2_pub: [u8; 32],
    pub name: String,
    pub colour: u32,
    pub persona_version: u32,
    pub paired_at: i64,
}

/// A stored message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    pub thread_id: i64,
    pub seq: i64,
    pub msg_id: Vec<u8>,
    pub body: Vec<u8>,
    pub direction: Direction,
    pub state: MessageState,
    pub created_at: i64,
}

/// Fields for inserting a new message.
pub struct NewMessage {
    pub thread_id: i64,
    pub seq: i64,
    pub msg_id: Vec<u8>,
    pub body: Vec<u8>,
    pub direction: Direction,
    pub state: MessageState,
    pub created_at: i64,
}

/// Outcome of inserting a message: either it was stored, or its `msg_id` was
/// already present (the dedup case from at-least-once delivery).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InsertOutcome {
    Inserted(i64),
    Duplicate,
}

/// A block-list entry (keyed by the per-counterparty pseudonym).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEntry {
    pub pseudonym: [u8; 32],
    pub created_at: i64,
    pub revoked_pairing: bool,
}

fn blob32(v: Vec<u8>) -> Result<[u8; 32], StoreError> {
    v.try_into()
        .map_err(|_| StoreError::Db("expected 32-byte blob".into()))
}

/// Convert a stored `i64` back to `u32`, rejecting out-of-range values rather
/// than wrapping — defensive against a corrupt row.
fn u32_from_i64(v: i64) -> Result<u32, StoreError> {
    u32::try_from(v).map_err(|_| StoreError::Db(format!("value {v} out of u32 range")))
}

impl Store {
    // ── contacts ────────────────────────────────────────────────────────────

    pub fn add_contact(&self, c: &NewContact) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO contacts (l1_pub, l2_pub, name, colour, persona_version, paired_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &c.l1_pub[..],
                &c.l2_pub[..],
                c.name,
                c.colour as i64,
                c.persona_version as i64,
                c.paired_at
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn contact_by_l1(&self, l1_pub: &[u8; 32]) -> Result<Option<Contact>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, l1_pub, l2_pub, name, colour, persona_version, paired_at
                 FROM contacts WHERE l1_pub = ?1",
                params![&l1_pub[..]],
                map_contact,
            )
            .optional()?
            .transpose()
    }

    pub fn contact_by_id(&self, id: i64) -> Result<Option<Contact>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, l1_pub, l2_pub, name, colour, persona_version, paired_at
                 FROM contacts WHERE id = ?1",
                params![id],
                map_contact,
            )
            .optional()?
            .transpose()
    }

    /// Move a contact onto a different Layer-1 key, keeping its id, thread and
    /// history.
    ///
    /// Needed because a contact can be created before there is anything durable
    /// to name it by. A device id is all that exists until a session does, and
    /// R0-F2 rotates that every twelve minutes — so a row first keyed on the id
    /// has to be adopted onto the real key rather than left behind, or one
    /// person ends up as two contacts with the conversation split between them.
    ///
    /// `l1_pub` is UNIQUE, so the caller must have established that no row
    /// already holds `new_l1`; this reports a match rather than deciding.
    pub fn rekey_contact(&self, id: i64, new_l1: &[u8; 32]) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE contacts SET l1_pub = ?1 WHERE id = ?2",
            params![&new_l1[..], id],
        )?;
        Ok(n > 0)
    }

    /// Update a contact's persona fields (their name/colour/version changed via
    /// a persona record). Returns whether a row matched.
    pub fn update_contact_persona(
        &self,
        id: i64,
        name: &str,
        colour: u32,
        persona_version: u32,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE contacts SET name = ?1, colour = ?2, persona_version = ?3 WHERE id = ?4",
            params![name, colour as i64, persona_version as i64, id],
        )?;
        Ok(changed > 0)
    }

    pub fn list_contacts(&self) -> Result<Vec<Contact>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, l1_pub, l2_pub, name, colour, persona_version, paired_at
             FROM contacts ORDER BY paired_at",
        )?;
        let rows = stmt.query_map([], map_contact)?;
        rows.map(|r| r?).collect()
    }

    // ── threads ─────────────────────────────────────────────────────────────

    pub fn create_thread(&self, contact_id: i64, created_at: i64) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO threads (contact_id, created_at) VALUES (?1, ?2)",
            params![contact_id, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn thread_for_contact(&self, contact_id: i64) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT id FROM threads WHERE contact_id = ?1",
                params![contact_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// All threads as `(thread_id, contact_id)`, newest first — for the T07
    /// conversation list.
    pub fn list_threads(&self) -> Result<Vec<(i64, i64)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, contact_id FROM threads ORDER BY created_at DESC, id DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    // ── messages ────────────────────────────────────────────────────────────

    /// Insert a message, or report it as a duplicate if its `msg_id` is already
    /// present (dedup for at-least-once delivery, tech spec §8).
    ///
    /// The ignore is scoped to the `msg_id` conflict, so a foreign-key or
    /// NOT NULL violation errors rather than masquerading as a duplicate.
    pub fn add_message(&self, m: &NewMessage) -> Result<InsertOutcome, StoreError> {
        let changed = self.conn.execute(
            "INSERT INTO messages
             (thread_id, seq, msg_id, body, direction, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(msg_id) DO NOTHING",
            params![
                m.thread_id,
                m.seq,
                m.msg_id,
                m.body,
                m.direction.to_i64(),
                m.state.to_i64(),
                m.created_at
            ],
        )?;
        if changed == 0 {
            Ok(InsertOutcome::Duplicate)
        } else {
            Ok(InsertOutcome::Inserted(self.conn.last_insert_rowid()))
        }
    }

    /// Look up a single message by its `msg_id`.
    pub fn message_by_msg_id(&self, msg_id: &[u8]) -> Result<Option<Message>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, thread_id, seq, msg_id, body, direction, state, created_at
                 FROM messages WHERE msg_id = ?1",
                params![msg_id],
                map_message,
            )
            .optional()?
            .transpose()
    }

    /// Messages in a given delivery state, oldest first — the T12 send queue
    /// (e.g. `messages_by_state(MessageState::Queued)` then filter Outgoing).
    pub fn messages_by_state(&self, state: MessageState) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, seq, msg_id, body, direction, state, created_at
             FROM messages WHERE state = ?1 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(params![state.to_i64()], map_message)?;
        rows.map(|r| r?).collect()
    }

    /// The next `seq` to assign for one sender's stream in a thread (max seq in
    /// that direction + 1). Per-sender because incoming and outgoing number
    /// independently (tech spec §8).
    pub fn next_seq(&self, thread_id: i64, direction: Direction) -> Result<i64, StoreError> {
        let max: Option<i64> = self.conn.query_row(
            "SELECT max(seq) FROM messages WHERE thread_id = ?1 AND direction = ?2",
            params![thread_id, direction.to_i64()],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) + 1)
    }

    /// Messages of a thread, ordered by the stored `(seq, id)`. `seq` is
    /// per-sender (tech spec §8), so a display layer filters or merges by
    /// direction; this returns the raw rows.
    pub fn messages_for_thread(&self, thread_id: i64) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, seq, msg_id, body, direction, state, created_at
             FROM messages WHERE thread_id = ?1 ORDER BY seq, id",
        )?;
        let rows = stmt.query_map(params![thread_id], map_message)?;
        rows.map(|r| r?).collect()
    }

    /// Update a message's delivery state. Returns whether a row matched, so a
    /// state transition against an unknown `msg_id` is detectable.
    pub fn set_message_state(
        &self,
        msg_id: &[u8],
        state: MessageState,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE messages SET state = ?1 WHERE msg_id = ?2",
            params![state.to_i64(), msg_id],
        )?;
        Ok(changed > 0)
    }

    // ── blocklist ───────────────────────────────────────────────────────────

    pub fn block(
        &self,
        pseudonym: &[u8; 32],
        created_at: i64,
        revoked_pairing: bool,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blocklist (pseudonym, created_at, revoked_pairing)
             VALUES (?1, ?2, ?3)",
            params![&pseudonym[..], created_at, revoked_pairing as i64],
        )?;
        Ok(())
    }

    pub fn is_blocked(&self, pseudonym: &[u8; 32]) -> Result<bool, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM blocklist WHERE pseudonym = ?1",
            params![&pseudonym[..]],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn unblock(&self, pseudonym: &[u8; 32]) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM blocklist WHERE pseudonym = ?1",
            params![&pseudonym[..]],
        )?;
        Ok(())
    }

    pub fn list_blocks(&self) -> Result<Vec<BlockEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT pseudonym, created_at, revoked_pairing FROM blocklist ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (pseudonym, created_at, revoked) = r?;
            Ok(BlockEntry {
                pseudonym: blob32(pseudonym)?,
                created_at,
                revoked_pairing: revoked != 0,
            })
        })
        .collect()
    }

    // ── settings ────────────────────────────────────────────────────────────

    pub fn settings_set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn settings_get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    // ── transfers ─────────────────────────────────────────────────────────────

    pub fn add_transfer(&self, t: &NewTransfer) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO transfers
             (thread_id, direction, name, size, mime, state, root_hash, chunk_bitmap, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                t.thread_id,
                t.direction.to_i64(),
                t.name,
                t.size,
                t.mime,
                t.state.to_i64(),
                &t.root_hash[..],
                t.chunk_bitmap,
                t.created_at
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn transfer_by_id(&self, id: i64) -> Result<Option<Transfer>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, thread_id, direction, name, size, mime, state, root_hash, chunk_bitmap, created_at
                 FROM transfers WHERE id = ?1",
                params![id],
                map_transfer,
            )
            .optional()?
            .transpose()
    }

    pub fn set_transfer_state(&self, id: i64, state: TransferState) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE transfers SET state = ?1 WHERE id = ?2",
            params![state.to_i64(), id],
        )?;
        Ok(changed > 0)
    }

    /// Update the resumable chunk bitmap for a transfer (T16 resume).
    pub fn set_chunk_bitmap(&self, id: i64, bitmap: &[u8]) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE transfers SET chunk_bitmap = ?1 WHERE id = ?2",
            params![bitmap, id],
        )?;
        Ok(changed > 0)
    }

    pub fn list_transfers(&self) -> Result<Vec<Transfer>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, direction, name, size, mime, state, root_hash, chunk_bitmap, created_at
             FROM transfers ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], map_transfer)?;
        rows.map(|r| r?).collect()
    }
}

fn map_transfer(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Transfer, StoreError>> {
    let id = r.get(0)?;
    let thread_id = r.get(1)?;
    let direction: i64 = r.get(2)?;
    let name = r.get(3)?;
    let size = r.get(4)?;
    let mime = r.get(5)?;
    let state: i64 = r.get(6)?;
    let root_hash: Vec<u8> = r.get(7)?;
    let chunk_bitmap = r.get(8)?;
    let created_at = r.get(9)?;
    Ok((|| {
        Ok(Transfer {
            id,
            thread_id,
            direction: Direction::from_i64(direction)?,
            name,
            size,
            mime,
            state: TransferState::from_i64(state)?,
            root_hash: blob32(root_hash)?,
            chunk_bitmap,
            created_at,
        })
    })())
}

fn map_contact(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Contact, StoreError>> {
    let id = r.get(0)?;
    let l1: Vec<u8> = r.get(1)?;
    let l2: Vec<u8> = r.get(2)?;
    let name = r.get(3)?;
    let colour: i64 = r.get(4)?;
    let persona_version: i64 = r.get(5)?;
    let paired_at = r.get(6)?;
    Ok((|| {
        Ok(Contact {
            id,
            l1_pub: blob32(l1)?,
            l2_pub: blob32(l2)?,
            name,
            colour: u32_from_i64(colour)?,
            persona_version: u32_from_i64(persona_version)?,
            paired_at,
        })
    })())
}

fn map_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Message, StoreError>> {
    let id = r.get(0)?;
    let thread_id = r.get(1)?;
    let seq = r.get(2)?;
    let msg_id: Vec<u8> = r.get(3)?;
    let body: Vec<u8> = r.get(4)?;
    let direction: i64 = r.get(5)?;
    let state: i64 = r.get(6)?;
    let created_at = r.get(7)?;
    Ok((|| {
        Ok(Message {
            id,
            thread_id,
            seq,
            msg_id,
            body,
            direction: Direction::from_i64(direction)?,
            state: MessageState::from_i64(state)?,
            created_at,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keystore::{Keystore, SoftwareKeystore};
    use std::sync::Arc;

    fn store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        let s = Store::open(ks, dir.path().join("db"), dir.path().join("files")).unwrap();
        (s, dir)
    }

    fn a_contact() -> NewContact {
        NewContact {
            l1_pub: [1u8; 32],
            l2_pub: [2u8; 32],
            name: "Alice".into(),
            colour: 0x00ff_8800,
            persona_version: 1,
            paired_at: 1000,
        }
    }

    /// Re-keying keeps the person: same row, same thread, same history.
    ///
    /// A contact can be created before there is anything durable to name it by
    /// — only a device id, which R0-F2 rotates every twelve minutes. When a
    /// session finally supplies the real key, the existing row has to move onto
    /// it. Adding a second row instead would split one conversation across two
    /// contacts, which is the bug this exists to prevent.
    #[test]
    fn rekeying_a_contact_keeps_its_thread_and_messages() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let thread = s.create_thread(id, 1000).unwrap();
        s.add_message(&NewMessage {
            thread_id: thread,
            seq: 1,
            msg_id: vec![7u8; 16],
            body: b"before the session".to_vec(),
            direction: Direction::Outgoing,
            state: MessageState::Sent,
            created_at: 1000,
        })
        .unwrap();

        assert!(s.rekey_contact(id, &[42u8; 32]).unwrap());

        assert_eq!(
            s.contact_by_l1(&[42u8; 32]).unwrap().unwrap().id,
            id,
            "the contact did not move onto the real key"
        );
        assert!(
            s.contact_by_l1(&[1u8; 32]).unwrap().is_none(),
            "the rotating id still resolves, so the next lookup makes a duplicate"
        );
        assert_eq!(
            s.thread_for_contact(id).unwrap(),
            Some(thread),
            "the thread came adrift from its contact"
        );
        assert_eq!(
            s.messages_for_thread(thread).unwrap().len(),
            1,
            "history written before the session was lost"
        );
    }

    #[test]
    fn rekeying_a_contact_that_is_not_there_reports_it() {
        let (s, _d) = store();
        assert!(!s.rekey_contact(9999, &[42u8; 32]).unwrap());
    }

    #[test]
    fn contact_round_trips_and_lists() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let got = s.contact_by_l1(&[1u8; 32]).unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.name, "Alice");
        assert_eq!(got.l2_pub, [2u8; 32]);
        assert_eq!(s.list_contacts().unwrap().len(), 1);
        assert!(s.contact_by_l1(&[9u8; 32]).unwrap().is_none());
    }

    #[test]
    fn thread_created_and_found_for_contact() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 1001).unwrap();
        assert_eq!(s.thread_for_contact(c).unwrap(), Some(t));
    }

    #[test]
    fn messages_dedup_and_order_by_seq() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 0).unwrap();

        let msg = |seq, id: &[u8]| NewMessage {
            thread_id: t,
            seq,
            msg_id: id.to_vec(),
            body: b"hi".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 0,
        };

        // Insert out of order; duplicate msg_id is ignored.
        assert!(matches!(
            s.add_message(&msg(2, b"m2")).unwrap(),
            InsertOutcome::Inserted(_)
        ));
        assert!(matches!(
            s.add_message(&msg(1, b"m1")).unwrap(),
            InsertOutcome::Inserted(_)
        ));
        assert_eq!(
            s.add_message(&msg(2, b"m2")).unwrap(),
            InsertOutcome::Duplicate
        );

        let msgs = s.messages_for_thread(t).unwrap();
        assert_eq!(msgs.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn message_state_updates() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 0).unwrap();
        s.add_message(&NewMessage {
            thread_id: t,
            seq: 1,
            msg_id: b"m".to_vec(),
            body: b"x".to_vec(),
            direction: Direction::Outgoing,
            state: MessageState::Queued,
            created_at: 0,
        })
        .unwrap();
        assert!(s.set_message_state(b"m", MessageState::Delivered).unwrap());
        assert_eq!(
            s.messages_for_thread(t).unwrap()[0].state,
            MessageState::Delivered
        );
        // Unknown msg_id: no row matched.
        assert!(!s.set_message_state(b"nope", MessageState::Sent).unwrap());
    }

    #[test]
    fn add_message_with_bad_thread_id_errors_not_duplicate() {
        let (s, _d) = store();
        // No such thread: an FK violation must surface as an error, never
        // masquerade as InsertOutcome::Duplicate.
        let r = s.add_message(&NewMessage {
            thread_id: 9999,
            seq: 1,
            msg_id: b"x".to_vec(),
            body: b"y".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 0,
        });
        assert!(r.is_err());
    }

    #[test]
    fn messages_ordered_by_seq_with_gaps() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 0).unwrap();
        for (seq, id) in [(5i64, b"a" as &[u8]), (1, b"b"), (3, b"c")] {
            s.add_message(&NewMessage {
                thread_id: t,
                seq,
                msg_id: id.to_vec(),
                body: b"x".to_vec(),
                direction: Direction::Incoming,
                state: MessageState::Delivered,
                created_at: 0,
            })
            .unwrap();
        }
        assert_eq!(
            s.messages_for_thread(t)
                .unwrap()
                .iter()
                .map(|m| m.seq)
                .collect::<Vec<_>>(),
            vec![1, 3, 5]
        );
        // Per-sender: incoming seq continues from 5, outgoing starts fresh.
        assert_eq!(s.next_seq(t, Direction::Incoming).unwrap(), 6);
        assert_eq!(s.next_seq(t, Direction::Outgoing).unwrap(), 1);
        assert_eq!(s.next_seq(999, Direction::Incoming).unwrap(), 1); // empty thread starts at 1
    }

    #[test]
    fn messages_by_state_finds_the_send_queue() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 0).unwrap();
        let mk = |seq, id: &[u8], state| NewMessage {
            thread_id: t,
            seq,
            msg_id: id.to_vec(),
            body: b"x".to_vec(),
            direction: Direction::Outgoing,
            state,
            created_at: seq,
        };
        s.add_message(&mk(1, b"q1", MessageState::Queued)).unwrap();
        s.add_message(&mk(2, b"d", MessageState::Delivered))
            .unwrap();
        s.add_message(&mk(3, b"q2", MessageState::Queued)).unwrap();
        let queued = s.messages_by_state(MessageState::Queued).unwrap();
        assert_eq!(
            queued.iter().map(|m| m.msg_id.clone()).collect::<Vec<_>>(),
            vec![b"q1".to_vec(), b"q2".to_vec()]
        );
        assert_eq!(
            s.message_by_msg_id(b"d").unwrap().unwrap().state,
            MessageState::Delivered
        );
    }

    #[test]
    fn transfer_round_trips_with_state_and_bitmap_updates() {
        let (s, _d) = store();
        let id = s
            .add_transfer(&NewTransfer {
                thread_id: None,
                direction: Direction::Incoming,
                name: "clip.mp4".into(),
                size: 5_000_000,
                mime: "video/mp4".into(),
                state: TransferState::Offered,
                root_hash: [9u8; 32],
                chunk_bitmap: vec![0x00],
                created_at: 1,
            })
            .unwrap();
        let t = s.transfer_by_id(id).unwrap().unwrap();
        assert_eq!(t.name, "clip.mp4");
        assert_eq!(t.state, TransferState::Offered);

        assert!(s.set_transfer_state(id, TransferState::Active).unwrap());
        assert!(s.set_chunk_bitmap(id, &[0xff, 0x0f]).unwrap());
        let t = s.transfer_by_id(id).unwrap().unwrap();
        assert_eq!(t.state, TransferState::Active);
        assert_eq!(t.chunk_bitmap, vec![0xff, 0x0f]);
        assert_eq!(s.list_transfers().unwrap().len(), 1);
    }

    #[test]
    fn contact_by_id_and_persona_update() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        assert_eq!(s.contact_by_id(id).unwrap().unwrap().name, "Alice");
        assert!(s.update_contact_persona(id, "Alicia", 0x010203, 2).unwrap());
        let c = s.contact_by_id(id).unwrap().unwrap();
        assert_eq!(c.name, "Alicia");
        assert_eq!(c.persona_version, 2);
    }

    #[test]
    fn blocklist_add_check_remove() {
        let (s, _d) = store();
        let p = [3u8; 32];
        assert!(!s.is_blocked(&p).unwrap());
        s.block(&p, 5, true).unwrap();
        assert!(s.is_blocked(&p).unwrap());
        assert!(s.list_blocks().unwrap()[0].revoked_pairing);
        s.unblock(&p).unwrap();
        assert!(!s.is_blocked(&p).unwrap());
    }

    #[test]
    fn settings_round_trip_and_replace() {
        let (s, _d) = store();
        assert!(s.settings_get("k").unwrap().is_none());
        s.settings_set("k", b"v1").unwrap();
        s.settings_set("k", b"v2").unwrap();
        assert_eq!(s.settings_get("k").unwrap().as_deref(), Some(&b"v2"[..]));
    }

    #[test]
    fn foreign_key_cascade_deletes_messages_with_contact() {
        let (s, _d) = store();
        let c = s.add_contact(&a_contact()).unwrap();
        let t = s.create_thread(c, 0).unwrap();
        s.add_message(&NewMessage {
            thread_id: t,
            seq: 1,
            msg_id: b"m".to_vec(),
            body: b"x".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 0,
        })
        .unwrap();
        s.conn
            .execute("DELETE FROM contacts WHERE id = ?1", params![c])
            .unwrap();
        assert!(s.messages_for_thread(t).unwrap().is_empty());
    }
}
