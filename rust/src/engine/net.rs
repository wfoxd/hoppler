//! The engine's networking half (T10 part 2b) — discovery, sessions, and the
//! event loop that joins them.
//!
//! Deliberately **not** a singleton, unlike [`super::CORE`]. The engine is
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
use crate::crypto::dh;
use crate::discovery::protocol::{Request, Response};
use crate::discovery::Discovery;
use crate::identity::{Identity, VerifiedPersona};
use crate::session::frame::{Frame, FrameKind};
use crate::session::handshake::{Established, Initiator, Responder};
use crate::session::table::SessionTable;
use crate::transport::{PeerId, Transport, TransportError, TransportEvent};

/// How long a queued Ping waits for a session before it is called undeliverable.
///
/// Long enough for the honest path — dial (3 s in the LAN rung), persona fetch,
/// and an IK handshake — and short enough that a person still connects the
/// answer to the tap that caused it.
pub const PING_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// A chat line arrived.
    ChatReceived { peer: PeerId, text: String },
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
    /// Pseudonyms the user has blocked (R0-F10).
    blocked: Mutex<Vec<[u8; 32]>>,
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
}

impl Net {
    pub fn new(
        transport: Arc<dyn Transport>,
        identity: Arc<Mutex<Identity>>,
        local_id: &str,
        now: Instant,
    ) -> Self {
        let discovery = Discovery::new(transport.clone(), identity.clone(), now);
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
            blocked: Mutex::new(Vec::new()),
            readers: Mutex::new(HashMap::new()),
            pending_pings: Mutex::new(HashMap::new()),
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

    pub fn block(&self, pseudonym: [u8; 32]) {
        self.discovery.block(pseudonym);
        let mut blocked = self.blocked.lock().unwrap_or_else(|e| e.into_inner());
        if !blocked.contains(&pseudonym) {
            blocked.push(pseudonym);
        }
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
            return self.send_frame(peer, FrameKind::Ping, Vec::new(), now);
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

    pub fn send_chat(&self, peer: &str, text: &str, now: Instant) -> Result<(), String> {
        self.send_frame(peer, FrameKind::Chat, text.as_bytes().to_vec(), now)
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
        let closed = self.sweep_sessions(now);
        if let Err(why) = self.discovery.tick(now) {
            log::warn!("could not rotate the advertised id: {why}");
        }
        closed
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
    ) -> Result<(), String> {
        if !self.sessions.is_open(peer) {
            return Err(format!("no session with {peer} yet"));
        }
        let frame = Frame::new(kind, payload).map_err(|e| e.to_string())?;
        let sealed = self
            .sessions
            .seal(peer, &frame, now)
            .map_err(|e| e.to_string())?;
        self.send_on(peer, CHANNEL_SESSION, &sealed)
            .map_err(|e| e.to_string())
    }

    fn send_on(&self, peer: &str, channel: u8, payload: &[u8]) -> Result<(), TransportError> {
        let framed =
            pipe::encode(channel, payload).map_err(|e| TransportError::Io(e.to_string()))?;
        self.transport.send(peer, &framed)
    }

    /// Feed one transport event in and take whatever the engine should act on.
    pub fn handle(&self, event: TransportEvent, now: Instant) -> Vec<NetEvent> {
        // Discovery keeps its own view of sightings and answers the persona
        // endpoint; this call is what drives both.
        self.discovery.on_event(event.clone(), now);

        match event {
            TransportEvent::PeerFound { .. } | TransportEvent::PeerLost { .. } => {
                vec![NetEvent::PeersChanged]
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
        let mut out = Vec::new();
        if dropped_ping {
            out.push(NetEvent::PingUndeliverable {
                peer: peer.clone(),
                why: "could not reach that device".into(),
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

        if self
            .blocked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&pending.pseudonym().0)
        {
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
        vec![
            NetEvent::SessionOpened {
                peer: peer.to_string(),
                persona_name: name,
            },
            NetEvent::PeersChanged,
        ]
    }

    fn on_session_bytes(&self, peer: &str, bytes: &[u8], now: Instant) -> Vec<NetEvent> {
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
                FrameKind::Chat => {
                    // Lossy on purpose: v0 is text, and a peer sending invalid
                    // UTF-8 should not be able to make us drop a session.
                    out.push(NetEvent::ChatReceived {
                        peer: peer.to_string(),
                        text: String::from_utf8_lossy(&frame.payload).into_owned(),
                    });
                }
                FrameKind::DropControl => {}
            }
        }
        out
    }

    /// The pseudonym proven for a peer, if a session is live — what a block
    /// binds to.
    pub fn pseudonym(&self, peer: &str) -> Option<dh::DhPublic> {
        self.sessions.pseudonym(peer)
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
    use super::radio_log;

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
