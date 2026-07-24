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

    // ── messages ────────────────────────────────────────────────────────────

    /// Insert a message, or report it as a duplicate if its `msg_id` is already
    /// present (dedup for at-least-once delivery, tech spec §8).
    pub fn add_message(&self, m: &NewMessage) -> Result<InsertOutcome, StoreError> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO messages
             (thread_id, seq, msg_id, body, direction, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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

    /// Messages of a thread, ordered by per-sender `seq` (tech spec §8: order by
    /// seq, never by arrival).
    pub fn messages_for_thread(&self, thread_id: i64) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, seq, msg_id, body, direction, state, created_at
             FROM messages WHERE thread_id = ?1 ORDER BY seq, id",
        )?;
        let rows = stmt.query_map(params![thread_id], map_message)?;
        rows.map(|r| r?).collect()
    }

    pub fn set_message_state(&self, msg_id: &[u8], state: MessageState) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE messages SET state = ?1 WHERE msg_id = ?2",
            params![state.to_i64(), msg_id],
        )?;
        Ok(())
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
            colour: colour as u32,
            persona_version: persona_version as u32,
            paired_at,
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
        s.set_message_state(b"m", MessageState::Delivered).unwrap();
        assert_eq!(
            s.messages_for_thread(t).unwrap()[0].state,
            MessageState::Delivered
        );
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
