//! Typed access to the store's tables. These `impl Store` blocks live in a
//! child module so they can reach `Store`'s private connection.
//!
//! Timestamps (`*_at`) are passed in by the caller (the core owns the clock),
//! keeping the store deterministic and unit-testable.

use rusqlite::{params, OptionalExtension};
use zeroize::Zeroizing;

use crate::session::chat::MAX_AHEAD;

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

/// Someone we have written down.
///
/// Not necessarily someone we have paired with — that is [`Pairing`], and most
/// contacts do not have one. A contact is the durable side of a person we have
/// met: a name, a colour, and a handle stable enough to hang a conversation on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: i64,
    /// The peer's per-counterparty pseudonym — the static their session
    /// handshake proved, or, before any session, a stand-in derived from the
    /// rotating device id.
    ///
    /// **Not a Layer-1 key**, and the column was called `l1_pub` until the
    /// ceremony made a real one possible. See the v2 migration.
    pub pseudonym: [u8; 32],
    pub l2_pub: [u8; 32],
    pub name: String,
    pub colour: u32,
    pub persona_version: u32,
    /// When this row was created. Was `paired_at`, which it never was.
    pub first_seen: i64,
}

/// Fields for inserting a new contact.
pub struct NewContact {
    pub pseudonym: [u8; 32],
    pub l2_pub: [u8; 32],
    pub name: String,
    pub colour: u32,
    pub persona_version: u32,
    pub first_seen: i64,
}

/// A completed pairing ceremony — the Circle seed (R0-F4).
///
/// Its own table, and its own type, because the existence of this row *is* the
/// claim that two people stood next to each other and both pressed confirm.
/// Nothing else in the schema can make that claim by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub contact_id: i64,
    /// The counterpart's Layer-1 public identity, from inside the ceremony
    /// channel. The only place in the store one of these appears.
    pub l1_pub: [u8; 32],
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

