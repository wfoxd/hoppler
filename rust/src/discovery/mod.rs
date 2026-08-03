//! The discovery service (T09) — R0-F2, and the endpoint shape R0-F10 rests on.
//!
//! Sits directly on a [`Transport`] rung and answers three questions: who is
//! nearby, who may learn that we are here, and what a requester is told when
//! the answer is no.
//!
//! # Rotation, and what the sighting list may not do
//!
//! We advertise under an ephemeral id that rotates on [`ROTATION_PERIOD`],
//! beneath which the radio rotates its own address. A sighting list that
//! carried a peer across its rotation would undo all of that — so sightings are
//! keyed by the id the rung reported and nothing stitches them together. A peer
//! that rotates *is* a new device here, which is the point.
//!
//! The one case where linking is legitimate is a peer we hold a pipe to, and
//! that case cannot arise: the transport refuses to rotate a local id while any
//! pipe is open (T08 contract rule 4), so a connected peer never rotates
//! underneath us. The guarantee is structural rather than a rule this module
//! has to remember.
//!
//! # Refusal is one path
//!
//! Discovery off, blocked, and rate-limited all produce [`Response::Silence`],
//! and they do so by falling through the *same* return rather than three
//! branches that happen to agree today. F10's indistinguishability is a
//! property of that shape; a future branch that wanted to be helpful — an
//! "unavailable" frame, an error code, a different close — would break it
//! silently, which is why `respond` funnels rather than matches.

pub mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::identity::{self, Identity};
use crate::transport::{PeerId, Transport, TransportError, TransportEvent};
use protocol::{ProtocolError, Request, Response, PSEUDONYM_LEN};

/// How often the advertised id rotates (tech spec §4: 10–15 min, aligned with
/// the radio's own address rotation).
///
/// The T01 spike that was to pin this against observed RPA cadence is parked,
/// so this is the midpoint of the specified range rather than a measured value.
/// Recorded as a decision, not a measurement.
pub const ROTATION_PERIOD: Duration = Duration::from_secs(12 * 60);

/// Requests one pseudonym may make before it is ignored, per window.
const RATE_LIMIT_BURST: u32 = 5;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// A device we can currently see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sighting {
    /// The rung's id for it — ephemeral, and replaced wholesale on rotation.
    pub peer: PeerId,
    /// Their persona, once fetched and signature-checked. `None` until then.
    pub persona: Option<identity::VerifiedPersona>,
}

/// Who a requester turned out to be, for the caller's rate limiting and
/// blocklist. Kept separate from the wire type so the endpoint's decision is
/// testable without a transport.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Requester {
    Pseudonym([u8; PSEUDONYM_LEN]),
    /// First contact: they cannot derive a pseudonym toward us yet, so they are
    /// limited by the connection they arrived on instead. That key rotates with
    /// the peer's id, which is weaker — noted in the findings rather than
    /// pretended otherwise.
    Unknown(PeerId),
}

struct Buckets {
    hits: HashMap<Requester, (u32, Instant)>,
}

