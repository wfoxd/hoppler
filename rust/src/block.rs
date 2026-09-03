//! The block list, and the one door every ingress goes through (R0-F10, tech
//! spec §12).
//!
//! # Why this is a module and not two fields
//!
//! Blocking is enforced in four places that share no code: the persona
//! endpoint, the session handshake, an established session, and the local
//! nearby list. Before this module there were two copies of the set — one in
//! `Discovery`, one in `Net` — kept in step by a single function remembering to
//! write both. Two copies of a security decision is one copy and a bug waiting
//! for the third surface to be written.
//!
//! There is now one [`Blocklist`], created at start-up, shared by `Arc`. Adding
//! a surface means asking it; there is nothing else to ask.
//!
//! # Why a peer has more than one handle
//!
//! R0-F10 binds a block to Layer-1, and the value that expresses is a peer's
//! pseudonym toward us. There is exactly one way to learn it — they dial us,
//! and the Noise handshake proves it — because deriving it needs their Layer-1
//! *seed*, and pairing only ever gives us their Layer-1 public.
//!
//! Which side dials is decided by comparing two rotating ids, so for any one
//! person we hold that value about half the time. A block list that accepted
//! nothing else would refuse to exist for the other half, which is the case a
//! person is most likely to be reaching for it in.
//!
//! So a block binds to the strongest handle this device holds, and every
//! ingress asks with all of them. See [`Handle`] for what "strongest" means and
//! T18b for why the weaker two cost the requirement nothing.
//!
//! # Why the answer is not a `bool`
//!
//! An `is_blocked(..) -> bool` reads perfectly well in either polarity, which is
//! exactly why an inverted condition survives review. [`Admit`] does not read
//! well backwards, and it is `#[must_use]`, so a caller that fetches a decision
//! and forgets to act on it does not compile. `session::handshake` makes the
//! same move with its typestate for the same reason: the check should precede
//! the response by construction, not by everyone remembering.
//!
//! # Why the refusing variant is called `Silence`
//!
//! Because that is what it produces. R0-F10 requires a blocked device to
//! observe nothing distinguishable from Discovery being off, so every refusal
//! in this system — blocked, rate-limited, off, unparseable — is the same
//! nothing. A variant named `Blocked` would invite a caller to log which one it
//! was, and that log line would be the only artefact in existence that
//! distinguishes them.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::crypto::dh;

/// A thirty-two byte value that identifies a peer to the block list.
///
/// Three different things end up here — see [`Handle`] — and to the gate they
/// are all the same: bytes that mean *not this device*. The kind is recorded
/// beside the block so a person can be told how durable theirs is; the lookup
/// does not know and must not need to.
pub type Pseudonym = [u8; dh::PUBLIC_LEN];

/// What kind of handle a block was written against.
///
/// **Declared weakest first**, because `Ord` is derived from declaration order
/// and the useful comparison is "which of these is the strongest claim". A
/// block records every handle this device holds, so a person has several of
/// these at once and `max()` over them is how anything reports how durable
/// their block actually is.
///
/// The ordering is a *ranking*, not a storage format. [`Self::to_i64`] maps by
/// name for exactly that reason: reordering the variants — which is the kind of
/// edit this derive invites — must not silently rewrite what is on disk.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Handle {
    /// The rotating rung id, hashed. Identifies the peer until the next
    /// rotation — about twelve minutes — and nothing after that.
    ///
    /// Last resort, and the only handle available for a device that has never
    /// answered a persona request.
    Device,
    /// The peer's Layer-2 persona key. Survives a rotation and every rename;
    /// stops matching the moment they generate a new persona, which R0-F10
    /// names as the thing a block must survive.
    PersonaKey,
    /// The peer's Layer-1-derived pseudonym toward us, proven by a handshake
    /// they opened. This is R0-F10's promise: a new name, a new colour and a
    /// fresh Layer-2 persona all leave it unchanged.
    Pseudonym,
}

impl Handle {
    /// The wire/storage form. Explicit rather than `as i64`, so reordering the
    /// variants — which the `Ord` above invites — cannot silently rewrite what
    /// is already on disk.
    pub fn to_i64(self) -> i64 {
        match self {
            Handle::Device => 0,
            Handle::PersonaKey => 1,
            Handle::Pseudonym => 2,
        }
    }

