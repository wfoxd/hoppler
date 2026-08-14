//! The chat envelope and the receiver pipeline (T12; tech spec §5, §8, R0-F5).
//!
//! What a message *is* on the wire, and what a receiver does with one before it
//! reaches the store. The ratchet next door makes a message unreadable to
//! anyone else; this decides whether it is new, where it goes, and what is
//! still missing.
//!
//! # The two numbers, which are not the same number
//!
//! A message has a ratchet message number and an envelope [`seq`], and
//! conflating them would be a quiet disaster.
//!
//! The ratchet's number counts *ciphertexts on a chain*: it resets to zero
//! every time either side replies, and its replay refusal is what stops the
//! same bytes opening twice. The envelope's `seq` counts *messages in a
//! conversation*: it never resets, survives restarts, and is durable.
//!
//! They come apart exactly where it matters. A message the sender never got an
//! ack for is resent from the outbox as a *fresh* ratchet message — new number,
//! new key, and the ratchet is right to accept it, because cryptographically it
//! has never seen it. Only `seq` can say that the person already has it. So the
//! ratchet's dedup and this one are not two implementations of the same check;
//! neither one covers the other's case.
//!
//! [`seq`]: ChatEnvelope::seq
//!
//! # Where dedup happens, and why not all in one place
//!
//! The task names `msg_id` as the dedup key, and it is — in the store, where
//! `messages.msg_id` is `UNIQUE` and an insert comes back
//! [`Duplicate`](crate::store::InsertOutcome::Duplicate). What [`Inbox`] does
//! here is dedup on `seq`, which it has to track anyway to order messages and
//! find gaps.
//!
//! Deliberately not the same key twice. `seq` is what the *sender* promises is
//! unique within its stream, and checking it here is free — the ordering state
//! already knows. `msg_id` is what the *store* needs to be unique across every
//! thread on the device. Doing `msg_id` in both places would be one check
//! written twice, which this project has learned means one check maintained
//! once and the copy quietly rotting.

use std::collections::BTreeSet;

use prost::Message as _;

use crate::crypto::rng;
use crate::proto::v0::ChatMessage;

/// Length of a [`MsgId`].
pub const MSG_ID_LEN: usize = 16;

/// The store's global dedup key for a message.
///
/// Sixteen random bytes, not a counter and not a hash of the body: two people
/// can legitimately send the same word twice, and 128 bits of randomness makes
/// an accidental collision between any two messages on a device impossible to
/// reach in a lifetime of chatting.
pub type MsgId = [u8; MSG_ID_LEN];

/// Longest body a message may carry.
///
/// Sized for what a person types, not for what the frame could hold —
/// [`MAX_FRAME_PAYLOAD`](super::frame::MAX_FRAME_PAYLOAD) is twice this and
/// still has to fit the ratchet header and the AEAD tag around it. R0-F5 is
/// text; a pasted note of eight thousand characters is already generous, and
/// anything genuinely large is a Drop (R0-F6), which never travels as a chat
/// line.
pub const MAX_BODY: usize = 8 * 1024;

/// How far past the contiguous run a `seq` may be before an [`Inbox`] stops
/// tracking it.
///
/// A receiver holds one entry per message that has arrived out of order, so
/// without a bound a peer sending `seq` four billion could make us remember
/// four billion gaps. The number is generous against the case it exists for —
/// R0-F5's reunion, where a run of messages queued during an absence arrives at
/// once and possibly reordered.
pub const MAX_AHEAD: u64 = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The bytes are not a `ChatMessage`.
    Malformed,
    /// `seq` was zero, which numbering never uses — see [`Inbox::through`].
    NoSeq,
    /// `msg_id` was not [`MSG_ID_LEN`] bytes.
    BadMsgId,
    /// The body is longer than [`MAX_BODY`].
    TooLong,
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvelopeError::Malformed => write!(f, "not a chat message"),
            EnvelopeError::NoSeq => write!(f, "message has no sequence number"),
            EnvelopeError::BadMsgId => write!(f, "message id is not {MSG_ID_LEN} bytes"),
            EnvelopeError::TooLong => write!(f, "message body exceeds {MAX_BODY} bytes"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

/// One chat message, checked.
///
/// The point of having a type distinct from the generated `ChatMessage` is that
/// a value of this type has already been through [`ChatEnvelope::decode`]: the
/// `seq` is usable, the id is the right length, and the body is within bounds.
/// Code downstream cannot be handed a half-checked one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEnvelope {
    /// Position in the sender's stream for this thread, from 1.
    pub seq: u64,
    pub msg_id: MsgId,
    pub body: Vec<u8>,
}

