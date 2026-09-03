//! The engine's networking half (T10 part 2b) — discovery, sessions, and the
//! event loop that joins them.
//!
//! Deliberately **not** a singleton, unlike the engine's `CORE`. The engine is
//! process-wide, so an engine-against-engine test cannot exist; two `Net`s can
//! talk to each other over the loopback rung, which is the only way the
//! stranger→session→Ping path gets tested end to end before it meets a radio.
//!
//! # Who initiates
//!
//! The peer with the lexicographically smaller id, and only that one.
//!
//! The obvious rule — "whoever dialled" — does not survive contact. A pipe
//! opens on *both* sides with nothing to say which end asked, and once two
//! devices have met, both know each other's persona and both are able to start
//! a handshake. They then do: each reads the other's message one as the reply
//! to its own, each fails, each discards its pending handshake, and no session
//! ever forms. That deadlock is invisible to every layer below — the transport
//! is healthy, the crypto is correct, the sessions simply never appear.
//!
//! A total order both ends can compute without agreeing on anything first is
//! enough, and the id comparison is the same tie-break the LAN rung already
//! uses for simultaneous dial. The handshake direction has nothing to do with
//! the dial direction: the larger id may open the pipe and still wait to be
//! spoken to.
//!
//! # What a stranger costs before they are anybody
//!
//! A peer we have never met takes: one sighting, one pipe, and one pending
//! handshake. Nothing is written to the store until a session is established
//! and a frame arrives, so a device that connects and says nothing leaves no
//! trace beyond the transport's own bookkeeping.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::pipe::{self, PipeReader, CHANNEL_DISCOVERY, CHANNEL_SESSION};
use std::cmp::Ordering;

use zeroize::Zeroizing;

use crate::block::{Admit, Blocklist};
use crate::crypto::{dh, sign};
use crate::discovery::protocol::{Request, Response};
use crate::discovery::Discovery;
use crate::identity::{Identity, VerifiedPersona};
use crate::pairing::ceremony::{Ceremony, CeremonyError, Paired, Step};
use crate::pairing::invite::Invite;
use crate::pairing::sas::Sas;
use crate::session::frame::{Frame, FrameKind};
use crate::session::handshake::{Established, HandshakeError, Initiator, Responder};
use crate::session::ratchet::Ratchet;
use crate::session::table::SessionTable;
use crate::transport::{PeerId, Transport, TransportError, TransportEvent};

/// How long a queued Ping waits for a session before it is called undeliverable.
///
/// Long enough for the honest path — dial (3 s in the LAN rung), persona fetch,
/// and an IK handshake — and short enough that a person still connects the
/// answer to the tap that caused it.
pub const PING_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How quiet a peer must go before the engine asks the rung about it.
///
/// Long enough that a rung reporting normally is never asked at all, short
/// enough that a departure is noticed in tens of seconds rather than the two
/// minutes an mDNS record TTL takes. Detection lands at roughly this plus the
/// rung's own probe timeout plus a tick — around 45–70 s on LAN, against ~120 s
/// before and ~15 s on BLE, which advertises continuously and needs none of
/// this. Closer parity would mean a faster clock, and that is a battery cost
/// (N4) paid by every rung to help one.
const QUIET_BEFORE_ASKING: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a frame did not go out.
///
/// Two cases that a `String` ran together, to everyone's cost. "No session
/// yet" is the ordinary state of a peer who is out of range or has simply gone
/// quiet, and the caller's right answer is to leave the message queued and say
/// nothing. Every other refusal — a frame too long to encode, a session that
/// will not seal, a transport that would not take the bytes — happens *with a
/// session open*, so nothing is going to reopen one and retry: the message
/// waits forever while its recipient sits there connected.
///
/// Telling them apart is the caller's whole decision, and a matched variant is
/// the only way to make it that does not turn on the wording of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// No session with that peer yet. Not a failure — nothing to report.
    NoSession,
    /// The session or the transport refused the frame. A real failure.
    Refused(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => write!(f, "no session with that peer yet"),
            Self::Refused(why) => write!(f, "{why}"),
        }
    }
}

/// Something the engine should act on: store a row, emit to Dart, update the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    /// The nearby list changed.
    PeersChanged,
    /// A session is live with a peer whose persona is now known.
    SessionOpened { peer: PeerId, persona_name: String },
    /// A session ended.
    SessionClosed { peer: PeerId },
    /// A Ping arrived from a peer we have a session with.
    Pinged { peer: PeerId, persona_name: String },
    /// A Ping *we* sent came back answered.
    ///
    /// Distinct from [`NetEvent::Pinged`], which is someone nudging us. Folding
    /// the two together made a tap look answered only when the other person
    /// happened to nudge back, and never otherwise.
    PingAcked { peer: PeerId },
    /// A chat line arrived. Opaque bytes, and deliberately so.
    ///
    /// What they are depends on the thread, and `Net` does not know which
    /// thread this is: on a paired one a ratchet header and a sealed
    /// [`ChatEnvelope`](crate::session::chat::ChatEnvelope), on an unpaired one
    /// that envelope encoded in the clear — R0-F5 lets strangers chat, and a
    /// stranger's thread has no ratchet to seal with. Only the engine holds
    /// both the thread and its keys, so only the engine can tell the two apart.
    ///
    /// That is the whole point of the shape. Decoding here instead meant every
    /// rejection this layer could make (`seq == 0`, a short `msg_id`, an
    /// over-long body) threw away a message that would have opened, and with it
    /// the chain step the sender had already taken.
    ChatReceived { peer: PeerId, body: Vec<u8> },
    /// A peer says it has stored a chat message ([`FrameKind::Ack`]).
    ///
    /// Carries the sealed bytes and nothing else, for the same reason
    /// [`NetEvent::ChatReceived`] does: only the engine holds the ratchet, and
    /// only the engine knows which thread this belongs to.
    ChatAcked { peer: PeerId, body: Vec<u8> },
    /// A peer opened its end of a paired thread's ratchet chain
    /// ([`FrameKind::Opening`]).
    ///
    /// Carries the sealed bytes and nothing else. There is no envelope to
    /// decode because there is nothing in it to number: this says only "here is
    /// a message key you can step to", and what it opens to is discarded.
    ChainOpening { peer: PeerId, body: Vec<u8> },
    /// A queued Ping was dropped because the pipe never opened.
    PingUndeliverable { peer: PeerId, why: String },
    /// The radio became usable or unusable, and why.
    ///
    /// Separate from [`NetEvent::PeersChanged`] because an empty list and an
    /// unusable radio are different facts that look identical on screen. F2
    /// turns on being able to tell them apart, and the reason is the only thing
    /// that can: without it "Bluetooth is off" reads as "nobody is nearby".
    RadioChanged {
        available: bool,
        reason: Option<String>,
    },
    /// A ceremony reached the point where two people compare colours (R0-F4).
    ///
    /// Nothing has been disclosed yet and nothing is written down. The next
    /// thing that must happen is a person looking at a screen.
    PairingSas { peer: PeerId, sas: Sas },
    /// The other person confirmed. A cue for the UI and nothing more: it is not
    /// a pairing, and on its own it discloses nothing.
    PairingPeerConfirmed { peer: PeerId },
    /// Both people confirmed and both Layer-1 proofs verified.
    ///
    /// Carries what a contact row needs. Not the ceremony's transport keys —
    /// see `Net::step_ceremony` for why they are dropped here.
    PairingCompleted {
        peer: PeerId,
        persona_name: String,
        persona_colour: u32,
        persona_version: u32,
        l2_pub: [u8; 32],
        l1_pub: [u8; 32],
    },
    /// A ceremony ended without pairing. Every abort path arrives here, and
    /// none of them has written anything.
    PairingFailed { peer: PeerId, why: String },
    /// We dialled someone whose build speaks a different protocol version, so
    /// the session was refused (see [`crate::protocol`]).
    ///
    /// The *only* handshake failure that produces an event. Every other one
    /// stays silent on purpose: they are indistinguishable from an attacker
    /// probing, and R0-F10 needs a blocked peer to look exactly like a peer who
    /// is not there. A version mismatch is different in kind — it is not a
    /// judgement about who is calling, it is a fact about two builds, and it is
    /// the one failure where the honest thing to tell the user is also
    /// actionable. Raised on the dialling side only, where somebody actually
    /// asked for something and is owed an answer.
    SessionRefused {
        peer: PeerId,
        their_version: u8,
        our_version: u8,
    },
}