/// How far a thread's incoming stream has got: the contiguous run, and the
/// messages past it that have arrived out of order.
///
/// The store's shape of `session::chat::Inbox`, kept as a plain pair of numbers
/// rather than the type itself so the store does not have to know what an inbox
/// means — only what it is made of. The session layer holds the rules; this
/// holds the two values they operate on.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct InboxPosition {
    pub through: u64,
    pub ahead: Vec<u64>,
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
            "INSERT INTO contacts (pseudonym, l2_pub, name, colour, persona_version, first_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &c.pseudonym[..],
                &c.l2_pub[..],
                c.name,
                c.colour as i64,
                c.persona_version as i64,
                c.first_seen
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn contact_by_pseudonym(
        &self,
        pseudonym: &[u8; 32],
    ) -> Result<Option<Contact>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, pseudonym, l2_pub, name, colour, persona_version, first_seen
                 FROM contacts WHERE pseudonym = ?1",
                params![&pseudonym[..]],
                map_contact,
            )
            .optional()?
            .transpose()
    }

    pub fn contact_by_id(&self, id: i64) -> Result<Option<Contact>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, pseudonym, l2_pub, name, colour, persona_version, first_seen
                 FROM contacts WHERE id = ?1",
                params![id],
                map_contact,
            )
            .optional()?
            .transpose()
    }

    /// Move a contact onto a different pseudonym, keeping its id, thread and
    /// history.
    ///
    /// Needed because a contact can be created before there is anything durable
    /// to name it by. A device id is all that exists until a session does, and
    /// R0-F2 rotates that every twelve minutes — so a row first keyed on the id
    /// has to be adopted onto the real key rather than left behind, or one
    /// person ends up as two contacts with the conversation split between them.
    ///
    /// `pseudonym` is UNIQUE, so the caller must have established that no row
    /// already holds `new_pseudonym`; this reports a match rather than deciding.
    pub fn rekey_contact(&self, id: i64, new_pseudonym: &[u8; 32]) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE contacts SET pseudonym = ?1 WHERE id = ?2",
            params![&new_pseudonym[..], id],
        )?;
        Ok(n > 0)
    }

    /// Fold one contact into another: its messages, its transfers, then the row.
    ///
    /// Needed when the same person has been written down twice — once under a
    /// durable key and once under a device id, because a message was queued
    /// after the id rotated but before the next session proved who they were.
    /// Re-keying cannot help there: the destination key is already taken, so
    /// without this the older conversation is simply orphaned.
    ///
    /// Messages are renumbered to continue the destination's `seq` per
    /// direction rather than keeping their own. `messages_for_thread` orders by
    /// `(seq, id)`, so two runs of 1, 2, 3 would interleave — the two halves of
    /// one conversation shuffled together instead of the newer following the
    /// older.
    ///
    /// Atomic. Four statements across three tables rewrite where a person's
    /// history lives, and a failure between any two of them is worse than the
    /// duplicate being fixed: messages moved but the old row still standing, or
    /// transfers left pointing at a thread that has been deleted. `?` drops the
    /// transaction unbuilt, which rolls back.
    pub fn merge_contact(&self, from: i64, into: i64) -> Result<(), StoreError> {
        if from == into {
            return Ok(());
        }
        // Both ends checked, not assumed.
        //
        // `into`, because with no thread on `from` nothing below touches a
        // foreign key: a bad destination would sail through to the DELETE and
        // quietly destroy the source instead of merging it, which is the one
        // outcome worse than the duplicate.
        //
        // `from`, because otherwise this is a silent success — the match does
        // nothing, the DELETE hits no rows, and a caller that had the wrong id
        // is told the merge happened. `rekey_contact` reports whether it
        // matched; a folding operation that says nothing is worse, not better.
        for (which, id) in [("into", into), ("from", from)] {
            if self.contact_by_id(id)?.is_none() {
                return Err(StoreError::Db(format!(
                    "cannot merge contact {from} into {into}: no such {which} contact {id}"
                )));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        match (
            self.thread_for_contact(from)?,
            self.thread_for_contact(into)?,
        ) {
            (Some(from_thread), Some(into_thread)) => {
                for dir in [Direction::Incoming, Direction::Outgoing] {
                    let offset = self.next_seq(into_thread, dir)? - 1;
                    self.conn.execute(
                        "UPDATE messages SET thread_id = ?1, seq = seq + ?2
                         WHERE thread_id = ?3 AND direction = ?4",
                        params![into_thread, offset, from_thread, dir.to_i64()],
                    )?;
                }
                self.conn.execute(
                    "UPDATE transfers SET thread_id = ?1 WHERE thread_id = ?2",
                    params![into_thread, from_thread],
                )?;
            }
            // Nothing to merge into: move the whole thread across rather than
            // letting the cascade take it with the contact.
            (Some(from_thread), None) => {
                self.conn.execute(
                    "UPDATE threads SET contact_id = ?1 WHERE id = ?2",
                    params![into, from_thread],
                )?;
            }
            (None, _) => {}
        }
        self.conn
            .execute("DELETE FROM contacts WHERE id = ?1", params![from])?;
        tx.commit()?;
        Ok(())
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
            "SELECT id, pseudonym, l2_pub, name, colour, persona_version, first_seen
             FROM contacts ORDER BY first_seen",
        )?;
        let rows = stmt.query_map([], map_contact)?;
        rows.map(|r| r?).collect()
    }

    // ── pairings ────────────────────────────────────────────────────────────

    /// Record that a ceremony completed, and open the persistent thread it
    /// entitles (R0-F4: "a completed ceremony creates a persistent thread").
    ///
    /// One transaction, because the pairing and the thread are one fact. A
    /// pairing row with no thread is a contact the UI cannot open; a thread
    /// with no pairing is a conversation claiming a ceremony that did not
    /// finish. Neither is a state anything downstream knows how to read, and
    /// both are reachable if these are two calls and the second fails.
    ///
    /// Idempotent on the pairing: re-pairing the same two people replaces the
    /// Layer-1 key and the timestamp rather than failing. That is not
    /// hypothetical tidiness — a ceremony can be run again after one side
    /// wipes (R0-F9) and regenerates, and the honest reading of the second
    /// ceremony is that it supersedes the first.
    ///
    /// Returns the thread id.
    pub fn pair_contact(
        &self,
        contact_id: i64,
        l1_pub: &[u8; 32],
        paired_at: i64,
    ) -> Result<i64, StoreError> {
        self.require_contact(contact_id)?;
        let tx = self.conn.unchecked_transaction()?;
        let thread = self.write_pairing(contact_id, l1_pub, paired_at)?;
        tx.commit()?;
        Ok(thread)
    }

    /// Everything a completed ceremony writes, in **one** transaction: the
    /// identity it proved, the persona it carried, the pairing, and the thread.
    ///
    /// One call because it is one fact. Done as four — update the Layer-2 key,
    /// update the persona, insert the pairing, open the thread — a failure
    /// partway leaves a contact wearing a name and a Layer-2 key that came from
    /// a ceremony which did not finish. Nothing downstream reads that as
    /// paired, since every paired lookup joins `pairings`, so it is not
    /// dangerous; it is simply a row asserting something that never happened,
    /// which is the class of thing this schema was just rewritten to stop.
    ///
    /// The **contact row itself** is deliberately outside: it may have existed
    /// for weeks, because people chat and Ping long before they pair (R0-F3,
    /// R0-F5), and a contact with no pairing is an ordinary thing to have
    /// rather than wreckage.
    #[allow(clippy::too_many_arguments)]
    pub fn record_pairing(
        &self,
        contact_id: i64,
        l2_pub: &[u8; 32],
        name: &str,
        colour: u32,
        persona_version: u32,
        l1_pub: &[u8; 32],
        paired_at: i64,
    ) -> Result<i64, StoreError> {
        self.require_contact(contact_id)?;
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE contacts SET l2_pub = ?1, name = ?2, colour = ?3, persona_version = ?4
             WHERE id = ?5",
            params![
                &l2_pub[..],
                name,
                colour as i64,
                persona_version as i64,
                contact_id
            ],
        )?;
        let thread = self.write_pairing(contact_id, l1_pub, paired_at)?;
        tx.commit()?;
        Ok(thread)
    }

    /// The pairing row and its thread. Assumes a transaction is already open —
    /// SQLite has no nested transactions, so this cannot open its own and both
    /// callers above must supply one.
    fn write_pairing(
        &self,
        contact_id: i64,
        l1_pub: &[u8; 32],
        paired_at: i64,
    ) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO pairings (contact_id, l1_pub, paired_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(contact_id) DO UPDATE SET l1_pub = ?2, paired_at = ?3",
            params![contact_id, &l1_pub[..], paired_at],
        )?;
        match self.thread_for_contact(contact_id)? {
            Some(t) => Ok(t),
            None => self.create_thread(contact_id, paired_at),
        }
    }

    fn require_contact(&self, contact_id: i64) -> Result<(), StoreError> {
        if self.contact_by_id(contact_id)?.is_none() {
            return Err(StoreError::Db(format!(
                "cannot pair: no such contact {contact_id}"
            )));
        }
        Ok(())
    }

    pub fn pairing_for_contact(&self, contact_id: i64) -> Result<Option<Pairing>, StoreError> {
        self.conn
            .query_row(
                "SELECT contact_id, l1_pub, paired_at FROM pairings WHERE contact_id = ?1",
                params![contact_id],
                map_pairing,
            )
            .optional()?
            .transpose()
    }

    /// The contact paired to this Layer-1 identity, if any.
    ///
    /// The lookup that makes a pairing durable across everything else: a
    /// pseudonym is per-counterparty and a device id rotates, but a Layer-1 key
    /// is the person.
    pub fn contact_by_l1(&self, l1_pub: &[u8; 32]) -> Result<Option<Contact>, StoreError> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT contact_id FROM pairings WHERE l1_pub = ?1",
                params![&l1_pub[..]],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.contact_by_id(id),
            None => Ok(None),
        }
    }

    /// The **paired** contact holding this Layer-2 key, if there is one.
    ///
    /// Joined against `pairings` rather than reading `contacts` alone, and that
    /// is not an optimisation: an unpaired contact carries a placeholder
    /// Layer-2 key, so a bare lookup on the column would match every one of
    /// them at once for the placeholder value. Only a ceremony writes a real
    /// key there, which is exactly the set this is asking about.
    pub fn paired_contact_by_l2(&self, l2_pub: &[u8; 32]) -> Result<Option<Contact>, StoreError> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT c.id FROM contacts c
                 JOIN pairings p ON p.contact_id = c.id
                 WHERE c.l2_pub = ?1",
                params![&l2_pub[..]],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.contact_by_id(id),
            None => Ok(None),
        }
    }

    /// Undo a pairing, leaving the contact and its history in place.
    ///
    /// R0-F10's "blocking a paired person revokes the pairing" needs this, and
    /// so does R0-F9 in part. Deliberately does **not** delete the thread: the
    /// messages are the person's, and a block is not a request to destroy what
    /// was said. Returns whether there was a pairing to revoke.
    pub fn unpair_contact(&self, contact_id: i64) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "DELETE FROM pairings WHERE contact_id = ?1",
            params![contact_id],
        )?;
        Ok(n > 0)
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

    /// How many of a thread's messages are in one state, without loading them.
    ///
    /// `messages_by_state` returns whole rows — bodies included, up to 8 KB
    /// each and across every thread — which is a lot of copying to answer "how
    /// many". The send path asks this on every message, so it counts in SQL.
    pub fn count_in_state(
        &self,
        thread_id: i64,
        direction: Direction,
        state: MessageState,
    ) -> Result<usize, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM messages
             WHERE thread_id = ?1 AND direction = ?2 AND state = ?3",
            params![thread_id, direction.to_i64(), state.to_i64()],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// A thread's messages in one state and direction, oldest first.
    ///
    /// Scoped in SQL rather than filtered afterwards: the resend path wants one
    /// thread's backlog, and fetching every thread's to throw most of it away
    /// is work that grows with the whole store.
    pub fn messages_in_state(
        &self,
        thread_id: i64,
        direction: Direction,
        state: MessageState,
    ) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, seq, msg_id, body, direction, state, created_at
             FROM messages WHERE thread_id = ?1 AND direction = ?2 AND state = ?3
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(
            params![thread_id, direction.to_i64(), state.to_i64()],
            map_message,
        )?;
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

    // ── thread receive state (T12) ──────────────────────────────────────────

    /// Write a thread's ratchet state and inbox position, and the message that
    /// moved them, as one fact.
    ///
    /// This is the transaction T12 calls the correctness heart of Chat, and the
    /// reason is that the three halves fail in different directions:
    ///
    /// - **Ratchet saved, message not.** The message is gone for good. Its key
    ///   was consumed by the decryption that produced it and the chain has
    ///   moved past; nothing can make that key again. A person watched a
    ///   message arrive and it is not there.
    /// - **Message saved, ratchet not.** On restart the ratchet is behind what
    ///   the conversation actually is. The peer's next message still opens —
    ///   the skip logic sees to that — so nothing reports an error, and the
    ///   thread quietly drifts.
    /// - **Either saved, inbox not.** The gap the screen was showing is either
    ///   forgotten or re-opened, so a message that arrived reads as missing or
    ///   a missing one reads as arrived.
    ///
    /// None of the three announces itself. That is what makes one transaction
    /// the requirement rather than the tidy option.
    ///
    /// Returns whether the message was new, so a caller can tell a delivery
    /// from a duplicate that cost nothing.
    pub fn commit_received(
        &self,
        ratchet: Option<&[u8]>,
        inbox: &InboxPosition,
        message: &NewMessage,
        now: i64,
    ) -> Result<InsertOutcome, StoreError> {
        // Which thread this is comes from the message and nowhere else.
        // Review asked for a check that a separately-passed `thread_id`
        // matched; taking the parameter away is the same guarantee without a
        // check that can be forgotten, and there was never a caller that wanted
        // to write one thread's state alongside another thread's message.
        let thread_id = message.thread_id;
        let tx = self.conn.unchecked_transaction()?;
        // State first, message last, and the order is the test's: written the
        // other way round, a message that fails its insert leaves nothing to
        // roll back, and `a_commit_that_fails_leaves_the_ratchet_where_it_was`
        // passes whether or not there is a transaction at all. That is the
        // shape of test this project keeps catching — one that asserts a
        // property the code happens not to be exercising.
        // `None` for a thread that has no ratchet, which is every unpaired
        // one: R0-F5's stranger chat rides the session's own encryption and the
        // Double Ratchet belongs to a paired thread (tech spec §5). Modelling
        // that as an absence rather than an empty blob keeps "no ratchet here"
        // distinct from "a ratchet that serialises to nothing", and stops a
        // caller with nothing to save from having to invent something.
        //
        // The transaction is unchanged and still the point: where there *is* a
        // ratchet, it advances with the message that advanced it or not at all.
        if let Some(ratchet) = ratchet {
            self.write_ratchet(thread_id, ratchet, now)?;
        }
        self.write_inbox(thread_id, inbox)?;
        let outcome = self.add_message(message)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// Save a thread's ratchet on its own — the pairing case, where there is a
    /// ratchet before there is any message to go with it.
    ///
    /// Everything after that first save goes through [`Store::commit_received`]
    /// instead. A bare save mid-conversation would be a ratchet moving without
    /// the message that moved it, which is the second failure listed there.
    pub fn start_ratchet(
        &self,
        thread_id: i64,
        ratchet: &[u8],
        now: i64,
    ) -> Result<(), StoreError> {
        self.write_ratchet(thread_id, ratchet, now)
    }

    fn write_ratchet(&self, thread_id: i64, state: &[u8], now: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO ratchets (thread_id, state, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id) DO UPDATE SET state = ?2, updated_at = ?3",
            params![thread_id, state, now],
        )?;
        Ok(())
    }

    fn write_inbox(&self, thread_id: i64, inbox: &InboxPosition) -> Result<(), StoreError> {
        let through = i64::try_from(inbox.through).map_err(|_| {
            StoreError::Db(format!(
                "inbox position {} does not fit the column",
                inbox.through
            ))
        })?;
        // The same bound the read applies, on the way in as well.
        //
        // Asymmetric validation is worse than none: a write that accepts what
        // the read refuses stores state that cannot be loaded again, so the
        // thread opens once and never afterwards, and the damage is already on
        // disk by the time anything notices. Refused here, the commit rolls
        // back and the conversation carries on from where it was.
        if inbox.ahead.len() > MAX_AHEAD as usize {
            return Err(StoreError::Db(format!(
                "inbox holds {} out-of-order seqs, over the {MAX_AHEAD} bound",
                inbox.ahead.len()
            )));
        }
        let mut ahead = Vec::with_capacity(inbox.ahead.len() * 8);
        for seq in &inbox.ahead {
            ahead.extend_from_slice(&seq.to_be_bytes());
        }
        self.conn.execute(
            "INSERT INTO inboxes (thread_id, through, ahead) VALUES (?1, ?2, ?3)
             ON CONFLICT(thread_id) DO UPDATE SET through = ?2, ahead = ?3",
            params![thread_id, through, ahead],
        )?;
        Ok(())
    }

    /// A thread's stored ratchet state, or `None` if it has never had one.
    ///
    /// `Zeroizing`, as `FileStore::get` is and for the same reason: this is the
    /// root key, both chains, and every key still owed. It came out of the
    /// database as a plain `Vec` and that copy is unavoidable, but the one the
    /// caller holds wipes when it goes.
    pub fn ratchet_state(&self, thread_id: i64) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM ratchets WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(Zeroizing::new))
    }

    /// A thread's stored inbox position, or `None` if nothing has arrived yet.
    pub fn inbox_position(&self, thread_id: i64) -> Result<Option<InboxPosition>, StoreError> {
        let row: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT through, ahead FROM inboxes WHERE thread_id = ?1",
                params![thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((through, packed)) = row else {
            return Ok(None);
        };
        if packed.len() % 8 != 0 {
            return Err(StoreError::Db(format!(
                "inbox for thread {thread_id} has {} bytes of ahead, not a whole number of seqs",
                packed.len()
            )));
        }
        // The session layer's bound, re-applied here rather than restated: a
        // second constant that had to agree with the first is the kind of pair
        // that drifts. Bytes on disk have been through builds where it may have
        // been larger, and a blob taken at face value is an allocation nobody
        // had to attack us to cause.
        if packed.len() / 8 > MAX_AHEAD as usize {
            return Err(StoreError::Db(format!(
                "inbox for thread {thread_id} holds {} out-of-order seqs, over the {MAX_AHEAD} bound",
                packed.len() / 8
            )));
        }
        let through = u64::try_from(through)
            .map_err(|_| StoreError::Db(format!("inbox position {through} is negative")))?;
        let ahead = packed
            .chunks_exact(8)
            .map(|c| u64::from_be_bytes(c.try_into().expect("8 bytes")))
            .collect();
        Ok(Some(InboxPosition { through, ahead }))
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
    let pseudonym: Vec<u8> = r.get(1)?;
    let l2: Vec<u8> = r.get(2)?;
    let name = r.get(3)?;
    let colour: i64 = r.get(4)?;
    let persona_version: i64 = r.get(5)?;
    let first_seen = r.get(6)?;
    Ok((|| {
        Ok(Contact {
            id,
            pseudonym: blob32(pseudonym)?,
            l2_pub: blob32(l2)?,
            name,
            colour: u32_from_i64(colour)?,
            persona_version: u32_from_i64(persona_version)?,
            first_seen,
        })
    })())
}

fn map_pairing(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Pairing, StoreError>> {
    let contact_id = r.get(0)?;
    let l1: Vec<u8> = r.get(1)?;
    let paired_at = r.get(2)?;
    Ok((|| {
        Ok(Pairing {
            contact_id,
            l1_pub: blob32(l1)?,
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
            pseudonym: [1u8; 32],
            l2_pub: [2u8; 32],
            name: "Alice".into(),
            colour: 0x00ff_8800,
            persona_version: 1,
            first_seen: 1000,
        }
    }

    // ── pairings ────────────────────────────────────────────────────────────

    /// The R0-F4 sentence, as a test: a completed ceremony creates a persistent
    /// thread.
    #[test]
    fn pairing_records_the_layer_1_key_and_opens_a_thread() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        assert!(s.pairing_for_contact(id).unwrap().is_none());
        assert!(s.thread_for_contact(id).unwrap().is_none());

        let thread = s.pair_contact(id, &[9u8; 32], 2000).unwrap();
        assert_eq!(s.thread_for_contact(id).unwrap(), Some(thread));
        assert_eq!(
            s.pairing_for_contact(id).unwrap().unwrap(),
            Pairing {
                contact_id: id,
                l1_pub: [9u8; 32],
                paired_at: 2000,
            }
        );
        assert_eq!(s.contact_by_l1(&[9u8; 32]).unwrap().unwrap().id, id);
    }

    /// A pairing must not invent a second thread for someone we were already
    /// talking to — the conversation predates the ceremony in the common case,
    /// because R0-F3 and R0-F5 both work unpaired.
    #[test]
    fn pairing_someone_we_already_chat_with_keeps_the_conversation() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let existing = s.create_thread(id, 1000).unwrap();
        s.add_message(&NewMessage {
            thread_id: existing,
            seq: 1,
            msg_id: vec![1u8; 16],
            body: b"before we paired".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 1000,
        })
        .unwrap();

        assert_eq!(s.pair_contact(id, &[9u8; 32], 2000).unwrap(), existing);
        assert_eq!(s.messages_for_thread(existing).unwrap().len(), 1);
    }

    /// Pairing again supersedes: the other side may have wiped (R0-F9) and come
    /// back with a new Layer-1 identity, and the second ceremony is the current
    /// truth. The old key must not still resolve to them.
    #[test]
    fn pairing_again_replaces_the_key_and_keeps_the_thread() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let thread = s.pair_contact(id, &[9u8; 32], 2000).unwrap();
        assert_eq!(s.pair_contact(id, &[8u8; 32], 3000).unwrap(), thread);

        assert_eq!(
            s.pairing_for_contact(id).unwrap().unwrap().l1_pub,
            [8u8; 32]
        );
        assert!(s.contact_by_l1(&[9u8; 32]).unwrap().is_none());
        assert_eq!(s.contact_by_l1(&[8u8; 32]).unwrap().unwrap().id, id);
    }

    /// The reason pairings are a separate table.
    ///
    /// A pseudonym is a value the *peer* chooses — it is their session static —
    /// while a Layer-1 key comes out of a ceremony. While both lived in one
    /// UNIQUE column, telling those two apart rested on nobody being able to
    /// present a pseudonym equal to somebody else's Layer-1 key. Now identical
    /// bytes in the two roles are simply two different facts.
    #[test]
    fn a_pseudonym_and_a_layer_1_key_with_the_same_bytes_do_not_collide() {
        let (s, _d) = store();
        let stranger = s.add_contact(&a_contact()).unwrap(); // pseudonym [1; 32]
        let other = s
            .add_contact(&NewContact {
                pseudonym: [2u8; 32],
                ..a_contact()
            })
            .unwrap();
        // Someone paired whose Layer-1 key happens to equal the stranger's
        // pseudonym.
        s.pair_contact(other, &[1u8; 32], 2000).unwrap();

        assert_eq!(
            s.contact_by_pseudonym(&[1u8; 32]).unwrap().unwrap().id,
            stranger
        );
        assert_eq!(s.contact_by_l1(&[1u8; 32]).unwrap().unwrap().id, other);
    }

    /// A pairing that fails partway leaves nothing behind.
    ///
    /// The four writes a ceremony produces are one fact, so they are one
    /// transaction. Without that, a failure after the persona update leaves a
    /// contact wearing a name and a Layer-2 key that came from a ceremony which
    /// did not finish — not dangerous, since every paired lookup joins
    /// `pairings`, but a row asserting something that never happened, which is
    /// what this schema was rewritten to stop.
    ///
    /// Forced through the one constraint that can fail late: a Layer-1 key
    /// another contact already holds.
    #[test]
    fn a_pairing_that_fails_leaves_the_contact_untouched() {
        let (s, _d) = store();
        let taken = s.add_contact(&a_contact()).unwrap();
        s.pair_contact(taken, &[9u8; 32], 1000).unwrap();

        let other = s
            .add_contact(&NewContact {
                pseudonym: [2u8; 32],
                l2_pub: [0u8; 32],
                name: "Unknown".into(),
                ..a_contact()
            })
            .unwrap();

        assert!(s
            .record_pairing(other, &[7u8; 32], "Ada", 0x00_ff_00, 4, &[9u8; 32], 2000)
            .is_err());

        let after = s.contact_by_id(other).unwrap().unwrap();
        assert_eq!(after.name, "Unknown", "the name survived a failed pairing");
        assert_eq!(
            after.l2_pub, [0u8; 32],
            "the Layer-2 key was written anyway"
        );
        assert!(s.pairing_for_contact(other).unwrap().is_none());
        assert!(s.thread_for_contact(other).unwrap().is_none());
    }

    /// The happy path writes all four things.
    #[test]
    fn recording_a_pairing_writes_identity_persona_pairing_and_thread() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let thread = s
            .record_pairing(id, &[7u8; 32], "Ada", 0x00_ff_00, 4, &[9u8; 32], 2000)
            .unwrap();

        let c = s.contact_by_id(id).unwrap().unwrap();
        assert_eq!(c.name, "Ada");
        assert_eq!(c.colour, 0x00_ff_00);
        assert_eq!(c.persona_version, 4);
        assert_eq!(c.l2_pub, [7u8; 32]);
        assert_eq!(s.contact_by_l1(&[9u8; 32]).unwrap().unwrap().id, id);
        assert_eq!(s.thread_for_contact(id).unwrap(), Some(thread));
        assert_eq!(s.paired_contact_by_l2(&[7u8; 32]).unwrap().unwrap().id, id);
    }

    /// One Layer-1 key, one person.
    ///
    /// A Layer-1 key *is* the identity, so two contacts paired to the same one
    /// would be one person written down twice, with `contact_by_l1` returning
    /// whichever row the index reached first. Refusing is not the only defensible
    /// answer — merging the two would be another — but silently allowing it is
    /// not, and the refusal must leave nothing behind.
    ///
    /// Here because mutation testing found the `UNIQUE` on `pairings.l1_pub`
    /// could be deleted with every test still passing.
    #[test]
    fn two_contacts_cannot_claim_the_same_layer_1_identity() {
        let (s, _d) = store();
        let first = s.add_contact(&a_contact()).unwrap();
        let second = s
            .add_contact(&NewContact {
                pseudonym: [2u8; 32],
                ..a_contact()
            })
            .unwrap();
        s.pair_contact(first, &[9u8; 32], 2000).unwrap();

        assert!(s.pair_contact(second, &[9u8; 32], 3000).is_err());
        // Rolled back whole: no pairing, and no thread opened on the way past.
        assert!(s.pairing_for_contact(second).unwrap().is_none());
        assert!(s.thread_for_contact(second).unwrap().is_none());
        assert_eq!(s.contact_by_l1(&[9u8; 32]).unwrap().unwrap().id, first);
    }

    /// Looking someone up by their Layer-2 key must find only people we have
    /// actually paired with.
    ///
    /// The trap this exists to avoid: an unpaired contact carries a
    /// **placeholder** Layer-2 key, because until a ceremony runs nothing
    /// proves one belongs to the person on the other end. A lookup on the
    /// column alone therefore matches every unpaired contact at once for the
    /// placeholder value, and would hand back a stranger as a paired friend.
    #[test]
    fn a_layer_2_lookup_finds_only_paired_contacts() {
        let (s, _d) = store();
        // Two strangers, both carrying the placeholder key.
        let stranger = s
            .add_contact(&NewContact {
                pseudonym: [1u8; 32],
                l2_pub: [0u8; 32],
                ..a_contact()
            })
            .unwrap();
        s.add_contact(&NewContact {
            pseudonym: [2u8; 32],
            l2_pub: [0u8; 32],
            ..a_contact()
        })
        .unwrap();
        // And a friend, whose key a ceremony actually established.
        let friend = s
            .add_contact(&NewContact {
                pseudonym: [3u8; 32],
                l2_pub: [5u8; 32],
                ..a_contact()
            })
            .unwrap();
        s.pair_contact(friend, &[9u8; 32], 2000).unwrap();

        assert!(
            s.paired_contact_by_l2(&[0u8; 32]).unwrap().is_none(),
            "the placeholder key matched a stranger"
        );
        assert_eq!(
            s.paired_contact_by_l2(&[5u8; 32]).unwrap().unwrap().id,
            friend
        );
        let _ = stranger;

        // And a revoked pairing stops answering (R0-F10).
        s.unpair_contact(friend).unwrap();
        assert!(s.paired_contact_by_l2(&[5u8; 32]).unwrap().is_none());
    }

    /// Blocking a paired person revokes the pairing (R0-F10) — and does not
    /// burn the conversation, which is theirs.
    #[test]
    fn unpairing_keeps_the_thread_and_its_messages() {
        let (s, _d) = store();
        let id = s.add_contact(&a_contact()).unwrap();
        let thread = s.pair_contact(id, &[9u8; 32], 2000).unwrap();
        s.add_message(&NewMessage {
            thread_id: thread,
            seq: 1,
            msg_id: vec![3u8; 16],
            body: b"said while paired".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 2100,
        })
        .unwrap();

        assert!(s.unpair_contact(id).unwrap());
        assert!(s.pairing_for_contact(id).unwrap().is_none());
        assert!(s.contact_by_l1(&[9u8; 32]).unwrap().is_none());
        assert_eq!(s.thread_for_contact(id).unwrap(), Some(thread));
        assert_eq!(s.messages_for_thread(thread).unwrap().len(), 1);
        // Reports rather than pretends, like every other revoking call here.
        assert!(!s.unpair_contact(id).unwrap());
    }

    /// A pairing for a contact that does not exist is a caller bug.
    ///
    /// The foreign key would refuse it anyway, so the explicit check buys
    /// exactly one thing: an error that names the contact instead of
    /// `FOREIGN KEY constraint failed` arriving three layers up with nothing to
    /// say. That being the whole point, the message is what this asserts —
    /// mutation testing was blunt about it, since deleting the check left the
    /// original test passing on the constraint's back.
    #[test]
    fn pairing_an_unknown_contact_says_which_contact() {
        let (s, _d) = store();
        let err = s
            .pair_contact(4242, &[9u8; 32], 2000)
            .unwrap_err()
            .to_string();
        assert!(err.contains("4242"), "unhelpful error: {err}");
        assert!(s.contact_by_l1(&[9u8; 32]).unwrap().is_none());
    }

    /// Forgetting someone forgets the pairing too — R0-F9 leaves nothing that
    /// still asserts a ceremony happened.
    #[test]
    fn deleting_a_contact_takes_its_pairing_with_it() {
        let (s, _d) = store();
        let keep = s.add_contact(&a_contact()).unwrap();
        let go = s
            .add_contact(&NewContact {
                pseudonym: [3u8; 32],
                ..a_contact()
            })
            .unwrap();
        s.pair_contact(go, &[9u8; 32], 2000).unwrap();

        s.merge_contact(go, keep).unwrap();
        assert!(s.contact_by_l1(&[9u8; 32]).unwrap().is_none());
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
            s.contact_by_pseudonym(&[42u8; 32]).unwrap().unwrap().id,
            id,
            "the contact did not move onto the real key"
        );
        assert!(
            s.contact_by_pseudonym(&[1u8; 32]).unwrap().is_none(),
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

    /// Merging keeps both halves of the conversation, in order.
    ///
    /// The case: we knew someone, their device id rotated, a message was queued
    /// to the new id before the next session proved who they were, and now one
    /// person holds two rows. Re-keying cannot fix it — the real key is taken —
    /// so the stray has to be folded in.
    #[test]
    fn merging_a_contact_keeps_both_halves_in_order() {
        let (s, _d) = store();
        let known = s.add_contact(&a_contact()).unwrap();
        let known_thread = s.create_thread(known, 1000).unwrap();
        let stray = s
            .add_contact(&NewContact {
                pseudonym: [8u8; 32],
                ..a_contact()
            })
            .unwrap();
        let stray_thread = s.create_thread(stray, 2000).unwrap();

        for (thread, text) in [(known_thread, "older"), (stray_thread, "newer")] {
            let seq = s.next_seq(thread, Direction::Outgoing).unwrap();
            s.add_message(&NewMessage {
                thread_id: thread,
                seq,
                msg_id: text.as_bytes().to_vec(),
                body: text.as_bytes().to_vec(),
                direction: Direction::Outgoing,
                state: MessageState::Sent,
                created_at: 1000,
            })
            .unwrap();
        }
        // Both start at seq 1, which is exactly what would interleave.
        s.merge_contact(stray, known).unwrap();

        assert!(
            s.contact_by_id(stray).unwrap().is_none(),
            "the stray row outlived the merge, so the person is still two people"
        );
        let texts: Vec<String> = s
            .messages_for_thread(known_thread)
            .unwrap()
            .into_iter()
            .map(|m| String::from_utf8(m.body).unwrap())
            .collect();
        assert_eq!(
            texts,
            vec!["older".to_string(), "newer".to_string()],
            "the newer half did not follow the older; ordering is by (seq, id), \
             so reused seq numbers shuffle one conversation together"
        );
    }

    /// Transfers follow the conversation, and do not come adrift of it.
    ///
    /// `transfers.thread_id` is `ON DELETE SET NULL`, so a merge that forgot to
    /// move them would not fail — it would silently null them out when the old
    /// thread went, and a Drop would survive with no conversation to belong to.
    /// A regression here is invisible without this assertion.
    #[test]
    fn merging_moves_the_transfers_too() {
        let (s, _d) = store();
        let known = s.add_contact(&a_contact()).unwrap();
        let known_thread = s.create_thread(known, 1000).unwrap();
        let stray = s
            .add_contact(&NewContact {
                pseudonym: [8u8; 32],
                ..a_contact()
            })
            .unwrap();
        let stray_thread = s.create_thread(stray, 2000).unwrap();
        let transfer = s
            .add_transfer(&NewTransfer {
                thread_id: Some(stray_thread),
                direction: Direction::Incoming,
                name: "photo.jpg".into(),
                size: 10,
                mime: "image/jpeg".into(),
                state: TransferState::Offered,
                root_hash: [3u8; 32],
                chunk_bitmap: vec![0u8],
                created_at: 2000,
            })
            .unwrap();

        s.merge_contact(stray, known).unwrap();

        assert_eq!(
            s.transfer_by_id(transfer).unwrap().unwrap().thread_id,
            Some(known_thread),
            "the transfer was left on the old thread or nulled out with it"
        );
    }

    /// A stray with no thread of its own still has to stop existing.
    #[test]
    fn merging_moves_a_lone_thread_rather_than_dropping_it() {
        let (s, _d) = store();
        let known = s.add_contact(&a_contact()).unwrap();
        let stray = s
            .add_contact(&NewContact {
                pseudonym: [8u8; 32],
                ..a_contact()
            })
            .unwrap();
        let stray_thread = s.create_thread(stray, 2000).unwrap();

        // The destination has no thread, so the cascade would take this one
        // with the deleted contact if it were not moved across first.
        s.merge_contact(stray, known).unwrap();

        assert_eq!(s.thread_for_contact(known).unwrap(), Some(stray_thread));
        assert!(s.contact_by_id(stray).unwrap().is_none());
    }

    /// Merging into a contact that is not there is refused, not half-done.
    ///
    /// This was the review's catch and it was a data-loss bug: with no thread
    /// on the source, nothing in the merge touched a foreign key, so a bad
    /// destination sailed through to the DELETE and destroyed the source row.
    ///
    /// **It does not exercise the rollback.** The check now fires before the
    /// transaction opens, so `?` alone gives the same result and this would
    /// pass without it. Atomicity of the multi-statement path is argued, not
    /// proven — nothing in this API can be made to fail part-way through on
    /// demand — and that is said here so a green tick is not read as a
    /// guarantee it does not give.
    #[test]
    fn merging_into_a_contact_that_is_not_there_is_refused() {
        let (s, _d) = store();
        let stray = s.add_contact(&a_contact()).unwrap();
        let thread = s.create_thread(stray, 1000).unwrap();

        assert!(
            s.merge_contact(stray, 9999).is_err(),
            "merging into a contact that does not exist has to fail, or this \
             test proves nothing at all"
        );

        assert!(
            s.contact_by_id(stray).unwrap().is_some(),
            "the contact was deleted by a merge that did not complete"
        );
        assert_eq!(
            s.thread_for_contact(stray).unwrap(),
            Some(thread),
            "the thread was left detached from the only contact that owns it"
        );
    }

    /// A merge whose source is not there is an error, not a quiet success.
    ///
    /// Nothing in the body would have complained: the match does nothing, the
    /// DELETE hits no rows, and a caller holding the wrong id would be told the
    /// fold happened. Every other contact API here reports whether it matched.
    #[test]
    fn merging_from_a_contact_that_is_not_there_is_refused() {
        let (s, _d) = store();
        let into = s.add_contact(&a_contact()).unwrap();
        assert!(s.merge_contact(9999, into).is_err());
        assert!(
            s.contact_by_id(into).unwrap().is_some(),
            "the destination was touched by a merge that could not happen"
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
        let got = s.contact_by_pseudonym(&[1u8; 32]).unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.name, "Alice");
        assert_eq!(got.l2_pub, [2u8; 32]);
        assert_eq!(s.list_contacts().unwrap().len(), 1);
        assert!(s.contact_by_pseudonym(&[9u8; 32]).unwrap().is_none());
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

    // ── thread receive state (T12) ──────────────────────────────────────────

    /// A paired thread with nothing received yet.
    fn a_thread(s: &Store) -> i64 {
        let id = s.add_contact(&a_contact()).unwrap();
        s.pair_contact(id, &[9u8; 32], 2000).unwrap()
    }

    fn an_incoming(thread_id: i64, seq: i64, msg_id: u8) -> NewMessage {
        NewMessage {
            thread_id,
            seq,
            msg_id: vec![msg_id; 16],
            body: b"hello".to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: 3000,
        }
    }

    #[test]
    fn a_thread_starts_with_no_ratchet_and_no_inbox() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        assert_eq!(s.ratchet_state(thread).unwrap(), None);
        assert_eq!(s.inbox_position(thread).unwrap(), None);
    }

    #[test]
    fn a_ratchet_and_an_inbox_round_trip() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let inbox = InboxPosition {
            through: 4,
            ahead: vec![7, 9],
        };
        s.commit_received(Some(b"state one"), &inbox, &an_incoming(thread, 1, 1), 3000)
            .unwrap();

        assert_eq!(
            s.ratchet_state(thread).unwrap().unwrap()[..],
            b"state one"[..]
        );
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), inbox);
    }

    /// A ratchet is a moving thing: the second write must replace the first,
    /// not sit beside it. A second row would be a second conversation.
    #[test]
    fn a_second_commit_replaces_the_state_rather_than_adding_one() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let first = InboxPosition {
            through: 1,
            ahead: vec![],
        };
        let second = InboxPosition {
            through: 2,
            ahead: vec![5],
        };
        s.commit_received(Some(b"state one"), &first, &an_incoming(thread, 1, 1), 3000)
            .unwrap();
        s.commit_received(
            Some(b"state two"),
            &second,
            &an_incoming(thread, 2, 2),
            3001,
        )
        .unwrap();

        assert_eq!(
            s.ratchet_state(thread).unwrap().unwrap()[..],
            b"state two"[..]
        );
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), second);
        assert_eq!(s.messages_for_thread(thread).unwrap().len(), 2);
    }

    /// The kill-the-app-mid-delivery case, as far as a test can stand in for
    /// it: if any part of the commit fails, the ratchet must not have moved.
    ///
    /// Provoked with an inbox position that does not fit its column, because
    /// that fails in `write_inbox` — *after* the ratchet has been written and
    /// before the commit, which is the only arrangement where the rollback is
    /// doing anything at all.
    ///
    /// The first version of this used a message against a missing thread. That
    /// looked like the same test and was not: with the thread taken from the
    /// message, the failure happens on the first statement, nothing has been
    /// written, and the assertion holds whether or not there is a transaction.
    #[test]
    fn a_commit_that_fails_leaves_the_ratchet_where_it_was() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let before = InboxPosition {
            through: 1,
            ahead: vec![],
        };
        s.commit_received(
            Some(b"state one"),
            &before,
            &an_incoming(thread, 1, 1),
            3000,
        )
        .unwrap();

        let impossible = InboxPosition {
            through: u64::MAX,
            ahead: vec![],
        };
        assert!(
            s.commit_received(
                Some(b"state two"),
                &impossible,
                &an_incoming(thread, 2, 2),
                3001
            )
            .is_err(),
            "an inbox position that cannot be stored was accepted"
        );

        assert_eq!(
            s.ratchet_state(thread).unwrap().unwrap()[..],
            b"state one"[..],
            "the ratchet moved for a message that was never stored — its key is \
             spent and the message is gone"
        );
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), before);
        assert_eq!(
            s.messages_for_thread(thread).unwrap().len(),
            1,
            "a message was stored by a commit that failed"
        );
    }

    /// And a message for a thread that does not exist is refused rather than
    /// written against a dangling id.
    #[test]
    fn a_commit_for_a_thread_that_does_not_exist_is_refused() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let nowhere = InboxPosition::default();
        assert!(s
            .commit_received(
                Some(b"state"),
                &nowhere,
                &an_incoming(thread + 999, 1, 1),
                3000
            )
            .is_err());
    }

    /// A duplicate is not a failure: the message is silently not inserted, but
    /// the ratchet genuinely did move to open it, so that has to be saved.
    ///
    /// Getting this backwards would roll back a real advance and leave the
    /// ratchet behind the conversation.
    #[test]
    fn a_duplicate_message_still_saves_the_ratchet_that_opened_it() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let first = InboxPosition {
            through: 1,
            ahead: vec![],
        };
        s.commit_received(Some(b"state one"), &first, &an_incoming(thread, 1, 1), 3000)
            .unwrap();

        let moved = InboxPosition {
            through: 1,
            ahead: vec![3],
        };
        assert_eq!(
            s.commit_received(Some(b"state two"), &moved, &an_incoming(thread, 1, 1), 3001)
                .unwrap(),
            InsertOutcome::Duplicate,
        );
        assert_eq!(
            s.ratchet_state(thread).unwrap().unwrap()[..],
            b"state two"[..]
        );
        assert_eq!(s.messages_for_thread(thread).unwrap().len(), 1);
    }

    /// Under R0-F9 a deleted thread must not leave key material behind: the
    /// root and every kept message key live in that blob.
    #[test]
    fn deleting_a_contact_takes_the_ratchet_with_it() {
        let (s, _d) = store();
        let contact = s.add_contact(&a_contact()).unwrap();
        let thread = s.pair_contact(contact, &[9u8; 32], 2000).unwrap();
        s.start_ratchet(thread, b"state one", 3000).unwrap();

        // Straight to SQL, as the messages cascade test next door does: there
        // is no public delete, and the property under test is the schema's, not
        // an API's.
        s.conn
            .execute("DELETE FROM contacts WHERE id = ?1", params![contact])
            .unwrap();
        assert_eq!(
            s.ratchet_state(thread).unwrap(),
            None,
            "a deleted conversation left its keys in the database"
        );
        assert_eq!(s.inbox_position(thread).unwrap(), None);
    }

    /// Corruption in the packed `ahead` column is reported rather than read as
    /// whatever happens to be there.
    #[test]
    fn an_inbox_that_is_not_a_whole_number_of_seqs_is_refused() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        s.conn
            .execute(
                "INSERT INTO inboxes (thread_id, through, ahead) VALUES (?1, 1, ?2)",
                params![thread, vec![0u8; 12]],
            )
            .unwrap();
        assert!(s.inbox_position(thread).is_err());
    }

    /// Same lesson as the ratchet's skipped-key count and `Inbox::resumed`: a
    /// length read off disk is an allocation nobody had to attack us to cause.
    #[test]
    fn an_inbox_holding_more_than_the_bound_is_refused() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let packed = |count: usize| {
            (0..count)
                .flat_map(|i| (i as u64).to_be_bytes())
                .collect::<Vec<u8>>()
        };

        s.conn
            .execute(
                "INSERT INTO inboxes (thread_id, through, ahead) VALUES (?1, 0, ?2)",
                params![thread, packed(MAX_AHEAD as usize + 1)],
            )
            .unwrap();
        assert!(
            s.inbox_position(thread).is_err(),
            "an inbox over the bound was read back at face value"
        );

        // And exactly the bound is fine, so what is refused is the bound and
        // not the shape of the thing.
        s.conn
            .execute(
                "UPDATE inboxes SET ahead = ?2 WHERE thread_id = ?1",
                params![thread, packed(MAX_AHEAD as usize)],
            )
            .unwrap();
        assert_eq!(
            s.inbox_position(thread).unwrap().unwrap().ahead.len(),
            MAX_AHEAD as usize
        );
    }

    /// A write that accepts what the read refuses is state that can be stored
    /// and never loaded — the thread opens once and never again.
    #[test]
    fn an_inbox_over_the_bound_is_refused_on_the_way_in_too() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let good = InboxPosition {
            through: 1,
            ahead: vec![],
        };
        s.commit_received(Some(b"state one"), &good, &an_incoming(thread, 1, 1), 3000)
            .unwrap();

        let too_many = InboxPosition {
            through: 1,
            ahead: (2..=MAX_AHEAD + 2).collect(),
        };
        assert!(
            s.commit_received(
                Some(b"state two"),
                &too_many,
                &an_incoming(thread, 2, 2),
                3001
            )
            .is_err(),
            "an inbox the read side would refuse was written anyway"
        );
        // And the refusal rolls the whole commit back, rather than leaving the
        // ratchet ahead of a thread whose inbox never moved.
        assert_eq!(
            s.ratchet_state(thread).unwrap().unwrap()[..],
            b"state one"[..]
        );
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), good);

        // Exactly the bound goes through, so what is refused is the bound and
        // not the shape of the thing — and so an off-by-one here shows up as a
        // conversation that cannot hold the gaps it is allowed to.
        let at_the_bound = InboxPosition {
            through: 1,
            ahead: (2..MAX_AHEAD + 2).collect(),
        };
        s.commit_received(
            Some(b"state two"),
            &at_the_bound,
            &an_incoming(thread, 2, 2),
            3002,
        )
        .unwrap();
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), at_the_bound);
    }

    /// An unpaired thread has no ratchet — R0-F5's stranger chat rides the
    /// session's own encryption — so `None` has to mean nothing was written,
    /// not an empty blob written. The difference matters later: a row that
    /// exists says "this thread has ratchet state", and one that decodes to
    /// nothing would be indistinguishable from a corrupted save.
    #[test]
    fn committing_without_a_ratchet_leaves_no_ratchet_row() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let inbox = InboxPosition {
            through: 1,
            ahead: vec![],
        };

        s.commit_received(None, &inbox, &an_incoming(thread, 1, 1), 3000)
            .unwrap();

        assert!(
            s.ratchet_state(thread).unwrap().is_none(),
            "a thread with no ratchet got one anyway"
        );
        // And the rest of the commit still happened.
        assert_eq!(s.inbox_position(thread).unwrap().unwrap(), inbox);
        assert_eq!(s.messages_for_thread(thread).unwrap().len(), 1);
    }

    /// And a later commit that *does* carry one still saves it, so a thread
    /// that gains a ratchet on pairing is not stuck without one.
    #[test]
    fn a_thread_can_gain_a_ratchet_after_committing_without_one() {
        let (s, _d) = store();
        let thread = a_thread(&s);
        let inbox = InboxPosition {
            through: 1,
            ahead: vec![],
        };
        s.commit_received(None, &inbox, &an_incoming(thread, 1, 1), 3000)
            .unwrap();

        let later = InboxPosition {
            through: 2,
            ahead: vec![],
        };
        s.commit_received(
            Some(b"now paired"),
            &later,
            &an_incoming(thread, 2, 2),
            3001,
        )
        .unwrap();
        assert_eq!(
            s.ratchet_state(thread).unwrap().map(|v| v.to_vec()),
            Some(b"now paired".to_vec())
        );
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