    /// Read one back, rejecting anything this build does not know rather than
    /// guessing — a block whose kind cannot be read is still a block, and the
    /// caller decides what to do about the unknown.
    pub fn from_i64(v: i64) -> Option<Self> {
        match v {
            0 => Some(Handle::Device),
            1 => Some(Handle::PersonaKey),
            2 => Some(Handle::Pseudonym),
            _ => None,
        }
    }
}

/// What an ingress point is allowed to do with a peer.
///
/// `#[must_use]` on purpose: the compiler is the only reviewer that reads every
/// call site.
#[must_use = "an ingress decision that is dropped is an ingress that admits everyone"]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Admit {
    /// Carry on. Nothing about this peer says otherwise.
    Yes,
    /// Say nothing at all — not an error, not a close, not a log line.
    Silence,
}

/// Pseudonyms this device refuses to hear from.
///
/// A set rather than a `Vec`: the lookup is on the receive path of every frame,
/// and the previous linear scan was fine only because the list was always
/// empty.
#[derive(Default)]
pub struct Blocklist {
    who: Mutex<HashSet<Pseudonym>>,
    /// Pseudonyms learned while refusing a dial, waiting to be written down.
    ///
    /// An outbox rather than a callback, because the learning happens on the
    /// transport pump thread inside `Net`, which has no store — and the two
    /// other ways to get it out are worse. A `NetEvent` would do it, but
    /// `NetEvent` derives `Debug` and is logged, so a variant announcing a
    /// refused blocked dial would be the one artefact in the system that
    /// distinguishes "blocked" from "absent". A callback into the store from
    /// this lock would invert the lock order the rest of the crate keeps.
    ///
    /// Each entry pairs the handle that *matched* with the pseudonym now
    /// proven: the first is how the engine finds which contact the block
    /// belongs to, which `Net` has no way of knowing.
    ///
    /// **Lock order: `who` before `learned`, never the reverse.** The two are
    /// needed together wherever a lesson and the set it came from must move as
    /// one — learning it, and forgetting it on an unblock. Taking them one
    /// after the other instead lets an unblock and a refused dial interleave,
    /// and the row that gets written then names a contact whose block has just
    /// been deleted: a block nothing on screen can show and no unblock can
    /// reach.
    learned: Mutex<Vec<(Pseudonym, Pseudonym)>>,
}

impl Blocklist {
    /// The list as it stands on disk. Called once, from `engine::install`,
    /// before anything can answer a peer.
    ///
    /// Goes through [`Self::block`] rather than collecting directly, so a zero
    /// handle that somehow reached the table cannot get in this way either.
    pub fn loaded(entries: impl IntoIterator<Item = Pseudonym>) -> Self {
        let list = Self::default();
        for e in entries {
            list.block(e);
        }
        list
    }

    /// **The gate.** Every ingress point in the app calls this and nothing else.
    ///
    /// Takes every handle the caller holds for this peer, because a peer is
    /// identified by different things at different surfaces and a block written
    /// against any of them is a block. A session we dialled, for one, has no
    /// pseudonym to offer — the static it proved is the peer's Layer-2 key —
    /// and would admit a blocked device if it could only ask with one value.
    ///
    /// Passing fewer handles gates less. It cannot gate *wrongly*, which is the
    /// property being bought: there is no polarity to invert and no way to ask
    /// the set a question it answers backwards.
    ///
    /// # Where a handle is weaker than it looks
    ///
    /// A block is only worth the strength of the handle it is checked against.
    /// Three of the four surfaces pass a pseudonym that was **proved**: the
    /// session handshake and an established session key on the initiator's
    /// Noise static, which the handshake demonstrates possession of, and the
    /// nearby list keys on what the store recorded from one of those.
    ///
    /// The persona endpoint is the exception and is worth naming rather than
    /// leaving implied. `discovery::protocol::Request` carries a **claimed**
    /// pseudonym — nothing in that frame shows the sender holds the matching
    /// secret — so the check there is advisory: an evader can present any 32
    /// bytes and be admitted.
    ///
    /// That is deliberate and it costs nothing. What passing the endpoint buys
    /// is a persona record, which is public and already readable off our
    /// advertisement. To do anything further the same device must complete a
    /// handshake, and there the pseudonym is a key it has to hold. So the
    /// endpoint check sheds honest traffic early; it is not what makes a block
    /// stick.
    ///
    /// A decision that is fetched and then dropped is a door that admits
    /// everyone, so it does not build:
    ///
    /// ```compile_fail
    /// #![deny(unused_must_use)]
    /// let list = rust_lib_hoppler::block::Blocklist::default();
    /// list.ingress_gate(&[[0u8; 32]]);
    /// ```
    ///
    /// The control for the above — same lint, same call, decision used. Without
    /// this, a typo would make the `compile_fail` pass while proving nothing:
    ///
    /// ```
    /// #![deny(unused_must_use)]
    /// use rust_lib_hoppler::block::{Admit, Blocklist};
    /// let list = Blocklist::default();
    /// assert_eq!(list.ingress_gate(&[[0u8; 32]]), Admit::Yes);
    /// ```
    pub fn ingress_gate(&self, handles: &[Pseudonym]) -> Admit {
        let who = self.who.lock().unwrap_or_else(|e| e.into_inner());
        if handles.iter().any(|h| who.contains(h)) {
            Admit::Silence
        } else {
            Admit::Yes
        }
    }