/// How long a ceremony may sit before it is abandoned.
///
/// It exists because a ceremony can be left half-open by the *other* side
/// failing quietly, which is not a rare case — it is exactly what a stale code
/// produces. The scanner reads a reply bound to a nonce it does not hold, gives
/// up, and sends nothing, because there is nothing useful it could send. Without
/// a clock the shower then holds Noise state indefinitely and, worse, refuses
/// the next attempt from the same person as "already pairing".
///
/// Found by a test that expected an abort to leave zero state on both sides and
/// discovered it left zero state on one.
///
/// Two minutes covers the handshake, where nothing is waiting on a person: if
/// three messages have not crossed in that time the ceremony is not slow, it is
/// stuck.
pub const CEREMONY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// How long a ceremony may sit once both screens are showing colours.
///
/// Much longer, because the deadline stops meaning the same thing. Up to the
/// SAS a silent ceremony is a stalled protocol; after it, a silent ceremony is
/// two people looking at their phones — and the correct amount of time for that
/// is however long they take. Two minutes is not generous for reading colours
/// down a phone line, or for being interrupted halfway.
///
/// This is not a guess. On two phones a ceremony with matching colours on both
/// screens was abandoned at exactly this point, and an earlier run finished
/// with **1.9 seconds** of margin — which was luck rather than headroom.
///
/// A backstop rather than a limit: what actually ends a ceremony is a
/// confirmation, a cancellation, or the pipe going away. This exists so an
/// abandoned one does not hold Noise state and block the next attempt forever.
pub const CEREMONY_CONFIRM_DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);

/// A ceremony and the moment it stops being worth holding.
struct InFlight {
    ceremony: Ceremony,
    deadline: Instant,
    /// True when this ceremony was started by answering the code on our screen,
    /// rather than by reading someone else's.
    ///
    /// The code answers one person at a time, and this is how the second
    /// answerer is recognised — the ceremony map is keyed by peer, so two
    /// people answering the same code are two perfectly valid-looking entries.
    answering_our_code: bool,
}

/// Everything the engine needs a network for.
pub struct Net {
    transport: Arc<dyn Transport>,
    discovery: Discovery,
    sessions: SessionTable,
    identity: Arc<Mutex<Identity>>,
    /// Handshakes we started and are waiting on a reply for.
    pending: Mutex<HashMap<PeerId, Initiator>>,
    /// Personas fetched from the discovery endpoint, needed before a Noise IK
    /// handshake can start (it must know the responder's static in advance).
    known: Mutex<HashMap<PeerId, VerifiedPersona>>,
    /// Pseudonyms the user has blocked (R0-F10). Shared with `Discovery` and
    /// with the engine — see [`crate::block`] for why there is one list and not
    /// three.
    blocked: Arc<Blocklist>,
    /// Pings asked for before a session existed, to be sent once it does —
    /// each with the moment it stops being worth waiting for.
    ///
    /// A deadline rather than a bare set, because "the pipe failed" is a much
    /// narrower event than "it did not arrive". A pipe that opens and then
    /// stalls mid-handshake produces no transport event at all, so without a
    /// clock a queued Ping waits forever and the person who tapped it is told
    /// nothing. Observed on hardware: two phones listing each other, Ping
    /// silent, and only the *chat* path saying "no session".
    pending_pings: Mutex<HashMap<PeerId, Instant>>,
    /// Per-peer stream reassembly. The rung splits and merges freely (T08 rule
    /// 3), so nothing arriving here is a whole message until this says so.
    readers: Mutex<HashMap<PeerId, PipeReader>>,
    /// Ceremonies in flight, at most one per peer (R0-F4).
    ///
    /// Held here rather than in the engine because a ceremony dies with its
    /// peer: the pipe going away has to take it with it, and this is the layer
    /// that hears about that. An abandoned ceremony is not merely untidy —
    /// while one exists, the next code scanned from the same peer is refused as
    /// out of order.
    ///
    /// **Lock order: `identity` before `ceremonies`, never the reverse.** Both
    /// are needed at once because `Ceremony::read` and `Ceremony::confirm` take
    /// `&Identity` while mutating the ceremony. The two orders did not actually
    /// overlap before — `begin_pairing` released the identity guard before
    /// touching this map — but "these happen not to overlap" is a property that
    /// survives exactly until someone edits one of them, so it is a stated
    /// order now. Neither lock is held across `dispatch`, which sends.
    ceremonies: Mutex<HashMap<PeerId, InFlight>>,
    /// Initial ratchet state for a pairing that has just completed, waiting for
    /// the engine to write it down beside the pairing row.
    ///
    /// Held here rather than carried on [`NetEvent::PairingCompleted`] because
    /// that type derives `Debug` and is logged: a ratchet's serialised state is
    /// key material, and the one place it must never reach is a transcript that
    /// outlives the conversation.
    pairing_ratchets: Mutex<HashMap<PeerId, Zeroizing<Vec<u8>>>>,
    /// The code currently on this device's screen, if any.
    ///
    /// Exactly one, because its nonce is what binds a ceremony. Showing a
    /// second code makes the first one dead: a scanner working from the old
    /// image fails the prologue and the handshake stops, which is the correct
    /// outcome and the reason this is an `Option` rather than a set.
    showing: Mutex<Option<Invite>>,
    /// A ceremony's opening message, queued until a session exists.
    ///
    /// Same shape and the same reason as `pending_pings`: `reach` returns on
    /// acceptance, not on a session, so sending immediately after means the
    /// first attempt at pairing always fails and the second works.
    pending_ceremony: Mutex<HashMap<PeerId, Vec<Vec<u8>>>>,
}

impl Net {
    pub fn new(
        transport: Arc<dyn Transport>,
        identity: Arc<Mutex<Identity>>,
        blocked: Arc<Blocklist>,
        local_id: &str,
        now: Instant,
    ) -> Self {
        let discovery = Discovery::new(transport.clone(), identity.clone(), blocked.clone(), now);
        // Seed discovery with the id the transport was built under, so the
        // tie-break has the right answer before the first advertisement.
        discovery.set_local_id_for_tiebreak(local_id);
        Self {
            transport,
            discovery,
            sessions: SessionTable::new(),
            identity,
            pending: Mutex::new(HashMap::new()),
            known: Mutex::new(HashMap::new()),
            blocked,
            readers: Mutex::new(HashMap::new()),
            pending_pings: Mutex::new(HashMap::new()),
            ceremonies: Mutex::new(HashMap::new()),
            pairing_ratchets: Mutex::new(HashMap::new()),
            showing: Mutex::new(None),
            pending_ceremony: Mutex::new(HashMap::new()),
        }
    }

    /// Whether we are the side that opens the handshake with this peer.
    ///
    /// Reads the id from `Discovery` every time rather than caching it. A cached
    /// copy would go stale on the next rotation — both sides could then believe
    /// they were the smaller id, or neither would — and the double-initiator
    /// deadlock this tie-break exists to prevent would come straight back, now
    /// only after a twelve-minute timer. One source of truth is the fix; keeping
    /// two in sync is the bug waiting to happen.
    fn we_initiate(&self, peer: &str) -> bool {
        self.discovery.local_id().as_str() < peer
    }

    pub fn discovery(&self) -> &Discovery {
        &self.discovery
    }

    pub fn sessions(&self) -> &SessionTable {
        &self.sessions
    }

    /// The list this `Net` and its `Discovery` both enforce.
    pub fn blocklist(&self) -> &Arc<Blocklist> {
        &self.blocked
    }

    /// Reach a peer: dial if needed. The session follows once the pipe opens.
    pub fn reach(&self, peer: &str) -> Result<(), TransportError> {
        if self.sessions.is_open(peer) {
            return Ok(());
        }
        self.transport.connect(peer)
    }

