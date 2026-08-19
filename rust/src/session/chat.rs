//! The chat envelope and both ends of the pipeline (T12; tech spec §5, §8,
//! R0-F5).
//!
//! What a message *is* on the wire, what a receiver does with one before it
//! reaches the store, and what a sender still owes. The ratchet next door makes
//! a message unreadable to anyone else; this decides whether it is new, where
//! it goes, what is still missing, and what has to go out again.
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

/// How many messages may be waiting on an acknowledgement before an [`Outbox`]
/// stops taking more.
///
/// The sender's mirror of [`MAX_AHEAD`], and bounded for a related reason: a
/// peer that never acknowledges anything would otherwise leave this growing
/// for as long as somebody keeps typing. The same size, because the two are
/// the same conversation seen from each end — a run queued during an absence
/// is exactly what the receiver will have to hold as it arrives.
pub const MAX_UNACKED: usize = 512;

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
    // ── outbox ──────────────────────────────────────────────────────────────

    /// Numbering starts at 1 because 0 is `Inbox::through`'s "nothing yet".
    #[test]
    fn numbering_starts_at_one_and_climbs() {
        let mut out = Outbox::new();
        assert_eq!(out.queue(b"a".to_vec()).unwrap().seq, 1);
        assert_eq!(out.queue(b"b".to_vec()).unwrap().seq, 2);
        assert_eq!(out.next_seq(), 3);
    }

    /// The store's dedup key has to differ per message or two lines a person
    /// sent would collapse into one row.
    #[test]
    fn every_message_gets_its_own_id() {
        let mut out = Outbox::new();
        let a = out.queue(b"same".to_vec()).unwrap();
        let b = out.queue(b"same".to_vec()).unwrap();
        assert_ne!(a.msg_id, b.msg_id, "identical text got one id");
    }

    #[test]
    fn a_queued_message_is_owed_until_it_is_acked() {
        let mut out = Outbox::new();
        let e = out.queue(b"hello".to_vec()).unwrap();
        assert_eq!(out.unacked(), vec![e.seq]);
        assert!(out.acked(e.seq));
        assert!(out.is_empty());
    }

    /// Reported rather than absorbed: an ack for something never sent means the
    /// two sides disagree about what exists, and swallowing it hides that.
    #[test]
    fn an_ack_for_something_not_owed_says_so() {
        let mut out = Outbox::new();
        assert!(!out.acked(7));
        let e = out.queue(b"x".to_vec()).unwrap();
        assert!(out.acked(e.seq));
        assert!(
            !out.acked(e.seq),
            "the same ack settled the same message twice"
        );
    }

    /// Resend order is `seq` order. Out of order, the far side holds every one
    /// of them in `ahead` until the lowest happens to turn up — which is the
    /// reunion case this exists for, made as slow as it can be.
    #[test]
    fn the_resend_list_is_in_order() {
        let mut out = Outbox::new();
        for _ in 0..5 {
            out.queue(b"m".to_vec()).unwrap();
        }
        out.acked(2);
        out.acked(4);
        assert_eq!(out.unacked(), vec![1, 3, 5]);
    }

    /// The bound refuses rather than dropping the oldest. Dropping would lose
    /// something a person typed and told them nothing; this can be shown.
    #[test]
    fn a_full_outbox_refuses_rather_than_forgetting() {
        let mut out = Outbox::new();
        for _ in 0..MAX_UNACKED {
            out.queue(b"m".to_vec()).unwrap();
        }
        assert_eq!(out.queue(b"one more".to_vec()), Err(OutboxError::Full));
        assert_eq!(out.len(), MAX_UNACKED);
        assert!(out.unacked().contains(&1), "the first message was dropped");
    }

    /// The one that matters most here. A refusal that moved the counter would
    /// leave a number nothing was ever sent under, and `Inbox::pending` shows
    /// exactly that as a message still on its way — for ever.
    #[test]
    fn a_refused_message_does_not_burn_a_number() {
        let mut out = Outbox::new();
        out.queue(b"first".to_vec()).unwrap();
        assert_eq!(
            out.queue(vec![0; MAX_BODY + 1]),
            Err(OutboxError::Envelope(EnvelopeError::TooLong))
        );
        assert_eq!(out.next_seq(), 2, "the rejected body consumed a seq");
        assert_eq!(out.queue(b"second".to_vec()).unwrap().seq, 2);

        // And the gap that would have been left is genuinely visible from the
        // other end, which is why this is worth its own assertion.
        let mut inbox = Inbox::new();
        inbox.receive(1);
        inbox.receive(2);
        assert_eq!(inbox.pending(), Vec::<u64>::new());
    }

    #[test]
    fn a_full_outbox_still_lets_an_ack_make_room() {
        let mut out = Outbox::new();
        for _ in 0..MAX_UNACKED {
            out.queue(b"m".to_vec()).unwrap();
        }
        assert!(out.acked(1));
        assert!(out.queue(b"now there is room".to_vec()).is_ok());
    }

    #[test]
    fn resuming_restores_what_was_owed() {
        let out = Outbox::resumed(10, [3, 7, 9]);
        assert_eq!(out.next_seq(), 10);
        assert_eq!(out.unacked(), vec![3, 7, 9]);
    }

    /// Zero is not a number a message can have, so an outbox restored with it
    /// would refuse every `queue` for ever with `NoSeq` — a thread that looks
    /// fine and cannot say anything. The floor makes a zero read as "nothing
    /// sent yet", which is what it means.
    #[test]
    fn resuming_from_zero_can_still_send() {
        let mut out = Outbox::resumed(0, []);
        assert_eq!(out.next_seq(), 1);
        assert_eq!(out.queue(b"first words".to_vec()).unwrap().seq, 1);
    }

    /// Neither is a number that was ever handed out, and letting one through
    /// would mean two different messages could wear it.
    #[test]
    fn resuming_drops_numbers_that_were_never_issued() {
        let out = Outbox::resumed(5, [0, 3, 5, 9]);
        assert_eq!(out.unacked(), vec![3]);
    }

    /// A store written when the bound was larger. The lowest are kept: they are
    /// what blocks the far side's run, so sending them is what lets it advance.
    #[test]
    fn resuming_over_the_bound_keeps_the_ones_that_unblock_the_far_side() {
        let many: Vec<u64> = (1..=(MAX_UNACKED as u64 + 10)).collect();
        let out = Outbox::resumed(MAX_UNACKED as u64 + 20, many);
        assert_eq!(out.len(), MAX_UNACKED);
        assert_eq!(out.unacked()[0], 1);
        assert_eq!(out.unacked()[MAX_UNACKED - 1], MAX_UNACKED as u64);
    }

    /// `Inbox` shipped this same arithmetic as `+ 1` and panicked on the last
    /// value in range. Unreachable in practice; a panic in the send path is
    /// still not how anyone should discover that.
    ///
    /// The boundary is one short of the end, deliberately: `u64::MAX` would
    /// leave nothing to count to next, so it is refused rather than issued.
    #[test]
    fn the_end_of_the_range_refuses_instead_of_panicking() {
        let mut out = Outbox::resumed(u64::MAX - 1, []);
        assert_eq!(
            out.queue(b"the last one".to_vec()).unwrap().seq,
            u64::MAX - 1
        );
        assert_eq!(out.queue(b"one past".to_vec()), Err(OutboxError::Exhausted));
    }

    /// And the exhausted error is not the full one. They are different faults
    /// and the message a person is shown has to say which.
    #[test]
    fn running_out_of_numbers_is_not_reported_as_a_full_outbox() {
        let mut out = Outbox::resumed(u64::MAX, []);
        assert_eq!(out.queue(b"x".to_vec()), Err(OutboxError::Exhausted));
        assert!(
            out.is_empty(),
            "nothing was waiting, so 'full' would be a lie"
        );
    }

    /// Two faults at once, and the one reported has to be the one actually in
    /// the way. A stream with no numbers left does not care how long the body
    /// is: shortening it would change nothing, so blaming the body sends
    /// somebody to fix the wrong thing.
    #[test]
    fn an_exhausted_stream_says_so_even_when_the_body_is_also_bad() {
        let mut out = Outbox::resumed(u64::MAX, []);
        assert_eq!(
            out.queue(vec![0; MAX_BODY + 1]),
            Err(OutboxError::Exhausted),
            "reported the body when the stream was the problem"
        );
    }

    /// The invariant a resend rests on, stated as a test: the numbers handed
    /// back are the ones that were issued, so the rows they name still carry
    /// the `msg_id` the far side saw.
    #[test]
    fn the_resend_list_names_the_numbers_that_were_issued() {
        let mut out = Outbox::new();
        let sent: Vec<u64> = (0..3)
            .map(|_| out.queue(b"m".to_vec()).unwrap().seq)
            .collect();
        assert_eq!(out.unacked(), sent);
    }

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