    /// Add a handle. Idempotent — blocking someone twice is one block.
    ///
    /// **The all-zero handle is refused**, and that guard is not decoration.
    /// Zero is a live sentinel in two places: `Request::UNKNOWN` is what a
    /// first-contact discovery request carries, and `ensure_contact` writes a
    /// zero `l2_pub` for a peer whose persona has never been fetched. If zero
    /// ever reached this set, every such peer would match, and a block on one
    /// stranger would silently become a block on all of them.
    ///
    /// Refused here rather than checked at each call site, because the call
    /// sites are the thing that keeps growing.
    pub fn block(&self, who: Pseudonym) {
        // Dropped rather than asserted on. An assertion would make the guard
        // untestable and would turn a caller's mistake into a crash on a path
        // whose entire job is to be the safe place for one.
        if who == [0u8; dh::PUBLIC_LEN] {
            return;
        }
        self.who
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(who);
    }

    /// Keep the pseudonym a refused dial just proved (R0-F10, T18e).
    ///
    /// Called on the refusal path with every handle the caller asked with and
    /// the pseudonym the handshake proved. If the block that fired was bound to
    /// something weaker — a Layer-2 persona key, which is what half of blocks
    /// have, or a rotating device id — this is the moment the durable value
    /// becomes available, and it is available *before any bytes go out*: that
    /// is what `handshake::Responder::read_first` is split for.
    ///
    /// Adds it to the live set at once, because enforcement should not wait for
    /// a disk write, and queues it for [`Self::take_learned`]. Does nothing if
    /// the pseudonym is already known, so a peer that dials repeatedly is
    /// learned once.
    ///
    /// Silent. No log line, here or at the call site — the refusal leaves no
    /// artefact, and "upgraded a block" would be one.
    pub fn note_proved(&self, asked_with: &[Pseudonym], proven: Pseudonym) {
        if proven == [0u8; dh::PUBLIC_LEN] {
            return;
        }
        // Both locks, in the stated order — see the note on `learned`.
        let mut who = self.who.lock().unwrap_or_else(|e| e.into_inner());
        let mut learned = self.learned.lock().unwrap_or_else(|e| e.into_inner());
        // Already durable, or this was not a block at all.
        if who.contains(&proven) {
            return;
        }
        let Some(matched) = asked_with.iter().find(|h| who.contains(*h)) else {
            return;
        };
        let pair = (*matched, proven);
        who.insert(proven);
        learned.push(pair);
    }