    /// Send a Ping, establishing a session first if there is not one.
    ///
    /// The obvious version — reach, then send — cannot work. `reach` returns on
    /// *acceptance* (T08 rule 2), and a session needs the pipe to open, the
    /// persona to be fetched and an IK handshake to complete: several round
    /// trips. Sending immediately after means the very first Ping to any peer
    /// always fails and the second succeeds, which is what a person would
    /// report as "I have to tap it twice".
    ///
    /// So a Ping with no session is queued and flushed when the session opens.
    /// `Ok` means accepted, as it already did — the peer's screen is still the
    /// only proof of delivery.
    pub fn ping(&self, peer: &str, now: Instant) -> Result<(), String> {
        if self.sessions.is_open(peer) {
            return self
                .send_frame(peer, FrameKind::Ping, Vec::new(), now)
                .map_err(|e| e.to_string());
        }
        self.pending_pings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer.to_string(), now + PING_DEADLINE);
        log::info!("ping to {peer} queued: no session yet, reaching");
        self.reach(peer).map_err(|e| e.to_string())
    }

    /// Give up on queued Pings whose deadline has passed, and say so.
    ///
    /// Someone has to call this — nothing in the engine ticks, and `handle`
    /// only runs when the transport has something to say, which in exactly the
    /// failing case it never does.
    pub fn expire_pings(&self, now: Instant) -> Vec<NetEvent> {
        let expired: Vec<PeerId> = {
            let mut pings = self.pending_pings.lock().unwrap_or_else(|e| e.into_inner());
            let due: Vec<PeerId> = pings
                .iter()
                .filter(|(_, &deadline)| now >= deadline)
                .map(|(peer, _)| peer.clone())
                .collect();
            for peer in &due {
                pings.remove(peer);
            }
            due
        };
        for peer in &expired {
            // Distinguishes the two ways a Ping can be reported undeliverable.
            // This one means the pipe was fine and the session simply did not
            // arrive in time — on a slow rung that may be the deadline being
            // wrong rather than the peer being unreachable.
            log::warn!("ping to {peer} expired: no session within the deadline");
        }
        expired
            .into_iter()
            .map(|peer| NetEvent::PingUndeliverable {
                peer,
                // Deliberately the same wording as the pipe-failure case. The
                // two are different internally but identical to the person
                // holding the phone, and distinguishing them here would leak
                // whether a peer refused us — the R0-F10 line.
                why: "could not reach that device".into(),
            })
            .collect()
    }

    // ── pairing (R0-F4) ─────────────────────────────────────────────────────

    /// Put a code on this device's screen, replacing any previous one.
    ///
    /// Returns nothing to check because there is nothing to fail. What matters
    /// is the replacement: a code that is no longer displayed must stop working,
    /// and it does, because its nonce is no longer the one this device will
    /// bind a ceremony to.
    pub fn show_invite(&self, invite: Invite) {
        *self.showing.lock().unwrap_or_else(|e| e.into_inner()) = Some(invite);
    }

    /// Take the code off the screen. Any ceremony already under way is
    /// unaffected — it is bound to the nonce it started with, and the person
    /// putting their phone down mid-ceremony has not withdrawn consent to the
    /// ceremony they are in.
    pub fn stop_showing(&self) {
        *self.showing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn showing(&self) -> Option<Invite> {
        self.showing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Begin a ceremony toward a peer whose code this device just read.
    ///
    /// Queued rather than sent if there is no session yet, for the reason
    /// [`Net::ping`] spells out at length: `reach` returns on acceptance, and a
    /// session is several round trips further on.
    pub fn begin_pairing(&self, peer: &str, invite: &Invite, now: Instant) -> Result<(), String> {
        let opening = {
            // Identity first, then ceremonies — see the note on `ceremonies`.
            //
            // The "already pairing" check and the insert happen under *one*
            // hold of the map, because `Net` is shared: the pump thread, the
            // clock thread and Dart all call in. Checking through
            // `ceremony_in_flight` and inserting afterwards — which is what
            // this did — lets two calls for the same peer both pass the check
            // and the second silently replace the first, leaving the first
            // one's peer talking to a ceremony that no longer exists here.
            let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
            let mut ceremonies = self.ceremonies.lock().unwrap_or_else(|e| e.into_inner());
            if ceremonies.contains_key(peer) {
                return Err(format!("already pairing with {peer}"));
            }
            let (ceremony, opening) =
                Ceremony::scan(&identity, invite).map_err(|e| e.to_string())?;
            ceremonies.insert(
                peer.to_string(),
                InFlight {
                    ceremony,
                    deadline: now + CEREMONY_DEADLINE,
                    // We read their code; ours is not what is being answered.
                    answering_our_code: false,
                },
            );
            opening
        };
        // A ceremony that could not get its first message out is not a
        // ceremony, and leaving it in the map would make the obvious response —
        // try again — fail with "already pairing" until the deadline two
        // minutes later. The insert above happens before the send because the
        // send needs the ceremony to exist; unwinding it here is the price.
        if let Err(why) = self.send_ceremony(peer, opening, now) {
            self.forget_ceremony(peer);
            return Err(why);
        }
        Ok(())
    }

    /// This device's human confirmed the colours match.
    ///
    /// Returns what came of it rather than swallowing it. The first version
    /// discarded `dispatch`'s events behind an `Ok(())`, so a confirmation
    /// whose bytes could not be sent looked — to the caller, and to the person
    /// who tapped — exactly like one that worked, with the failure visible only
    /// in a log. `Err` is a caller mistake (no such ceremony); a ceremony that
    /// ended is a `PairingFailed` in the returned list, as everywhere else.
    pub fn confirm_pairing(&self, peer: &str, now: Instant) -> Result<Vec<NetEvent>, String> {
        let steps = {
            // Identity first, then ceremonies — see the note on `ceremonies`.
            let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
            let mut ceremonies = self.ceremonies.lock().unwrap_or_else(|e| e.into_inner());
            let in_flight = ceremonies
                .get_mut(peer)
                .ok_or_else(|| format!("no ceremony with {peer}"))?;
            in_flight.deadline = now + CEREMONY_CONFIRM_DEADLINE;
            in_flight
                .ceremony
                .confirm(&identity)
                .map_err(|e| e.to_string())?
        };
        let events = self.dispatch(peer, steps, now);
        // Confirming sends; it never completes. Pairing finishes when the
        // peer's Layer-1 proof arrives, which is `read`'s job — if that stops
        // being true, the ordering guarantee has moved and this is where it
        // shows. The first version of this assertion was written the other way
        // round and could only trip on success.
        debug_assert!(
            !events
                .iter()
                .any(|e| matches!(e, NetEvent::PairingCompleted { .. })),
            "confirming completed a pairing on its own"
        );
        Ok(events)
    }

    /// Abandon a ceremony — the screen closed, the colours did not match, the
    /// other person walked off.
    ///
    /// No message goes to the peer. There is nothing useful to tell them: a
    /// ceremony that does not complete is indistinguishable from one that was
    /// never started, and their side ends the same way when its own pipe or
    /// person gives up. Nothing was written, so nothing is undone.
    pub fn cancel_pairing(&self, peer: &str) {
        self.forget_ceremony(peer);
    }

    /// Drop every trace of a ceremony with this peer, and say whether there was
    /// one.
    ///
    /// The single place either map is cleared, because they must be cleared
    /// together and were not: `sweep_ceremonies` dropped the expired ceremony
    /// and left its queued opening message behind, so a session opening later
    /// flushed bytes for a ceremony this side no longer had — starting one on
    /// the peer that could never complete. Three callers wanted this (cancel,
    /// sweep, the pipe going away) and only two of them did it in full.
    fn forget_ceremony(&self, peer: &str) -> bool {
        let had = self
            .ceremonies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .is_some();
        let queued = self
            .pending_ceremony
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .is_some();
        had || queued
    }

    /// Abandon ceremonies nothing has moved for [`CEREMONY_DEADLINE`].
    ///
    /// Called from [`Net::tick`], alongside the session sweep and the id
    /// rotation, and for the same reason all three live there: the condition
    /// they fire on is the *absence* of traffic, so no transport event will ever
    /// arrive to carry them.
    fn sweep_ceremonies(&self, now: Instant) -> Vec<NetEvent> {
        // Deciding and removing happen under one hold, and deliberately not
        // through `forget_ceremony`.
        //
        // The clock thread runs this while the pump thread is delivering
        // ceremony traffic, and every inbound message pushes the deadline out.
        // Choosing the victims, releasing the lock, and then removing them
        // unconditionally — which is what calling the helper here did — sweeps
        // a ceremony that made progress in between, killing a pairing two
        // people are in the middle of. This is the one place the two maps are
        // not cleared by the same call, so the queue half follows below.
        let expired: Vec<PeerId> = {
            let mut ceremonies = self.ceremonies.lock().unwrap_or_else(|e| e.into_inner());
            let due: Vec<PeerId> = ceremonies
                .iter()
                .filter(|(_, in_flight)| now >= in_flight.deadline)
                .map(|(peer, _)| peer.clone())
                .collect();
            for peer in &due {
                ceremonies.remove(peer);
            }
            due
        };
        if !expired.is_empty() {
            let mut queued = self
                .pending_ceremony
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for peer in &expired {
                queued.remove(peer);
            }
        }
        expired
            .into_iter()
            .map(|peer| {
                log::info!("ceremony with {peer} abandoned: nothing moved for two minutes");
                NetEvent::PairingFailed {
                    peer,
                    why: "pairing did not complete".into(),
                }
            })
            .collect()
    }

    pub fn ceremony_in_flight(&self, peer: &str) -> bool {
        self.ceremonies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(peer)
    }

    /// Put one ceremony message on the wire, or queue it and reach.
    fn send_ceremony(&self, peer: &str, message: Vec<u8>, now: Instant) -> Result<(), String> {
        if self.sessions.is_open(peer) {
            return self
                .send_frame(peer, FrameKind::Ceremony, message, now)
                .map_err(|e| e.to_string());
        }
        self.pending_ceremony
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(peer.to_string())
            .or_default()
            .push(message);
        log::info!("ceremony message to {peer} queued: no session yet, reaching");
        self.reach(peer).map_err(|e| e.to_string())
    }

    /// Feed an inbound ceremony frame to its ceremony, starting one if this
    /// device is the side showing the code.
    fn on_ceremony_frame(&self, peer: &str, payload: &[u8], now: Instant) -> Vec<NetEvent> {
        let steps = {
            // Identity first, then ceremonies — see the note on `ceremonies`.
            let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
            let mut ceremonies = self.ceremonies.lock().unwrap_or_else(|e| e.into_inner());
            // Asked before the entry is taken, because it is a question about
            // the *other* entries and the borrow below rules them out.
            let code_already_answered = ceremonies.values().any(|f| f.answering_our_code);
            let in_flight = match ceremonies.entry(peer.to_string()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Nothing in flight, so this is someone answering a code we
                    // are showing. If we are not showing one, we are not in a
                    // ceremony, and there is nothing to answer with.
                    //
                    // Silently, and without an event: the person here has not
                    // asked to pair with anybody, so a screen saying "pairing
                    // failed" would be the first they heard of it. Their peer
                    // learns the same thing from the silence.
                    let Some(invite) = self.showing() else {
                        log::info!("ignoring a ceremony message from {peer}: no code on screen");
                        return Vec::new();
                    };
                    // A code on screen is the token for *one* ceremony, and
                    // R0-F4 is between two people who can see each other. Left
                    // open, a second answerer got a second ceremony: two sets
                    // of colours on one screen, two completions, and — since
                    // the store folds both onto one contact — one thread
                    // announced twice with no way to tell which stranger had
                    // just been let in. Two phones logged exactly that.
                    //
                    // Refused while the first is still going, not spent for
                    // good: a ceremony that fails or times out releases the
                    // code, so the ordinary "it did not work, try again" with
                    // the code still on screen keeps working.
                    //
                    // Silent, like the no-code case above and for the same
                    // reason — the person here is already pairing with someone,
                    // and a stranger's failed attempt is not their news.
                    if code_already_answered {
                        log::info!(
                            "ignoring a ceremony message from {peer}: \
                             the code on screen is already being answered"
                        );
                        return Vec::new();
                    }
                    match Ceremony::show(&identity, &invite) {
                        Ok(ceremony) => slot.insert(InFlight {
                            ceremony,
                            deadline: now + CEREMONY_DEADLINE,
                            answering_our_code: true,
                        }),
                        Err(why) => {
                            log::warn!("could not answer {peer}'s ceremony: {why}");
                            return Vec::new();
                        }
                    }
                }
            };
            // Every message is progress, so the clock restarts — and how long
            // it restarts *for* depends on what the ceremony is now waiting on.
            // Before the colours it is waiting on a protocol, which should not
            // take two minutes. After them it is waiting on two people, and the
            // right amount of time for that is however long they take.
            in_flight.deadline = now
                + if in_flight.ceremony.sas().is_some() {
                    CEREMONY_CONFIRM_DEADLINE
                } else {
                    CEREMONY_DEADLINE
                };
            match in_flight.ceremony.read(&identity, payload) {
                Ok(steps) => steps,
                Err(why) => {
                    ceremonies.remove(peer);
                    return vec![Self::pairing_failed(peer, &why)];
                }
            }
        };
        // Reaching the colours changes what the ceremony is waiting for, and
        // that has to take effect now rather than on some later message — there
        // may not be one. This is the exact case that killed a ceremony on two
        // phones with matching colours on both screens.
        if let Some(in_flight) = self
            .ceremonies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(peer)
        {
            if in_flight.ceremony.sas().is_some() {
                in_flight.deadline = now + CEREMONY_CONFIRM_DEADLINE;
            }
        }
        self.dispatch(peer, steps, now)
    }

    /// Turn a ceremony's steps into wire traffic and events.
    ///
    /// The ceremony's own transport keys are dropped when it completes. They
    /// are a second Noise channel inside a session that already has one, and
    /// nothing after pairing speaks through them — keeping them would mean two
    /// live encryption states per peer, with every later message having to pick
    /// one and both sides having to pick the same.
    fn dispatch(&self, peer: &str, steps: Vec<Step>, now: Instant) -> Vec<NetEvent> {
        let mut out = Vec::new();
        for step in steps {
            match step {
                Step::Send(bytes) => {
                    if let Err(why) = self.send_ceremony(peer, bytes, now) {
                        self.forget_ceremony(peer);
                        out.push(NetEvent::PairingFailed {
                            peer: peer.to_string(),
                            why,
                        });
                        return out;
                    }
                }
                Step::ShowSas(sas) => out.push(NetEvent::PairingSas {
                    peer: peer.to_string(),
                    sas,
                }),
                Step::PeerConfirmed => out.push(NetEvent::PairingPeerConfirmed {
                    peer: peer.to_string(),
                }),
                Step::Paired(paired) => {
                    self.forget_ceremony(peer);
                    // Destructured rather than borrowed: the ratchet secret is
                    // not `Clone` — deliberately, so it cannot be copied by
                    // accident — and seeding needs it by value.
                    let Paired {
                        persona,
                        l1_pub,
                        ratchet_secret,
                        their_ratchet,
                        root,
                        ..
                    } = *paired;
                    // Seeded while the ceremony's keys are still here; they go
                    // out of scope at the end of this arm.
                    match self.seed_ratchet(&root, ratchet_secret, their_ratchet, &l1_pub) {
                        Ok(state) => {
                            self.pairing_ratchets
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(peer.to_string(), state);
                        }
                        // Reported, not fatal to the pairing. The identities
                        // crossed and both people confirmed; refusing the whole
                        // ceremony over a ratchet we can rebuild would throw
                        // away the part that needed two humans.
                        Err(why) => {
                            log::warn!("could not seed the ratchet for {peer}: {why}");
                        }
                    }
                    out.push(NetEvent::PairingCompleted {
                        peer: peer.to_string(),
                        persona_name: persona.name.clone(),
                        persona_colour: persona.colour,
                        persona_version: persona.version,
                        l2_pub: persona.l2_pub.0,
                        l1_pub: l1_pub.0,
                    });
                }
            }
        }
        out
    }

    /// Build this side's opening ratchet from what the ceremony agreed.
    ///
    /// # Who speaks first
    ///
    /// The two roles are not interchangeable: the initiator takes a Diffie-
    /// Hellman step immediately and can send, the responder cannot send until
    /// it has heard. Both sides picking the same role is a conversation that
    /// never starts, so the choice has to be one both compute and agree on
    /// without another round trip.
    ///
    /// Layer-1 keys decide it, smaller half first. They are stable — unlike the
    /// transport id, which R0-F2 rotates every twelve minutes and which would
    /// hand the two ends different answers to the same question an hour later —
    /// and both sides hold both keys by the time this runs.
    fn seed_ratchet(
        &self,
        root: &[u8; 32],
        ours_dh: dh::DhSecret,
        theirs_dh: dh::DhPublic,
        their_l1: &sign::PublicKey,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        let our_l1 = self
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layer1_public();
        let ours_pub = ours_dh.public();
        let first = speaks_first((&our_l1, &ours_pub), (their_l1, &theirs_dh))
            .ok_or("paired with something holding our own identity and our own key")?;
        let ratchet = if first {
            Ratchet::initiator(*root, theirs_dh).map_err(|e| e.to_string())?
        } else {
            Ratchet::responder(*root, ours_dh)
        };
        Ok(ratchet.to_state())
    }

    /// This device's Layer-1 public key.
    ///
    /// Public so the contract tests can work out which side of a pairing the
    /// ceremony will make the initiator — `speaks_first` decides it by
    /// comparing these, and a test that cannot compute the answer can only
    /// cover one of the two arrangements at random.
    ///
    /// A public key, and one that goes on the wire inside every ceremony this
    /// device runs, so nothing is disclosed by having it here that pairing does
    /// not already disclose to the person being paired with.
    pub fn layer1_public(&self) -> sign::PublicKey {
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layer1_public()
    }

    /// Take the ratchet a just-completed pairing left, if there is one.
    ///
    /// Taken rather than read: it belongs in the store from here on, and a copy
    /// left in memory is a copy that outlives the write.
    pub fn take_pairing_ratchet(&self, peer: &str) -> Option<Zeroizing<Vec<u8>>> {
        self.pairing_ratchets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
    }

    /// One wording for every ceremony failure the peer can cause.
    ///
    /// The distinctions are real — a substituted code, a bad proof, a message
    /// out of order — and none of them is something the person holding the
    /// phone can act on differently. Reporting them separately would also tell
    /// a prober which of its guesses was closer, so the detail goes to the log
    /// and the screen gets one sentence.
    fn pairing_failed(peer: &str, why: &CeremonyError) -> NetEvent {
        log::warn!("ceremony with {peer} failed: {why}");
        NetEvent::PairingFailed {
            peer: peer.to_string(),
            why: "pairing did not complete".to_string(),
        }
    }

    /// Send an already-encoded [`ChatEnvelope`](crate::session::chat::ChatEnvelope).
    ///
    /// Takes bytes rather than text so the numbering stays where it is decided.
    /// A `&str` here would mean this layer inventing a `seq`, which is the
    /// sender's business and has to survive a restart.
    pub fn send_chat(&self, peer: &str, envelope: Vec<u8>, now: Instant) -> Result<(), SendError> {
        self.send_frame(peer, FrameKind::Chat, envelope, now)
    }

    /// Send the frame that opens a paired thread's other end.
    ///
    /// `body` is a ratchet header followed by a sealed empty plaintext — the
    /// engine's business, since the ratchet lives in the store. This layer only
    /// knows it is not a chat line and must not arrive as one.
    pub fn send_opening(&self, peer: &str, body: Vec<u8>, now: Instant) -> Result<(), SendError> {
        self.send_frame(peer, FrameKind::Opening, body, now)
    }

    /// Say that a chat message has been stored, so the sender's row can stop
    /// claiming only that the bytes left.
    ///
    /// `body` is a sealed `msg_id`, and it is sealed for the reason
    /// [`FrameKind::Ack`] gives: an unsealed one is a forgeable delivery claim.
    /// This layer holds no ratchet, so it seals nothing and reads nothing — it
    /// carries what the engine hands it, exactly as it does for a chat line.
    pub fn send_ack(&self, peer: &str, body: Vec<u8>, now: Instant) -> Result<(), SendError> {
        self.send_frame(peer, FrameKind::Ack, body, now)
    }

    /// One turn of the engine's clock: drop sessions that have gone quiet, then
    /// rotate the advertised id if it is due.
    ///
    /// Everything else in the engine runs because the transport said something.
    /// These two do not — they come due while nothing at all is happening, which
    /// is exactly when no transport event will arrive to carry them. Without a
    /// caller they are unreachable, and both were: `Discovery::tick` and
    /// `SessionTable::sweep` were each written, documented and tested, and
    /// neither had ever run outside a test. The id did not rotate on any real
    /// device, and no session ever expired.
    ///
    /// The order is deliberate. A rotation is refused while a pipe is open
    /// (T08 rule 4), so hanging up on idle peers first is what makes a rotation
    /// possible at all — a device left alone would otherwise hold both its last
    /// pipe and its last id indefinitely, which is the linkable state F2 exists
    /// to prevent. Whether the hang-up lands in time to help *this* turn is up
    /// to the rung: loopback disconnects synchronously, a radio does not. Either
    /// is fine, because a refused rotation is retried on the next turn rather
    /// than reported.
    pub fn tick(&self, now: Instant) -> Vec<NetEvent> {
        let mut out = self.sweep_ceremonies(now);
        out.extend(self.sweep_sessions(now));
        if let Err(why) = self.discovery.tick(now) {
            log::warn!("could not rotate the advertised id: {why}");
        }
        self.check_on_quiet_peers(now);
        out
    }

    /// Ask the rung about anyone it has not mentioned lately.
    ///
    /// A peer that leaves without saying goodbye — out of range, flat battery,
    /// interface gone — sends nothing, so the rung finds out by asking or by
    /// waiting. Waiting is what the LAN rung did, and it costs about two
    /// minutes against BLE's fifteen seconds, because mDNS re-resolves a
    /// stationary peer only every ~98 s (§5.0.21).
    ///
    /// This does not decide anyone has gone. It asks, and the rung answers the
    /// only way it ever does, with `PeerLost` or a fresh sighting. A rung with
    /// nothing to ask ignores it, which is why `verify_peer` has a default.
    ///
    /// Self-limiting, and measured to be: a probe that finds the peer alive
    /// comes back as a sighting, which refreshes its last-seen and takes it off
    /// this list until it goes quiet again. Twelve probes of a live peer
    /// produced sixteen sightings and no removals.
    ///
    /// Only while Discovery is on. The point of noticing quickly is a list
    /// somebody is looking at; with it off there is no list, and probing the
    /// neighbourhood on a timer would be traffic for nobody.
    fn check_on_quiet_peers(&self, now: Instant) {
        if !self.discovery.is_on() {
            return;
        }
        for peer in self.discovery.unheard_from(now, QUIET_BEFORE_ASKING) {
            self.transport.verify_peer(&peer);
        }
    }

    /// Drop sessions idle past the timeout, and hang up on them.
    ///
    /// Hanging up is the point, not the freed memory. An idle pipe holds an LE
    /// connection slot, and those come from a fixed pool shared by every app on
    /// the phone — keeping one for a conversation that ended is precisely how
    /// the next dial finds none free, which is the failure that cost this
    /// project six hardware runs to diagnose.
    fn sweep_sessions(&self, now: Instant) -> Vec<NetEvent> {
        self.sessions
            .sweep(now)
            .into_iter()
            .map(|peer| {
                if let Err(why) = self.transport.disconnect(&peer) {
                    // Not the benign "there was nothing to close" case — the
                    // contract makes closing an absent pipe an `Ok`. So an error
                    // here means the hang-up genuinely failed and the connection
                    // slot may still be held, which is the half of this sweep
                    // that matters.
                    log::warn!("could not hang up on idle {peer}, slot may still be held: {why}");
                }
                log::info!("session with {peer} dropped: idle too long");
                NetEvent::SessionClosed { peer }
            })
            .collect()
    }

    fn send_frame(
        &self,
        peer: &str,
        kind: FrameKind,
        payload: Vec<u8>,
        now: Instant,
    ) -> Result<(), SendError> {
        if !self.sessions.is_open(peer) {
            return Err(SendError::NoSession);
        }
        let frame = Frame::new(kind, payload).map_err(|e| SendError::Refused(e.to_string()))?;
        let sealed = self
            .sessions
            .seal(peer, &frame, now)
            .map_err(|e| SendError::Refused(e.to_string()))?;
        self.send_on(peer, CHANNEL_SESSION, &sealed)
            .map_err(|e| SendError::Refused(e.to_string()))
    }

    fn send_on(&self, peer: &str, channel: u8, payload: &[u8]) -> Result<(), TransportError> {
        let framed =
            pipe::encode(channel, payload).map_err(|e| TransportError::Io(e.to_string()))?;
        self.transport.send(peer, &framed)
    }

    /// Feed one transport event in and take whatever the engine should act on.
    pub fn handle(&self, event: TransportEvent, now: Instant) -> Vec<NetEvent> {
        // Discovery keeps its own view of sightings and answers the persona
        // endpoint; this call is what drives both. It also says whether the
        // nearby list moved, which is not the same question as whether an
        // event arrived.
        let list_moved = self.discovery.on_event(event.clone(), now);

        match event {
            TransportEvent::PeerFound { .. } | TransportEvent::PeerLost { .. } => {
                // Only when something actually changed. A rung may report the
                // same peer many times over — mDNS resolves once per interface
                // and address, sixteen times in a second for one peer on this
                // machine — and telling the UI to rebuild an identical list
                // that often is work nobody asked for, on a device whose
                // battery budget is a Ring 0 requirement.
                if list_moved {
                    vec![NetEvent::PeersChanged]
                } else {
                    Vec::new()
                }
            }
            TransportEvent::PipeOpened { peer } => self.on_pipe_opened(&peer),
            // Split so the reason can be logged. Every rung's failures converge
            // here, which is why the logging belongs here: instrumenting the
            // LAN dial alone left BLE's failures invisible — the same blindness
            // that cost four hardware sessions, moved down one layer.
            TransportEvent::PipeClosed { peer } => {
                log::info!("pipe to {peer} closed");
                self.on_pipe_gone(peer)
            }
            TransportEvent::PipeFailed { peer, why } => {
                log::warn!("pipe to {peer} failed: {why}");
                self.on_pipe_gone(peer)
            }
            TransportEvent::Received { peer, bytes } => self.on_stream(&peer, &bytes, now),
            // Both, and in this order. The reason is what the screen needs;
            // the list still has to be redrawn, because a radio going down
            // takes every peer with it. Folding the two together is what threw
            // the reason away — `TransportEvent::Availability` has always
            // carried it, and nothing downstream ever saw it.
            TransportEvent::Availability { available, reason } => {
                log::info!("{}", radio_log(available, reason.as_deref()));
                vec![
                    NetEvent::RadioChanged { available, reason },
                    NetEvent::PeersChanged,
                ]
            }
        }
    }

    /// A pipe ended, however it ended.
    fn on_pipe_gone(&self, peer: String) -> Vec<NetEvent> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer);
        // A Ping waiting on a pipe that will never open. Reported here as soon
        // as we know, rather than waiting out the deadline — this is the case
        // where the answer is already certain.
        let dropped_ping = self
            .pending_pings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer)
            .is_some();
        self.readers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer);
        // A ceremony cannot outlive the pipe it was running over: its Noise
        // state is tied to a channel that no longer exists, so every later
        // message would fail anyway. Reported rather than dropped quietly,
        // because on this side a person is looking at a screen waiting for the
        // other one to confirm.
        let dropped_ceremony = self.forget_ceremony(&peer);
        let mut out = Vec::new();
        if dropped_ping {
            out.push(NetEvent::PingUndeliverable {
                peer: peer.clone(),
                why: "could not reach that device".into(),
            });
        }
        if dropped_ceremony {
            out.push(NetEvent::PairingFailed {
                peer: peer.clone(),
                why: "pairing did not complete".into(),
            });
        }
        if self.sessions.close(&peer) {
            out.push(NetEvent::SessionClosed { peer });
        }
        out
    }

    /// A pipe opened. If we know who they are we can start a handshake; if not
    /// we must ask first, because Noise IK needs their static up front.
    fn on_pipe_opened(&self, peer: &str) -> Vec<NetEvent> {
        // `PipeOpened` is not once-per-pipe. T08 rule 2 requires the rung to
        // emit it for a `connect` to a peer that is *already* connected, so a
        // caller waiting on the event never hangs — which means every extra tap
        // of Ping delivers another one, and this must be idempotent.
        //
        // It was not, and the cost was total. A second `start_handshake`
        // overwrites the first initiator in `pending`, so the reply to msg1 #1
        // gets decrypted against msg1 #2's ephemeral key — `Noise("decrypt
        // error")` — while the far side, which established a session from msg1
        // #1, reads msg1 #2 as ciphertext and drops that session. Both ends
        // destroy what they had just built, and the harder the person taps the
        // more reliably it fails.
        if self.sessions.is_open(peer) {
            return Vec::new();
        }
        if self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(peer)
        {
            return Vec::new();
        }
        // The other side will speak first; anything we sent now would collide
        // with it (see the module docs on who initiates).
        if !self.we_initiate(peer) {
            log::info!("pipe open to {peer}: they initiate, waiting");
            return Vec::new();
        }
        let persona = self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned();
        match persona {
            Some(persona) => {
                log::info!("pipe open to {peer}: persona known, starting handshake");
                self.start_handshake(peer, &persona)
            }
            None => {
                // First contact: ask for their persona. We cannot present a
                // pseudonym yet — it is derived from *their* Layer-2 key, which
                // is exactly what we are asking for.
                let sent =
                    self.send_on(peer, CHANNEL_DISCOVERY, &Request::first_contact().encode());
                log::info!("pipe open to {peer}: asking for persona (sent={sent:?})");
                Vec::new()
            }
        }
    }

    /// Seed a peer's persona, skipping the discovery fetch.
    ///
    /// For tests that need this side holding a pending handshake without a
    /// second `Net` on the other end. The version gate is the case that needs
    /// it: both `Net`s in one process compile the same `PROTOCOL_VERSION`, so a
    /// disagreement can only come from a hand-rolled peer, and that peer has no
    /// persona endpoint to fetch from.
    #[cfg(test)]
    pub fn learn_persona_for_test(&self, peer: &str, persona: VerifiedPersona) {
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer.to_string(), persona);
    }

    /// Open a handshake — **at most one at a time per peer**.
    ///
    /// The guard lives here rather than only at the call sites because there
    /// are two routes in: a pipe that opens when the persona is already known,
    /// and a persona arriving for a pipe that opened earlier. A duplicate
    /// `PipeOpened` reaches the second route with `pending` still empty (we are
    /// waiting on the persona, not on a reply), so guarding the caller alone
    /// misses exactly the case hardware hit.
    fn start_handshake(&self, peer: &str, persona: &VerifiedPersona) -> Vec<NetEvent> {
        if self.sessions.is_open(peer)
            || self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(peer)
        {
            return Vec::new();
        }
        let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        match Initiator::start(&identity, persona) {
            Ok((initiator, msg1)) => {
                drop(identity);
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.to_string(), initiator);
                let sent = self.send_on(peer, CHANNEL_SESSION, &msg1);
                log::info!(
                    "handshake to {peer}: sent msg1 ({} bytes, {sent:?})",
                    msg1.len()
                );
            }
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(peer);
            }
        }
        Vec::new()
    }

    /// Reassemble, then route by channel. Nothing here decides what a payload
    /// is by looking at it — that is what `engine::pipe` exists to make
    /// unnecessary.
    fn on_stream(&self, peer: &str, bytes: &[u8], now: Instant) -> Vec<NetEvent> {
        let frames = {
            let mut readers = self.readers.lock().unwrap_or_else(|e| e.into_inner());
            let reader = readers.entry(peer.to_string()).or_default();
            reader.push(bytes);
            let mut frames = Vec::new();
            loop {
                match reader.next_frame() {
                    Ok(Some(f)) => frames.push(f),
                    Ok(None) => break,
                    // Unrecoverable: there is no resynchronisation marker, so
                    // the pipe goes rather than being guessed at.
                    Err(_) => {
                        readers.remove(peer);
                        let _ = self.transport.disconnect(peer);
                        return Vec::new();
                    }
                }
            }
            frames
        };

        let mut out = Vec::new();
        for frame in frames {
            match frame.channel {
                CHANNEL_DISCOVERY => out.extend(self.on_discovery(peer, &frame.payload, now)),
                CHANNEL_SESSION => out.extend(self.on_session(peer, &frame.payload, now)),
                // A channel this build does not know: ignored, so a peer on a
                // newer build cannot end the pipe by using one.
                _ => {}
            }
        }
        out
    }

    /// A discovery request to answer, or the persona we asked for.
    fn on_discovery(&self, peer: &str, payload: &[u8], now: Instant) -> Vec<NetEvent> {
        // A response to our own request?
        if let Ok(Some(Response::Persona(record))) = Response::decode(payload) {
            if let Ok(persona) = crate::identity::verify_persona_record(&record) {
                self.known
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.to_string(), persona.clone());
                let _ = self.discovery.accept_persona(peer, &record);
                // Deliberately not the name. A persona name is stable where the
                // device id is not, so a log carrying both is a rotation-proof
                // link between them — the exact correlation R0-F2 exists to
                // deny — and it survives in a bug report long after the id it
                // was paired with has gone.
                log::info!("learned persona of {peer}");
                let mut out = self.start_handshake(peer, &persona);
                out.push(NetEvent::PeersChanged);
                return out;
            }
        }
        // Otherwise it is a request for ours. Silence is a `None`, and the
        // caller cannot tell which refusal produced it.
        if let Some(reply) = self.discovery.answer(peer, payload, now) {
            let _ = self.send_on(peer, CHANNEL_DISCOVERY, &reply);
        }
        Vec::new()
    }

    fn on_session(&self, peer: &str, bytes: &[u8], now: Instant) -> Vec<NetEvent> {
        log::info!("session bytes from {peer}: {} bytes", bytes.len());
        // 1. An established session: ordinary traffic.
        if self.sessions.is_open(peer) {
            return self.on_session_bytes(peer, bytes, now);
        }
        // 2. A handshake we started: this should be the reply.
        let waiting = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer);
        if let Some(initiator) = waiting {
            return match initiator.finish(bytes) {
                Ok(established) => self.adopt(peer, established, now),
                Err(HandshakeError::VersionMismatch { theirs, ours }) => {
                    log::warn!("{peer} speaks protocol version {theirs}, we speak {ours}");
                    vec![NetEvent::SessionRefused {
                        peer: peer.to_string(),
                        their_version: theirs,
                        our_version: ours,
                    }]
                }
                Err(e) => {
                    log::warn!("handshake reply from {peer} rejected: {e:?}");
                    Vec::new()
                }
            };
        }
        // 3. Otherwise: someone opening a handshake with us.
        self.on_handshake_offer(peer, bytes, now)
    }

    /// Someone is dialling us. This is where a block is enforced, and the whole
    /// enforcement is *not answering* — see `session::handshake` for why the
    /// type makes that the only option.
    fn on_handshake_offer(&self, peer: &str, bytes: &[u8], now: Instant) -> Vec<NetEvent> {
        let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        let pending = match Responder::read_first(&identity, bytes) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("handshake offer from {peer} rejected: {e:?}");
                return Vec::new();
            }
        };

        // Both handles. The static is the strong one — it is their pseudonym
        // toward us, proven — and the persona key catches a block that was
        // written before this device had ever dialled us and so had only the
        // weaker handle to bind to.
        let handles = [pending.pseudonym().0, pending.persona().l2_pub.0];
        if self.blocked.ingress_gate(&handles) == Admit::Silence {
            // Keep what this dial just proved. Half of all blocks are bound to
            // a Layer-2 persona key — the durable pseudonym is only learnable
            // from a handshake the peer opened — and R0-F10 says a block must
            // survive that key being regenerated. This is the one moment the
            // durable value is in our hands, and `read_first` has produced no
            // bytes, so keeping it costs nothing and changes nothing on the
            // wire. See T18e.
            self.blocked.note_proved(&handles, pending.pseudonym().0);
            // Dropped. Not an error frame, not a close — nothing, so a blocked
            // device cannot tell this from us being out of range (R0-F10).
            // Not logged, and that is the point. A block is enforced by
            // silence, and nothing on the wire reveals it (R0-F10) — so a log
            // line saying so would be the only artifact in existence that does.
            // Whoever reads the log is not always the person who wrote the
            // block.
            return Vec::new();
        }

        match pending.accept(&identity) {
            Ok((established, reply)) => {
                drop(identity);
                let _ = self.send_on(peer, CHANNEL_SESSION, &reply);
                self.adopt(peer, established, now)
            }
            Err(e) => {
                log::warn!("handshake offer from {peer} accepted then failed: {e:?}");
                Vec::new()
            }
        }
    }

    fn adopt(&self, peer: &str, established: Established, now: Instant) -> Vec<NetEvent> {
        log::info!("session open with {peer}");
        let name = established.persona.name.clone();
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer.to_string(), established.persona.clone());
        // The nearby list reads names off the *sighting*, and only the
        // initiator ever fetches a persona over the discovery channel. Without
        // this the responder — whichever side drew the larger id this time —
        // shows a live, session-bearing peer as a nameless tile for as long as
        // the session lasts.
        self.discovery
            .note_persona(peer, established.persona.clone());
        self.sessions.open(peer, established, now);
        // Anything asked for before the session existed goes now.
        if self
            .pending_pings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .is_some()
        {
            let _ = self.send_frame(peer, FrameKind::Ping, Vec::new(), now);
        }
        // In order. A ceremony's messages are a sequence, and Noise refuses one
        // that arrives out of turn, so a queue flushed in any other order ends
        // the ceremony rather than delivering it late.
        let queued = self
            .pending_ceremony
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .unwrap_or_default();
        for message in queued {
            if let Err(why) = self.send_frame(peer, FrameKind::Ceremony, message, now) {
                log::warn!("could not send a queued ceremony message to {peer}: {why}");
            }
        }
        vec![
            NetEvent::SessionOpened {
                peer: peer.to_string(),
                persona_name: name,
            },
            NetEvent::PeersChanged,
        ]
    }

    fn on_session_bytes(&self, peer: &str, bytes: &[u8], now: Instant) -> Vec<NetEvent> {
        // §12's third surface. A block written while a session is live has to
        // stop the bytes already in flight, and tearing the session down cannot
        // do it alone: teardown and delivery race, and the pump thread may
        // already be holding a frame. Gating here means the race has no wrong
        // outcome — and means a frame kind added later inherits the enforcement
        // without its author having to know that it should.
        //
        // The pseudonym is the one the Noise handshake proved, not one this
        // peer claimed; there is nothing else in a session to key on.
        //
        // Both handles, and the second is not belt-and-braces here. In Noise IK
        // the remote static is whatever the *other* side presented, so it is
        // their pseudonym only when they dialled us; when we dialled, it is the
        // `session_pub` out of their persona record, which `Initiator::finish`
        // asserts. Which role we take is a comparison of two rotating ids, so
        // for one person it flips every twelve minutes — and a gate asking only
        // for a pseudonym would admit a blocked device half the time.
        if let Some(who) = self.sessions.handles(peer) {
            if self.blocked.ingress_gate(&who) == Admit::Silence {
                // Nothing, and no log line — the same silence as the handshake,
                // for the same reason.
                return Vec::new();
            }
        }
        let frames = match self.sessions.open_frames(peer, bytes, now) {
            Ok(f) => f,
            // The session is already dropped by the table; tell the engine so
            // the UI does not keep showing a thread that cannot receive.
            Err(_) => {
                return vec![NetEvent::SessionClosed {
                    peer: peer.to_string(),
                }]
            }
        };

        let name = self
            .sessions
            .persona(peer)
            .map(|p| p.name)
            .unwrap_or_default();
        let mut out = Vec::new();
        for frame in frames {
            match frame.kind {
                FrameKind::Ping => {
                    // Answered before it is reported, so a peer waiting on the
                    // other end is not held up by anything the UI does with it.
                    // Best effort: a Pong that cannot go out leaves the sender
                    // to time out, which is what happened to every ping before
                    // there was a Pong at all — no worse, and never a reason to
                    // drop the nudge we did receive.
                    if let Err(why) = self.send_frame(peer, FrameKind::Pong, Vec::new(), now) {
                        log::warn!("could not answer {peer}'s ping: {why}");
                    }
                    out.push(NetEvent::Pinged {
                        peer: peer.to_string(),
                        persona_name: name.clone(),
                    });
                }
                // Never answered — see `FrameKind::Pong`. Two devices that each
                // replied to the other's answer would nudge forever.
                FrameKind::Pong => {
                    out.push(NetEvent::PingAcked {
                        peer: peer.to_string(),
                    });
                }
                // Passed on sealed. This layer has no ratchet and so no way to
                // read a chat frame, and no business forming an opinion about
                // one it cannot read — see [`NetEvent::ChatReceived`].
                FrameKind::Chat => out.push(NetEvent::ChatReceived {
                    peer: peer.to_string(),
                    body: frame.payload.clone(),
                }),
                FrameKind::Opening => out.push(NetEvent::ChainOpening {
                    peer: peer.to_string(),
                    body: frame.payload.clone(),
                }),
                // Sealed, like a chat line, and passed on unread for the same
                // reason: the ratchet that opens it lives in the store.
                FrameKind::Ack => out.push(NetEvent::ChatAcked {
                    peer: peer.to_string(),
                    body: frame.payload.clone(),
                }),
                FrameKind::Ceremony => {
                    out.extend(self.on_ceremony_frame(peer, &frame.payload, now))
                }
                FrameKind::DropControl => {}
            }
        }
        out
    }

    /// The remote static a live session's handshake proved.
    ///
    /// Called `pseudonym` until T18b, which is what it is only when the peer
    /// dialled us. See [`SessionTable::remote_static`], and use
    /// [`Net::session_handles`] for anything deciding about a person.
    pub fn remote_static(&self, peer: &str) -> Option<dh::DhPublic> {
        self.sessions.remote_static(peer)
    }

    /// Everything a live session knows that could identify its peer to the
    /// block list.
    pub fn session_handles(&self, peer: &str) -> Option<[[u8; 32]; 2]> {
        self.sessions.handles(peer)
    }

    /// The peer's pseudonym toward us, where a live session proves one — which
    /// it does only if they dialled. See [`SessionTable::proved_pseudonym`].
    pub fn proved_pseudonym(&self, peer: &str) -> Option<[u8; 32]> {
        self.sessions.proved_pseudonym(peer)
    }
}

