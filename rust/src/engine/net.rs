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
    /// A chat line arrived.
    ChatReceived { peer: PeerId, text: String },
    /// A queued Ping was dropped because the pipe never opened.
    PingUndeliverable { peer: PeerId, why: String },
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
            TransportEvent::PipeClosed { peer } | TransportEvent::PipeFailed { peer, .. } => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer);
                // A Ping waiting on a pipe that will never open. Reported here
                // as soon as we know, rather than waiting out the deadline —
                // this is the case where the answer is already certain.
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
            TransportEvent::Received { peer, bytes } => self.on_stream(&peer, &bytes, now),
            TransportEvent::Availability { .. } => vec![NetEvent::PeersChanged],
        }
    }

    /// A pipe opened. If we know who they are we can start a handshake; if not
    /// we must ask first, because Noise IK needs their static up front.
    fn on_pipe_opened(&self, peer: &str) -> Vec<NetEvent> {
        // The other side will speak first; anything we sent now would collide
        // with it (see the module docs on who initiates).
        if !self.we_initiate(peer) {
            return Vec::new();
        }
        let persona = self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned();
        match persona {
            Some(persona) => self.start_handshake(peer, &persona),
            None => {
                // First contact: ask for their persona. We cannot present a
                // pseudonym yet — it is derived from *their* Layer-2 key, which
                // is exactly what we are asking for.
                let _ = self.send_on(peer, CHANNEL_DISCOVERY, &Request::first_contact().encode());
                Vec::new()
            }
        }
    }

    fn start_handshake(&self, peer: &str, persona: &VerifiedPersona) -> Vec<NetEvent> {
        let identity = self.identity.lock().unwrap_or_else(|e| e.into_inner());
        match Initiator::start(&identity, persona) {
            Ok((initiator, msg1)) => {
                drop(identity);
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.to_string(), initiator);
                let _ = self.send_on(peer, CHANNEL_SESSION, &msg1);
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
                Err(_) => Vec::new(),
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
            Err(_) => return Vec::new(),
        };

        if self
            .blocked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&pending.pseudonym().0)
        {
            // Dropped. Not an error frame, not a close — nothing, so a blocked
            // device cannot tell this from us being out of range (R0-F10).
            return Vec::new();
        }

        match pending.accept(&identity) {
            Ok((established, reply)) => {
                drop(identity);
                let _ = self.send_on(peer, CHANNEL_SESSION, &reply);
                self.adopt(peer, established, now)
            }
            Err(_) => Vec::new(),
        }
    }

    fn adopt(&self, peer: &str, established: Established, now: Instant) -> Vec<NetEvent> {
        let name = established.persona.name.clone();
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer.to_string(), established.persona.clone());
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
                    out.push(NetEvent::Pinged {
                        peer: peer.to_string(),
                        persona_name: name.clone(),
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