    /// Take what has been learned since the last call, for writing down.
    ///
    /// Draining rather than reading, so two drains cannot write the same row
    /// twice. Anything the caller fails to persist must come back through
    /// [`Self::relearn`] — an entry dropped on a store error is a block that
    /// silently goes weak again at the next launch.
    pub fn take_learned(&self) -> Vec<(Pseudonym, Pseudonym)> {
        std::mem::take(&mut *self.learned.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Put back what could not be written.
    ///
    /// Prepended, so the order survives a partial drain: earlier lessons stay
    /// earlier.
    pub fn relearn(&self, entries: Vec<(Pseudonym, Pseudonym)>) {
        if entries.is_empty() {
            return;
        }
        let mut learned = self.learned.lock().unwrap_or_else(|e| e.into_inner());
        let tail = std::mem::take(&mut *learned);
        learned.extend(entries);
        learned.extend(tail);
    }

    /// Remove **one** handle from the set.
    ///
    /// Named for what it does, for the same reason `Store::unblock_handle` is:
    /// a person is blocked on several handles at once, so lifting a block means
    /// removing all of theirs. Lifting one leaves the others enforcing, which
    /// is a person told they are unblocked and still refused.
    ///
    /// Restores stranger-level status and nothing else — whatever the block
    /// revoked stays revoked (R0-F10). The screen for this is T18c.
    ///
    /// # It also forgets what this handle taught
    ///
    /// A pseudonym learned from a refused dial (T18e) is in the live set
    /// *before* it is on disk, and it is queued to be written. Lifting a block
    /// without accounting for both leaves two ways for it to come back: the
    /// queued entry gets written after the unblock, resurrecting the block, and
    /// the live-set pseudonym is invisible to a caller working from stored rows
    /// — so the person stays refused while the screen says they are not.
    ///
    /// So removing a handle also drops anything it taught: the queued entries
    /// it produced, and the pseudonyms they carry.
    pub fn unblock_handle(&self, handle: &Pseudonym) {
        // Both locks, in the stated order — see the note on `learned`. Held
        // together and not one after the other: a `note_proved` slipping
        // between them would re-queue a lesson for the handle being lifted, and
        // the row it later wrote would name a contact whose block has just been
        // deleted.
        let mut who = self.who.lock().unwrap_or_else(|e| e.into_inner());
        let mut learned = self.learned.lock().unwrap_or_else(|e| e.into_inner());

        let taught: Vec<Pseudonym> = learned
            .iter()
            .filter(|(matched, _)| matched == handle)
            .map(|(_, proven)| *proven)
            .collect();
        learned.retain(|(matched, _)| matched != handle);

        who.remove(handle);
        for p in taught {
            who.remove(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: Pseudonym = [1u8; dh::PUBLIC_LEN];
    const BOB: Pseudonym = [2u8; dh::PUBLIC_LEN];

    #[test]
    fn the_gate_answers_for_the_person_it_was_asked_about() {
        let list = Blocklist::default();
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Yes);
        list.block(ALICE);
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Silence);
        assert_eq!(
            list.ingress_gate(&[BOB]),
            Admit::Yes,
            "blocking one person closed the door on another"
        );
        list.unblock_handle(&ALICE);
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Yes);
    }

    #[test]
    fn a_list_starts_life_holding_what_was_on_disk() {
        let list = Blocklist::loaded([ALICE]);
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Silence);
        assert_eq!(list.ingress_gate(&[BOB]), Admit::Yes);
    }

    /// Zero is a sentinel in two live code paths, and a zero on this list
    /// would turn one block into a block on every peer that carries it.
    #[test]
    fn the_all_zero_handle_cannot_get_on_the_list() {
        let list = Blocklist::default();
        list.block([0u8; dh::PUBLIC_LEN]);
        assert_eq!(
            list.ingress_gate(&[[0u8; dh::PUBLIC_LEN]]),
            Admit::Yes,
            "the zero sentinel was accepted as a peer"
        );
    }

    /// A gate asked with several handles refuses if *any* of them is blocked —
    /// a session we dialled has no pseudonym to offer, only a persona key.
    #[test]
    fn any_one_blocked_handle_is_enough() {
        let list = Blocklist::default();
        list.block(BOB);
        assert_eq!(list.ingress_gate(&[ALICE, BOB]), Admit::Silence);
        assert_eq!(list.ingress_gate(&[BOB, ALICE]), Admit::Silence);
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Yes);
        assert_eq!(
            list.ingress_gate(&[]),
            Admit::Yes,
            "nothing known, nothing to refuse"
        );
    }

    /// The order is what the block action reads to pick the best handle it
    /// holds, and what stops an upgrade ever making a block weaker.
    #[test]
    fn handles_rank_weakest_first() {
        assert!(Handle::Pseudonym > Handle::PersonaKey);
        assert!(Handle::PersonaKey > Handle::Device);
        for h in [Handle::Device, Handle::PersonaKey, Handle::Pseudonym] {
            assert_eq!(Handle::from_i64(h.to_i64()), Some(h));
        }
        assert_eq!(
            Handle::from_i64(3),
            None,
            "an unknown kind must not be guessed at"
        );
    }

    /// The whole of T18e, at this layer: a block bound to a weak handle picks
    /// up the durable one when a refused dial proves it.
    #[test]
    fn a_refused_dial_teaches_a_weak_block_the_durable_handle() {
        const PERSONA_KEY: Pseudonym = [3u8; dh::PUBLIC_LEN];
        const PROVEN: Pseudonym = [4u8; dh::PUBLIC_LEN];
        let list = Blocklist::default();
        list.block(PERSONA_KEY);

        list.note_proved(&[PROVEN, PERSONA_KEY], PROVEN);

        assert_eq!(
            list.ingress_gate(&[PROVEN]),
            Admit::Silence,
            "the durable handle was not adopted, so regenerating a persona \
             still evades this block"
        );
        assert_eq!(list.take_learned(), vec![(PERSONA_KEY, PROVEN)]);
        assert!(
            list.take_learned().is_empty(),
            "draining twice handed the same lesson out twice"
        );
    }