impl ChatEnvelope {
    /// A new outgoing message. The id is drawn here and nowhere else, so there
    /// is exactly one place that decides what makes two messages the same one.
    pub fn new(seq: u64, body: Vec<u8>) -> Result<Self, EnvelopeError> {
        if seq == 0 {
            return Err(EnvelopeError::NoSeq);
        }
        if body.len() > MAX_BODY {
            return Err(EnvelopeError::TooLong);
        }
        Ok(Self {
            seq,
            msg_id: rng::random_array::<MSG_ID_LEN>(),
            body,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        ChatMessage {
            seq: self.seq,
            msg_id: self.msg_id.to_vec(),
            body: self.body.clone(),
        }
        .encode_to_vec()
    }

    /// Parse one, refusing anything a sender should not have produced.
    ///
    /// Checked even though these bytes came out of the ratchet and are
    /// therefore genuinely from the paired peer. Authenticated is not the same
    /// as correct: the peer may be a future version, or an older one with a
    /// bug, and every field here reaches either the store or the screen.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        let message = ChatMessage::decode(bytes).map_err(|_| EnvelopeError::Malformed)?;
        if message.seq == 0 {
            return Err(EnvelopeError::NoSeq);
        }
        let msg_id: MsgId = message
            .msg_id
            .try_into()
            .map_err(|_| EnvelopeError::BadMsgId)?;
        if message.body.len() > MAX_BODY {
            return Err(EnvelopeError::TooLong);
        }
        Ok(Self {
            seq: message.seq,
            msg_id,
            body: message.body,
        })
    }
}

/// What receiving a `seq` came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// New. Store it and show it.
    Accepted,
    /// Already had it — at-least-once delivery, or a sender resending after a
    /// severance it never saw acked. Dropping it silently is the correct
    /// outcome and the whole reason `seq` is durable.
    Duplicate,
    /// Further ahead than [`MAX_AHEAD`], so tracking the gap would cost more
    /// than the messages behind it are worth.
    ///
    /// Refused rather than accepted-with-a-huge-gap, and refused rather than
    /// accepted-and-forget-the-gap: the first is unbounded memory on a peer's
    /// say-so, and the second is silent loss, which R0-F5 exists to prevent.
    TooFarAhead,
}

/// One direction of one thread's arriving messages: what has come, what is
/// still missing, and what has been seen before.
///
/// One per thread per *sender*, because `seq` is per-sender — our own outgoing
/// numbering is the outbox's business and never meets this.
///
/// # The hazard this cannot see
///
/// All of it rests on the sender's `seq` being durable. A peer that lost its
/// counter and restarted at 1 would have every message refused as a duplicate,
/// and from this side that is indistinguishable from a peer sending nothing.
/// The defence is on the sending side — the counter is stored, not held in
/// memory — which is why it lands with the rest of persistence.
#[derive(Clone, Debug, Default)]
pub struct Inbox {
    through: u64,
    /// Every element is inside the window: `through < seq <= through +
    /// MAX_AHEAD`. Both constructors establish it and [`Inbox::receive`] keeps
    /// it — the run only ever moves up, which moves the window with it.
    ///
    /// [`Inbox::pending`] depends on this and not merely benefits from it: it
    /// walks from the run to the highest entry, so an entry outside the window
    /// is a walk of arbitrary length.
    ahead: BTreeSet<u64>,
}