/// Which side of a pairing takes the ratchet's initiator role.
///
/// A free function, and tested as one, because the two ends of a pairing can
/// never be stood up together in a test — `CORE` is process-wide — so the only
/// way to check that they *disagree* is to ask the rule directly.
///
/// The property is antisymmetry: given the same two keys, exactly one side
/// answers yes. Both saying no is a conversation neither can start; both saying
/// yes is two sending chains that cannot read each other. Neither failure says
/// anything on screen — the messages simply never open.
/// `None` when the two ends are indistinguishable — see below.
fn speaks_first(
    ours: (&sign::PublicKey, &dh::DhPublic),
    theirs: (&sign::PublicKey, &dh::DhPublic),
) -> Option<bool> {
    match ours.0 .0.cmp(&theirs.0 .0) {
        Ordering::Less => Some(true),
        Ordering::Greater => Some(false),
        // Two devices wearing the same Layer-1 identity. Unreachable between
        // honest strangers and not forgeable — a Layer-1 proof is signed over
        // the *peer's* Layer-2 key, so ours cannot be reflected back at us —
        // but an identity restored onto a second device would land here, and
        // the failure is the silent one: both sides answer no, both become
        // responders, and neither can ever speak.
        //
        // The ephemeral ratchet keys break it. They are fresh per ceremony and
        // differ with overwhelming probability, so this is a tie-break that
        // resolves rather than a second coin with the same bias.
        Ordering::Equal => match ours.1 .0.cmp(&theirs.1 .0) {
            Ordering::Less => Some(true),
            Ordering::Greater => Some(false),
            // Same identity *and* same ephemeral key: one device on both ends
            // of one ceremony. Refused rather than given a role, because there
            // is no answer that makes a conversation work.
            Ordering::Equal => None,
        },
    }
}