    /// Learning is not a way in. A pseudonym proved by somebody who is *not*
    /// blocked must not end up on the list.
    #[test]
    fn a_stranger_who_dials_is_not_learned() {
        let list = Blocklist::default();
        list.block(ALICE);

        list.note_proved(&[BOB], BOB);

        assert_eq!(list.ingress_gate(&[BOB]), Admit::Yes);
        assert!(list.take_learned().is_empty());
    }

    /// A blocked peer dials over and over. The lesson is recorded once.
    #[test]
    fn the_same_lesson_is_learned_once() {
        const PERSONA_KEY: Pseudonym = [3u8; dh::PUBLIC_LEN];
        const PROVEN: Pseudonym = [4u8; dh::PUBLIC_LEN];
        let list = Blocklist::default();
        list.block(PERSONA_KEY);

        for _ in 0..5 {
            list.note_proved(&[PROVEN, PERSONA_KEY], PROVEN);
        }

        assert_eq!(
            list.take_learned().len(),
            1,
            "a peer that keeps dialling wrote a row per attempt"
        );
    }

    /// The zero sentinel cannot arrive this way either.
    #[test]
    fn a_zero_pseudonym_is_not_learned() {
        let list = Blocklist::default();
        list.block(ALICE);
        list.note_proved(&[ALICE], [0u8; dh::PUBLIC_LEN]);
        assert_eq!(list.ingress_gate(&[[0u8; dh::PUBLIC_LEN]]), Admit::Yes);
        assert!(list.take_learned().is_empty());
    }

    /// Lifting a block must take what that block *taught* with it.
    ///
    /// A learned pseudonym lives in two places before it reaches disk: the live
    /// set, and the queue waiting to be written. An unblock that cleared
    /// neither leaves the person refused by the live set while the screen says
    /// they are not — and the queued entry writes the block back at the next
    /// drain.
    #[test]
    fn unblocking_forgets_what_the_block_taught() {
        const PERSONA_KEY: Pseudonym = [3u8; dh::PUBLIC_LEN];
        const PROVEN: Pseudonym = [4u8; dh::PUBLIC_LEN];
        let list = Blocklist::default();
        list.block(PERSONA_KEY);
        list.note_proved(&[PROVEN, PERSONA_KEY], PROVEN);

        list.unblock_handle(&PERSONA_KEY);

        assert_eq!(
            list.ingress_gate(&[PERSONA_KEY]),
            Admit::Yes,
            "the handle itself survived the unblock"
        );
        assert_eq!(
            list.ingress_gate(&[PROVEN]),
            Admit::Yes,
            "the learned pseudonym still refuses, so the person is blocked by \
             something no screen can show and no unblock can reach"
        );
        assert!(
            list.take_learned().is_empty(),
            "a queued lesson outlived its block and will write it back"
        );
    }

    /// What could not be written comes back, in order.
    #[test]
    fn a_lesson_that_could_not_be_written_is_not_lost() {
        const A: Pseudonym = [3u8; dh::PUBLIC_LEN];
        const B: Pseudonym = [4u8; dh::PUBLIC_LEN];
        let list = Blocklist::default();
        list.block(A);
        list.block(B);
        list.note_proved(&[A], [5u8; dh::PUBLIC_LEN]);
        list.note_proved(&[B], [6u8; dh::PUBLIC_LEN]);

        let taken = list.take_learned();
        assert_eq!(taken.len(), 2);
        // The caller managed the first and not the second.
        list.relearn(taken[1..].to_vec());
        list.note_proved(&[A], [7u8; dh::PUBLIC_LEN]);

        let again = list.take_learned();
        assert_eq!(
            again,
            vec![(B, [6u8; dh::PUBLIC_LEN]), (A, [7u8; dh::PUBLIC_LEN])],
            "a returned lesson lost its place in the queue"
        );
    }

    /// Blocking twice must not need unblocking twice.
    #[test]
    fn a_second_block_is_still_one_block() {
        let list = Blocklist::default();
        list.block(ALICE);
        list.block(ALICE);
        list.unblock_handle(&ALICE);
        assert_eq!(list.ingress_gate(&[ALICE]), Admit::Yes);
    }
}