impl Inbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore one from what was stored. See [`Inbox::through`] and
    /// [`Inbox::ahead`] for the two halves.
    ///
    /// Both bounds re-applied rather than trusted. What comes back has been
    /// through the store and possibly through a different build of this file,
    /// and the window is a constant that could reasonably change between
    /// versions — a `seq` that was legal when it was written and is not now
    /// would otherwise reach [`Inbox::pending`] and make it walk from the run
    /// to wherever that number is.
    ///
    /// Out-of-window entries are dropped rather than refused. The cost is that
    /// such a message is no longer known-delivered and could be shown twice if
    /// it arrives again; the alternative is a thread that cannot be opened at
    /// all, which is worse for the same person.
    pub fn resumed(through: u64, ahead: impl IntoIterator<Item = u64>) -> Self {
        let window = through.saturating_add(MAX_AHEAD);
        Self {
            through,
            // Anything at or below the contiguous run is already covered by it;
            // keeping a copy would leave two answers to "have we had this".
            ahead: ahead
                .into_iter()
                .filter(|seq| *seq > through && *seq <= window)
                .collect(),
        }
    }

    /// Every `seq` up to and including this one has arrived.
    ///
    /// Zero means nothing has, which is why numbering starts at 1 — with a
    /// 0-based scheme this would need a separate "anything yet?" flag, and the
    /// flag and the number would be able to disagree.
    pub fn through(&self) -> u64 {
        self.through
    }

    /// Arrived, but with something before them still missing.
    pub fn ahead(&self) -> Vec<u64> {
        self.ahead.iter().copied().collect()
    }

    /// Take one message's `seq`.
    pub fn receive(&mut self, seq: u64) -> Delivery {
        if seq == 0 || seq <= self.through || self.ahead.contains(&seq) {
            return Delivery::Duplicate;
        }
        if seq > self.through.saturating_add(MAX_AHEAD) {
            return Delivery::TooFarAhead;
        }
        self.ahead.insert(seq);
        // Absorb whatever run this completed. A message that filled the last
        // hole can close a gap of hundreds at once, which is what a reunion
        // looks like when the missing one finally arrives.
        //
        // Checked, because the last seq in the range closes the run *onto* the
        // end of it and the next step would be off the end. Written with `+ 1`
        // first, which panicked in exactly that spot.
        while let Some(next) = self.through.checked_add(1) {
            if !self.ahead.remove(&next) {
                break;
            }
            self.through = next;
        }
        Delivery::Accepted
    }

    /// The `seq` numbers that have not arrived but are known to exist, because
    /// something after them has.
    ///
    /// R0-F5's "message pending": the UI shows these as a hole in the
    /// conversation rather than closing over them, so a message that never
    /// comes is visible as missing instead of being silently absent.
    pub fn pending(&self) -> Vec<u64> {
        let Some(&highest) = self.ahead.iter().next_back() else {
            return Vec::new();
        };
        ((self.through + 1)..highest)
            .filter(|seq| !self.ahead.contains(seq))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64, body: &str) -> ChatEnvelope {
        ChatEnvelope::new(seq, body.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn an_envelope_round_trips() {
        let sent = envelope(7, "are you coming");
        let read = ChatEnvelope::decode(&sent.encode()).unwrap();
        assert_eq!(read, sent);
    }

    /// The wire is a contract with other builds, so it is pinned rather than
    /// merely self-consistent. A round-trip test proves only that this build
    /// agrees with itself, which it would go on doing after a field renumbering
    /// that stopped it talking to every other device.
    #[test]
    fn the_wire_form_is_pinned() {
        let envelope = ChatEnvelope {
            seq: 2,
            msg_id: [0xAB; MSG_ID_LEN],
            body: b"hi".to_vec(),
        };
        let expected = [
            "0802",                                 // field 1, seq, varint 2
            "1210abababababababababababababababab", // field 2, msg_id, 16 bytes
            "1a026869",                             // field 3, body, "hi"
        ]
        .concat();
        assert_eq!(hex::encode(envelope.encode()), expected);
    }

    /// Two messages must not be able to be the same message by accident, and
    /// the id is the only thing standing between the store and a dropped line.
    #[test]
    fn identical_messages_get_different_ids() {
        assert_ne!(
            envelope(1, "ok").msg_id,
            envelope(1, "ok").msg_id,
            "the same text at the same seq produced the same id — the store \
             would take the second one for a duplicate and drop it"
        );
    }

    #[test]
    fn a_message_with_no_seq_is_refused() {
        // Both doors, because `new` is ours and `decode` is a peer's.
        assert_eq!(
            ChatEnvelope::new(0, b"nowhere".to_vec()).err(),
            Some(EnvelopeError::NoSeq)
        );
        let unnumbered = ChatMessage {
            seq: 0,
            msg_id: vec![0u8; MSG_ID_LEN],
            body: b"nowhere".to_vec(),
        }
        .encode_to_vec();
        assert_eq!(
            ChatEnvelope::decode(&unnumbered).err(),
            Some(EnvelopeError::NoSeq),
            "seq 0 is what an Inbox means by 'nothing yet' — a message wearing \
             it would be permanently a duplicate"
        );
    }

    #[test]
    fn a_message_id_of_the_wrong_length_is_refused() {
        for length in [0, MSG_ID_LEN - 1, MSG_ID_LEN + 1] {
            let wrong = ChatMessage {
                seq: 1,
                msg_id: vec![0u8; length],
                body: b"hello".to_vec(),
            }
            .encode_to_vec();
            assert_eq!(
                ChatEnvelope::decode(&wrong).err(),
                Some(EnvelopeError::BadMsgId),
                "a {length}-byte id was accepted"
            );
        }
    }

    #[test]
    fn an_oversized_body_is_refused_at_both_doors() {
        let too_long = vec![b'x'; MAX_BODY + 1];
        assert_eq!(
            ChatEnvelope::new(1, too_long.clone()).err(),
            Some(EnvelopeError::TooLong)
        );
        let wire = ChatMessage {
            seq: 1,
            msg_id: vec![0u8; MSG_ID_LEN],
            body: too_long,
        }
        .encode_to_vec();
        assert_eq!(
            ChatEnvelope::decode(&wire).err(),
            Some(EnvelopeError::TooLong)
        );
        // And the largest allowed body is allowed, so the bound is a bound and
        // not an off-by-one that quietly costs a character.
        assert!(ChatEnvelope::new(1, vec![b'x'; MAX_BODY]).is_ok());
    }

    #[test]
    fn rubbish_is_refused_rather_than_guessed_at() {
        assert_eq!(
            ChatEnvelope::decode(&[0xFF, 0xFF, 0xFF]).err(),
            Some(EnvelopeError::Malformed)
        );
    }

    /// The plain case: everything in order, nothing missing.
    #[test]
    fn messages_in_order_are_all_new() {
        let mut inbox = Inbox::new();
        for seq in 1..=5 {
            assert_eq!(inbox.receive(seq), Delivery::Accepted);
        }
        assert_eq!(inbox.through(), 5);
        assert!(inbox.pending().is_empty());
    }

    /// R0-F5's reunion: a run queued during an absence, arriving at once and
    /// out of order. The acceptance criterion is that the result is clean.
    #[test]
    fn a_reunion_arrives_reordered_and_ends_up_whole() {
        let mut inbox = Inbox::new();
        // Deliberately shuffled, deterministically, and not merely reversed:
        // one shape of disorder would exercise one path through the run
        // absorbing.
        let arrival = [3u64, 1, 5, 2, 20, 4, 6, 19, 7, 8, 9, 10, 12, 11, 13];
        for seq in arrival {
            assert_eq!(inbox.receive(seq), Delivery::Accepted, "seq {seq}");
        }
        assert_eq!(inbox.through(), 13, "the contiguous run did not close up");
        assert_eq!(
            inbox.pending(),
            vec![14, 15, 16, 17, 18],
            "the hole between 13 and 19 is what the screen has to show as pending"
        );
    }

    /// At-least-once delivery, and the resend an unacked severance causes. Both
    /// arrive as messages the ratchet is right to accept — this is the only
    /// layer that knows better.
    #[test]
    fn a_message_that_already_arrived_is_not_delivered_twice() {
        let mut inbox = Inbox::new();
        assert_eq!(inbox.receive(1), Delivery::Accepted);
        assert_eq!(inbox.receive(2), Delivery::Accepted);
        assert_eq!(inbox.receive(1), Delivery::Duplicate);
        assert_eq!(inbox.receive(2), Delivery::Duplicate);
        assert_eq!(inbox.through(), 2, "a duplicate moved the run");
    }

    /// The out-of-order copy has its own path: it is held in `ahead` rather
    /// than covered by the contiguous run, and a second copy has to be caught
    /// there instead.
    #[test]
    fn a_duplicate_of_a_message_still_waiting_on_a_gap_is_caught() {
        let mut inbox = Inbox::new();
        assert_eq!(inbox.receive(5), Delivery::Accepted);
        assert_eq!(inbox.receive(5), Delivery::Duplicate);
        // Four holes, already: a fifth message having arrived is what makes the
        // first four *known* to be missing rather than merely not sent yet.
        assert_eq!(inbox.pending(), vec![1, 2, 3, 4]);
        assert_eq!(inbox.receive(3), Delivery::Accepted);
        assert_eq!(inbox.receive(3), Delivery::Duplicate);
        assert_eq!(inbox.pending(), vec![1, 2, 4]);
    }

    /// A peer numbering wildly must not be able to make us remember a gap the
    /// size of its imagination.
    #[test]
    fn a_seq_far_beyond_the_run_is_refused_rather_than_tracked() {
        let mut inbox = Inbox::new();
        assert_eq!(inbox.receive(1), Delivery::Accepted);
        assert_eq!(inbox.receive(1 + MAX_AHEAD), Delivery::Accepted);
        assert_eq!(
            inbox.receive(2 + MAX_AHEAD),
            Delivery::TooFarAhead,
            "the bound is off by one: a seq exactly MAX_AHEAD past the run is \
             the last one that fits"
        );
        assert_eq!(inbox.receive(u64::MAX), Delivery::TooFarAhead);
        assert_eq!(
            inbox.ahead().len(),
            1,
            "a refused seq was remembered anyway"
        );
    }

    /// The bound is measured from the contiguous run, so a run that closes up
    /// lets the window move — otherwise a conversation could only ever hold
    /// MAX_AHEAD messages before jamming.
    #[test]
    fn the_window_moves_with_the_run() {
        let mut inbox = Inbox::new();
        // From an empty inbox the window runs to MAX_AHEAD itself: the run is
        // at 0, so that is the furthest a first message can be.
        assert_eq!(inbox.receive(MAX_AHEAD), Delivery::Accepted);
        assert_eq!(inbox.receive(MAX_AHEAD + 1), Delivery::TooFarAhead);
        for seq in 1..MAX_AHEAD {
            assert_eq!(inbox.receive(seq), Delivery::Accepted, "seq {seq}");
        }
        assert_eq!(inbox.through(), MAX_AHEAD, "the run did not close up");
        assert_eq!(inbox.receive(MAX_AHEAD + 1), Delivery::Accepted);
    }

    /// Near the top of the range the bound has to hold without wrapping: a
    /// window that overflowed would let everything through.
    #[test]
    fn the_window_does_not_wrap_at_the_end_of_the_range() {
        let mut inbox = Inbox::resumed(u64::MAX - 1, []);
        assert_eq!(inbox.receive(u64::MAX), Delivery::Accepted);
        assert_eq!(inbox.receive(u64::MAX), Delivery::Duplicate);

        // And the same window on the restore path, which computes it too. A
        // wrap there would put the top of the window *below* the run, so every
        // stored seq would be silently thrown away and every message already
        // delivered would arrive again as new.
        let restored = Inbox::resumed(u64::MAX - 1, [u64::MAX]);
        assert_eq!(restored.ahead(), vec![u64::MAX]);
    }

    /// The seam persistence needs (slice 3): what an inbox is, taken out and
    /// put back.
    #[test]
    fn an_inbox_survives_being_stored_and_restored() {
        let mut inbox = Inbox::new();
        for seq in [1, 2, 3, 7, 9] {
            inbox.receive(seq);
        }
        let restored = Inbox::resumed(inbox.through(), inbox.ahead());
        assert_eq!(restored.through(), inbox.through());
        assert_eq!(restored.ahead(), inbox.ahead());
        assert_eq!(restored.pending(), inbox.pending());
    }

    /// A stored inbox whose two halves disagree must not end up with a message
    /// that is both delivered and still expected.
    #[test]
    fn restoring_drops_anything_the_run_already_covers() {
        let restored = Inbox::resumed(5, [2, 5, 6]);
        assert_eq!(
            restored.ahead(),
            vec![6],
            "a seq inside the contiguous run was kept as if still waiting"
        );
    }

    /// Restoring re-applies the window, because what comes back has been
    /// through the store and possibly through a build whose `MAX_AHEAD` was
    /// larger.
    ///
    /// The failure this prevents is not a wrong answer, it is a phone that
    /// stops: `pending` walks from the run to the highest entry, so one absurd
    /// `seq` in the restored set makes opening the thread an attempt to build a
    /// list of billions.
    #[test]
    fn restoring_refuses_a_seq_from_outside_the_window() {
        let restored = Inbox::resumed(10, [11, 10 + MAX_AHEAD, 11 + MAX_AHEAD, u64::MAX]);
        assert_eq!(
            restored.ahead(),
            vec![11, 10 + MAX_AHEAD],
            "a seq from outside the window survived being restored"
        );
        // The proof that it is bounded: this returns rather than running for
        // the rest of the afternoon.
        assert_eq!(restored.pending().len() as u64, MAX_AHEAD - 2);
    }
}