impl Buckets {
    /// Returns whether this requester is within its allowance, and charges it.
    fn allow(&mut self, who: &Requester, now: Instant) -> bool {
        let entry = self.hits.entry(who.clone()).or_insert((0, now));
        if now.saturating_duration_since(entry.1) >= RATE_LIMIT_WINDOW {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= RATE_LIMIT_BURST
    }

    /// Drop windows that have expired. Without this the map grows once per
    /// stranger seen, for the life of the process.
    fn sweep(&mut self, now: Instant) {
        self.hits
            .retain(|_, (_, seen)| now.saturating_duration_since(*seen) < RATE_LIMIT_WINDOW);
    }
}

struct Inner {
    transport: Arc<dyn Transport>,
    /// The id we are currently known by. Set at construction and on every
    /// rotation, so anything needing it reads one value rather than keeping a
    /// copy that goes stale twelve minutes later.
    local_id: Mutex<PeerId>,
    identity: Arc<Mutex<Identity>>,
    on: AtomicBool,
    sightings: Mutex<HashMap<PeerId, Sighting>>,
    blocked: Mutex<Vec<[u8; PSEUDONYM_LEN]>>,
    buckets: Mutex<Buckets>,
    last_rotation: Mutex<Instant>,
}

/// The discovery engine.
pub struct Discovery {
    inner: Arc<Inner>,
}

impl Discovery {
    /// The identity is **shared, not copied**. An owned copy would keep serving
    /// the persona as it was at construction, so a rename would change what the
    /// UI shows and not what the endpoint hands out — a divergence with no
    /// symptom on this device and a wrong name on every other one.
    pub fn new(
        transport: Arc<dyn Transport>,
        identity: Arc<Mutex<Identity>>,
        now: Instant,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport,
                local_id: Mutex::new(String::new()),
                identity,
                on: AtomicBool::new(false),
                sightings: Mutex::new(HashMap::new()),
                blocked: Mutex::new(Vec::new()),
                buckets: Mutex::new(Buckets {
                    hits: HashMap::new(),
                }),
                last_rotation: Mutex::new(now),
            }),
        }
    }

    /// How we currently appear to peers.
    ///
    /// The single source of truth: rotation happens here, so anything that
    /// needs our id — the initiator tie-break in `engine::net`, for one — must
    /// read it rather than keep a copy. A second copy would silently go stale
    /// on the next rotation.
    pub fn local_id(&self) -> PeerId {
        self.inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Adopt an id we are already known by, without touching the radio.
    ///
    /// Used at construction, when the transport was built under an id that
    /// discovery did not choose. Rotation proper goes through
    /// [`Self::rotate`], which withdraws the old name first.
    pub fn set_local_id_for_tiebreak(&self, id: &str) {
        *self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = id.to_string();
    }

    pub fn is_on(&self) -> bool {
        self.inner.on.load(Ordering::SeqCst)
    }

    /// Turn Discovery on or off (R0-F2). Off stops advertising *and* disables
    /// the endpoint; both take effect before this returns, which is what makes
    /// the ≤ 5 s acceptance trivially true rather than a timing question.
    ///
    /// Scanning is left running when off: seeing others is not the same as
    /// being seen, and the F2 toggle is about our own visibility.
    pub fn set_enabled(&self, enabled: bool, now: Instant) -> Result<(), TransportError> {
        if enabled {
            self.inner.on.store(true, Ordering::SeqCst);
            *self
                .inner
                .last_rotation
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = now;
            if let Err(e) = self.publish() {
                // Do not claim to be discoverable when the radio refused.
                self.inner.on.store(false, Ordering::SeqCst);
                return Err(e);
            }
        } else {
            // Flag first: the endpoint must already be refusing by the time the
            // radio call returns, not after it.
            self.inner.on.store(false, Ordering::SeqCst);
            self.inner.transport.stop_advertising()?;
        }
        Ok(())
    }

    /// Begin scanning. Independent of [`Self::set_enabled`].
    pub fn start_scanning(&self) -> Result<(), TransportError> {
        self.inner.transport.start_scanning()
    }

    /// What we can currently see.
    pub fn sightings(&self) -> Vec<Sighting> {
        self.inner
            .sightings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Block a pseudonym. Silent and local (R0-F10).
    pub fn block(&self, pseudonym: [u8; PSEUDONYM_LEN]) {
        let mut blocked = self.inner.blocked.lock().unwrap_or_else(|e| e.into_inner());
        if !blocked.contains(&pseudonym) {
            blocked.push(pseudonym);
        }
    }

    /// Rotate the advertised id if it is due. Driven by the caller's clock so
    /// the cadence is testable without waiting twelve minutes.
    pub fn tick(&self, now: Instant) -> Result<(), TransportError> {
        self.inner
            .buckets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sweep(now);
        if !self.is_on() {
            return Ok(());
        }
        let due = {
            let last = self
                .inner
                .last_rotation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            now.saturating_duration_since(*last) >= ROTATION_PERIOD
        };
        if due {
            self.rotate(now)?;
        }
        Ok(())
    }

    /// Rotate now. A rotation while a pipe is open is refused by the rung
    /// (contract rule 4); that is not an error worth surfacing — it means the
    /// device is mid-conversation, and the next tick will try again.
    pub fn rotate(&self, now: Instant) -> Result<(), TransportError> {
        let fresh = ephemeral_id();
        match self.inner.transport.set_local_id(&fresh) {
            Ok(()) => {
                *self
                    .inner
                    .local_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = fresh.clone();
                *self
                    .inner
                    .last_rotation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = now;
                self.publish()
            }
            Err(TransportError::Unavailable(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Advertise. The payload is empty: the persona is disclosed through the
    /// endpoint, to a requester that has identified itself first. Putting it in
    /// the advertisement would hand it to every scanner in range and make the
    /// requester-first ordering decorative.
    fn publish(&self) -> Result<(), TransportError> {
        self.inner.transport.start_advertising(Vec::new())
    }

    /// Feed a transport event in.
    pub fn on_event(&self, event: TransportEvent, _now: Instant) {
        match event {
            TransportEvent::PeerFound { peer, .. } => {
                // Re-sent whenever the advertised payload changes (T08 rule on
                // PeerFound), so this fires repeatedly for a peer we already
                // know. Overwriting would discard a persona we have already
                // fetched and signature-checked, and the UI would watch names
                // blink out for no reason the user could see.
                self.inner
                    .sightings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(peer.clone())
                    .or_insert(Sighting {
                        peer,
                        persona: None,
                    });
            }
            TransportEvent::PeerLost { peer } => {
                self.inner
                    .sightings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer);
            }
            // Bytes are *not* handled here any more. The pipe carries two
            // protocols, and telling them apart by inspecting content cannot be
            // made reliable — see `engine::pipe`. The caller demultiplexes and
            // hands whole requests to `answer`.
            _ => {}
        }
    }

    /// Answer one complete discovery request.
    ///
    /// Returns the bytes to send back, or `None` for silence — which is every
    /// refusal: Discovery off, blocked, rate-limited, or unparseable. The
    /// caller cannot tell them apart and neither can the requester (R0-F10).
    ///
    /// Takes a whole request rather than a stream: framing and reassembly are
    /// the pipe layer's job, and doing it here meant guessing where a request
    /// ended in a stream that also carried session ciphertext.
    pub fn answer(&self, peer: &str, request: &[u8], now: Instant) -> Option<Vec<u8>> {
        let reply = match Request::decode(request) {
            Ok(request) => self.respond(peer, &request, now),
            // A frame we cannot parse earns the same nothing as a refusal; an
            // error reply would distinguish "malformed" from "blocked".
            Err(ProtocolError::Malformed) | Err(ProtocolError::Version(_)) => Response::Silence,
        };
        let wire = reply.encode();
        if wire.is_empty() {
            None
        } else {
            Some(wire)
        }
    }

    /// Decide what a requester is told.
    ///
    /// Every refusal returns the same `Silence` from the same place. Reading
    /// this function, there should be no way to tell from the outside which
    /// condition fired — that indistinguishability is R0-F10.
    fn respond(&self, peer: &str, request: &Request, now: Instant) -> Response {
        let who = if request.is_first_contact() {
            Requester::Unknown(peer.to_string())
        } else {
            Requester::Pseudonym(request.pseudonym)
        };

        // One `Silence` return, reached three ways. Short-circuiting on the
        // toggle first means an off device charges nothing: otherwise requests
        // arriving while off would spend a stranger's allowance and suppress
        // their first legitimate ask moments after Discovery came back on —
        // and the bucket map would grow for a device that is answering nobody.
        //
        // The order is not observable: every path below sends the same nothing,
        // and none of them sends it at a different time.
        let refuse = !self.is_on() || {
            // Rate limit before the blocklist and before any disclosure, so
            // load can be shed without leaking whether the pseudonym is one we
            // recognise.
            let within_allowance = self
                .inner
                .buckets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .allow(&who, now);

            let blocked = !request.is_first_contact()
                && self
                    .inner
                    .blocked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&request.pseudonym);

            !within_allowance || blocked
        };

        if refuse {
            return Response::Silence;
        }

        Response::Persona(
            self.inner
                .identity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .persona_record(),
        )
    }

    /// Record a persona we were sent, if it verifies.
    ///
    /// A record that fails its signature is discarded here and never reaches a
    /// sighting, so it cannot reach the UI — the acceptance is that the check
    /// is not skippable by any path that populates the list.
    pub fn accept_persona(&self, peer: &str, wire: &[u8]) -> Result<(), identity::IdentityError> {
        let verified = identity::verify_persona_record(wire)?;
        self.note_persona(peer, verified);
        Ok(())
    }

    /// Record a persona that was verified somewhere else — in practice the
    /// session handshake.
    ///
    /// Only one side of a pair ever *asks* for a persona: the initiator, which
    /// the tie-break fixes as the smaller id. The responder learns the same
    /// persona from the handshake instead, and had no way to say so, so its
    /// sighting stayed nameless for the life of the session. Deterministically,
    /// not intermittently — the larger id always showed the other as unnamed.
    ///
    /// What arrives here is *better* attested than what `accept_persona`
    /// verifies. A record off the discovery channel proves only that it was
    /// signed; one that came through the handshake is additionally bound to the
    /// static key the peer proved possession of, which is the check that closed
    /// the T10 impersonation hole. So overwriting is an upgrade.
    ///
    /// A peer with no sighting is left alone: nothing has advertised it, so
    /// there is no tile to name.
    pub fn note_persona(&self, peer: &str, persona: identity::VerifiedPersona) {
        let mut sightings = self
            .inner
            .sightings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(sighting) = sightings.get_mut(peer) {
            sighting.persona = Some(persona);
        }
    }
}

/// A fresh advertised id: random, and carrying nothing derived from our keys.
///
/// Anything with structure would survive rotation as a fingerprint, which is
/// the linkage rotation exists to break.
fn ephemeral_id() -> String {
    let mut bytes = [0u8; 8];
    crate::crypto::rng::fill(&mut bytes);
    let mut out = String::with_capacity(16);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}