/// What to log for a radio report.
///
/// `available` decides, never `reason`. Keying off the reason instead reads an
/// unavailable radio with nothing to say as "radio available" — writing the
/// opposite of the truth into the one place a diagnosis starts from. That is
/// the availability lie again, in the log this time, and it went in one file
/// away from the Dart handler that states the rule correctly.
fn radio_log(available: bool, reason: Option<&str>) -> String {
    match (available, reason) {
        (true, _) => "radio available".to_string(),
        (false, Some(why)) => format!("radio unavailable: {why}"),
        (false, None) => "radio unavailable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{dh, sign};

    /// A peer on another protocol version is refused, and the dialling side is
    /// told why.
    ///
    /// The reason the version went in the intro payload rather than the Noise
    /// prologue is that a mismatch can *name itself*. A prologue refuses just
    /// as safely and leaves the user staring at somebody who never connects —
    /// so the event is the feature, and this is the test of it.
    ///
    /// The far side is a hand-rolled Noise responder, because both `Net`s in
    /// this process compile the same `PROTOCOL_VERSION` and a disagreement can
    /// only be built by hand. Its intro is a single version byte and nothing
    /// else: `decode_intro` reads the version before it believes any length, so
    /// there is deliberately nothing behind it to believe.
    #[test]
    fn a_peer_on_another_protocol_version_is_refused_with_a_reason() {
        use crate::identity::Identity;
        use crate::session::handshake::{Initiator, MAX_NOISE_MESSAGE, NOISE_PARAMS};
        use snow::Builder;

        let alice = Identity::generate("Alice", 1);
        let bob = Identity::generate("Bob", 2);
        let bob_persona = crate::identity::verify_persona_record(&bob.persona_record()).unwrap();

        // Alice dials, and holds the initiator half.
        let (initiator, first) = Initiator::start(&alice, &bob_persona).unwrap();

        // Bob's build, from another version. Same keys, same pattern — only the
        // number in the intro differs.
        let secret = bob.session_secret();
        let mut state = Builder::new(NOISE_PARAMS.parse().unwrap())
            .local_private_key(&*secret.expose_secret())
            .unwrap()
            .build_responder()
            .unwrap();
        let mut scratch = vec![0u8; MAX_NOISE_MESSAGE];
        state.read_message(&first, &mut scratch).unwrap();
        let mut reply = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state.write_message(&[200u8], &mut reply).unwrap();
        reply.truncate(n);

        // The detection.
        assert!(
            matches!(
                initiator.finish(&reply),
                Err(HandshakeError::VersionMismatch {
                    theirs: 200,
                    ours: 2
                })
            ),
            "the dialling side did not refuse a foreign version"
        );

        // And the report, through the real path: a `Net` holding this pending
        // handshake, handed these bytes. Asserting on a helper that rebuilt the
        // event would only prove the helper works — the arm in `on_session` is
        // what has to fire, and nothing else in the handshake path raises an
        // event on failure, so dropping it is otherwise invisible.
        let air = crate::transport::loopback::LoopbackNet::new();
        let transport: Arc<dyn Transport> = Arc::new(air.join("alice", Box::new(|_| {})));
        let net = Net::new(
            transport,
            Arc::new(Mutex::new(alice)),
            Arc::new(Blocklist::default()),
            "alice",
            Instant::now(),
        );
        // This Net's own dial, so the reply below is the one it is waiting for.
        let (pending, first) = {
            let id = net.identity.lock().unwrap_or_else(|e| e.into_inner());
            Initiator::start(&id, &bob_persona).unwrap()
        };
        let secret = bob.session_secret();
        let mut state = Builder::new(NOISE_PARAMS.parse().unwrap())
            .local_private_key(&*secret.expose_secret())
            .unwrap()
            .build_responder()
            .unwrap();
        let mut scratch = vec![0u8; MAX_NOISE_MESSAGE];
        state.read_message(&first, &mut scratch).unwrap();
        let mut reply = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state.write_message(&[200u8], &mut reply).unwrap();
        reply.truncate(n);

        net.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("bob".to_string(), pending);
        let out = net.on_session("bob", &reply, Instant::now());
        assert!(
            matches!(
                out.as_slice(),
                [NetEvent::SessionRefused {
                    their_version: 200,
                    our_version: 2,
                    ..
                }]
            ),
            "the dialling side was not told why; got {out:?}"
        );
    }

    /// Exactly one side speaks first, whichever way round you ask.
    ///
    /// The failure this rules out is silent on both phones: two responders
    /// never start, two initiators never understand each other, and in neither
    /// case does anything appear on a screen. Asserted over both orderings
    /// rather than one, because a rule that answered the same way twice would
    /// pass a single-direction check.
    #[test]
    fn exactly_one_side_of_a_pairing_speaks_first() {
        let dh_a = dh::DhPublic([3u8; 32]);
        let dh_b = dh::DhPublic([4u8; 32]);
        let small = sign::PublicKey([1u8; 32]);
        let large = sign::PublicKey([2u8; 32]);
        assert_eq!(speaks_first((&small, &dh_a), (&large, &dh_b)), Some(true));
        assert_eq!(speaks_first((&large, &dh_b), (&small, &dh_a)), Some(false));

        // Differing in the last byte only: a rule comparing the wrong end, or
        // comparing lengths, would agree with itself here.
        let mut a = [7u8; 32];
        let mut b = [7u8; 32];
        a[31] = 0;
        b[31] = 9;
        let (a, b) = (sign::PublicKey(a), sign::PublicKey(b));
        assert_ne!(
            speaks_first((&a, &dh_a), (&b, &dh_b)),
            speaks_first((&b, &dh_b), (&a, &dh_a)),
            "both ends of a pairing took the same role"
        );
    }

    /// One identity on two devices still gets two roles.
    ///
    /// Raised in review, and the failure it prevents is the silent one: with
    /// Layer-1 keys alone, equal identities make *both* ends responders and the
    /// conversation simply never starts. Not reachable between honest strangers
    /// — a Layer-1 proof is signed over the peer's Layer-2 key, so ours cannot
    /// be reflected at us — but an identity restored onto a second device would
    /// land here, and nothing would say so.
    #[test]
    fn the_same_identity_on_both_ends_is_still_split_by_the_ephemeral_keys() {
        let same = sign::PublicKey([5u8; 32]);
        let mine = dh::DhPublic([1u8; 32]);
        let yours = dh::DhPublic([2u8; 32]);

        assert_eq!(speaks_first((&same, &mine), (&same, &yours)), Some(true));
        assert_eq!(speaks_first((&same, &yours), (&same, &mine)), Some(false));

        // Same identity *and* the same ephemeral key is one device on both ends
        // of one ceremony. There is no role that makes that work, so it is
        // refused rather than answered.
        assert_eq!(speaks_first((&same, &mine), (&same, &mine)), None);
    }

    #[test]
    fn availability_is_read_from_the_flag_and_never_from_the_reason() {
        // The case that was wrong: unavailable, with nothing to say about it.
        assert_eq!(radio_log(false, None), "radio unavailable");
        // And its mirror, which a reason-keyed check also gets backwards.
        assert_eq!(radio_log(true, Some("stale")), "radio available");
    }

    #[test]
    fn a_reason_is_carried_when_there_is_one() {
        assert_eq!(
            radio_log(false, Some("Bluetooth is off")),
            "radio unavailable: Bluetooth is off"
        );
    }
}