/// Why a message could not be queued.
#[derive(Debug, PartialEq, Eq)]
pub enum OutboxError {
    /// The body did not make a valid envelope.
    Envelope(EnvelopeError),
    /// [`MAX_UNACKED`] messages are already waiting.
    Full,
    /// The stream has no numbers left.
    ///
    /// Its own variant rather than folding into [`OutboxError::Full`], which
    /// would tell somebody that five hundred messages are waiting when none
    /// are. Unreachable — it is eighteen quintillion messages to one person —
    /// but an error that misdescribes the fault is worse than an error nobody
    /// sees, because the one person who ever sees it has no other information.
    Exhausted,
}

impl std::fmt::Display for OutboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutboxError::Envelope(e) => write!(f, "{e}"),
            OutboxError::Full => {
                write!(
                    f,
                    "{MAX_UNACKED} messages are already waiting to be delivered"
                )
            }
            OutboxError::Exhausted => write!(f, "this conversation is out of sequence numbers"),
        }
    }
}

impl std::error::Error for OutboxError {}

/// One direction of one thread's outgoing messages: what number comes next, and
/// what has gone out without being acknowledged.
///
/// The mirror of [`Inbox`], and one per thread, because `seq` is per-sender.
///
/// # Why this holds numbers and not messages
///
/// A message's body and `msg_id` are in the store the moment it is queued, and
/// that is what makes them survive the app being closed. Holding a second copy
/// here would be a second answer to "what did we send", able to disagree with
/// the first after a crash between the two writes — and it would put a
/// conversation's worth of bodies in memory to no end. So this holds the
/// decision, and the store holds the messages.
///
/// # What a resend must not do
///
/// Give the message a new `seq` or a new `msg_id`. The ratchet will encrypt it
/// afresh — new number, new key, and it is right to, since cryptographically it
/// has never sent this — so the *only* thing that tells the far side it already
/// has this message is the pair of identifiers that stayed the same. Reassign
/// either and a resend becomes a duplicate on somebody's screen.
///
/// That is why [`Outbox::unacked`] hands back the `seq` numbers rather than
/// rebuilding envelopes: there is nothing here to rebuild them from, which is
/// the point. The caller reads the rows the numbers name.
#[derive(Clone, Debug)]
pub struct Outbox {
    /// Always at least 1. `seq` 0 is what [`Inbox::through`] uses for "nothing
    /// yet", so it is never a message.
    next_seq: u64,
    /// Sent, not yet acknowledged, in `seq` order — which is resend order, and
    /// the reason this is a `BTreeSet`. A `HashSet` would resend a reunion's
    /// backlog in an arbitrary order, and the far side would hold every one of
    /// them in `ahead` until the lowest happened to turn up.
    unacked: BTreeSet<u64>,
}

impl Default for Outbox {
    fn default() -> Self {
        Self {
            next_seq: 1,
            unacked: BTreeSet::new(),
        }
    }
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore one from the store: the next number to hand out, and what was
    /// still waiting.
    ///
    /// Both bounds re-applied, as in [`Inbox::resumed`] and for the same
    /// reason — what comes back has been through the store and possibly
    /// through a different build. `seq` 0 is dropped because it is not a
    /// message, and anything at or above `next_seq` because it has not been
    /// handed out; either would mean two different messages could end up
    /// wearing one number.
    ///
    /// Over [`MAX_UNACKED`], the *lowest* numbers are kept. They are the ones
    /// blocking the far side's contiguous run, so delivering them is what lets
    /// it advance; keeping the newest instead would leave a hole at the front
    /// that nothing later can close. The rest are still in the store — this
    /// bounds what is resent, not what exists.
    pub fn resumed(next_seq: u64, unacked: impl IntoIterator<Item = u64>) -> Self {
        let next_seq = next_seq.max(1);
        Self {
            next_seq,
            unacked: unacked
                .into_iter()
                .filter(|seq| *seq > 0 && *seq < next_seq)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(MAX_UNACKED)
                .collect(),
        }
    }

    /// The number the next message will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Take a message for sending: assign it a number and an id, and remember
    /// that it is owed.
    ///
    /// # Nothing is consumed by a refusal
    ///
    /// Both refusals happen before `next_seq` moves, and that is not tidiness.
    /// A rejected message that burned a number would leave a gap the far side
    /// can never fill — [`Inbox::pending`] would show it as a message still on
    /// its way, for ever, because nothing was ever sent under it.
    pub fn queue(&mut self, body: Vec<u8>) -> Result<ChatEnvelope, OutboxError> {
        if self.unacked.len() >= MAX_UNACKED {
            return Err(OutboxError::Full);
        }
        // Worked out before the envelope so that an exhausted stream reports
        // itself as one. Built the other way round, a stream with no numbers
        // left and an over-long body answered `Envelope(TooLong)` — true, and
        // not the thing standing in the way, since shortening the body would
        // change nothing.
        //
        // Checked at all because `Inbox` had this written as `+ 1` and panicked
        // on the last value in range, and a panic in the send path is not how
        // anyone should meet an edge this far out. The cost is that `u64::MAX`
        // is never issued: there would be no number after it, and a counter
        // that cannot advance is worse than being one message short of the end
        // of a range nothing reaches.
        let Some(next) = self.next_seq.checked_add(1) else {
            return Err(OutboxError::Exhausted);
        };
        let envelope = ChatEnvelope::new(self.next_seq, body).map_err(OutboxError::Envelope)?;
        // Only now, so a refused body still leaves the number unspent.
        self.next_seq = next;
        self.unacked.insert(envelope.seq);
        Ok(envelope)
    }

    /// The far side confirmed it has this one. Returns whether it was owed, so
    /// an acknowledgement for something already settled — or never sent — is
    /// visible rather than silently absorbed.
    pub fn acked(&mut self, seq: u64) -> bool {
        self.unacked.remove(&seq)
    }

    /// Everything still owed, lowest first: the resend list when a pipe opens.
    pub fn unacked(&self) -> Vec<u64> {
        self.unacked.iter().copied().collect()
    }

    /// How many are waiting.
    pub fn len(&self) -> usize {
        self.unacked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.unacked.is_empty()
    }
}
