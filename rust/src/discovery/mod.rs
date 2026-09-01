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

pub mod hint;
pub mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::block::{Admit, Blocklist};
use crate::identity::{self, Identity};
use crate::transport::{PeerId, Transport, TransportError, TransportEvent};
use hint::HINT_LEN;
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
    /// The advert hint they are carrying, if the payload held one (T09a).
    ///
    /// Meaningless on its own — resolving it to a person needs the Layer-1 key
    /// of somebody we have paired with, which lives in the store, not here.
    /// Discovery carries the bytes and forms no opinion about them.
    pub hint: Option<[u8; HINT_LEN]>,
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
    /// When each sighting was last confirmed by the rung.
    ///
    /// Beside the sightings rather than inside `Sighting`, which is public and
    /// travels to the UI — the screen has no use for an `Instant`, and putting
    /// one there would make every consumer carry it.
    last_seen: Mutex<HashMap<PeerId, Instant>>,
    /// Personas learned for a peer, held independently of whether that peer has
    /// a sighting *yet*.
    ///
    /// The two arrive in either order. A peer that dials us is known through the
    /// pipe — hello, persona fetch, handshake — before its advertisement
    /// necessarily reaches us, and storing the persona only onto an existing
    /// sighting silently dropped it in exactly that case. The sighting then
    /// appeared with no name and nothing ever refilled it, which is a nameless
    /// tile above a live session.
    ///
    /// Keyed by the same rotating id as the sightings, so it stitches nothing
    /// together that they do not: a peer that rotates has no entry here under
    /// its new id, and has to be learned again.
    personas: Mutex<HashMap<PeerId, identity::VerifiedPersona>>,
    /// Shared with `engine::net::Net` — one list, not a copy. See
    /// [`crate::block`] for why there is exactly one.
    blocked: Arc<Blocklist>,
    buckets: Mutex<Buckets>,
    last_rotation: Mutex<Instant>,
    /// The payload currently on the air, so [`Discovery::publish`] can tell a
    /// re-advertisement that would change nothing from one that would. `None`
    /// means nothing of ours is being advertised at all, which is not the same
    /// as advertising an empty payload — the first has to reach the radio even
    /// if the bytes match.
    on_the_air: Mutex<Option<Vec<u8>>>,
    /// Wall clock, in milliseconds since the Unix epoch.
    ///
    /// Injected rather than read from `SystemTime` at the point of use, and
    /// separate from the `Instant`s already threaded through here, because the
    /// two answer different questions. An `Instant` measures how long since we
    /// last rotated, which only this device needs to agree with itself about.
    /// The hint's epoch has to be a number *both* devices compute, so it can
    /// only come from a clock they share.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
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
        blocked: Arc<Blocklist>,
        now: Instant,
    ) -> Self {
        Self::with_clock(transport, identity, blocked, now, Box::new(system_millis))
    }

    /// As [`Self::new`], with the wall clock supplied.
    ///
    /// The hint's epoch is a division of real time, so testing what happens on
    /// either side of a boundary means being able to stand on one. The system
    /// clock cannot be asked to do that, and waiting twelve minutes for it is
    /// not a test.
    pub fn with_clock(
        transport: Arc<dyn Transport>,
        identity: Arc<Mutex<Identity>>,
        blocked: Arc<Blocklist>,
        now: Instant,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport,
                local_id: Mutex::new(String::new()),
                identity,
                on: AtomicBool::new(false),
                sightings: Mutex::new(HashMap::new()),
                last_seen: Mutex::new(HashMap::new()),
                personas: Mutex::new(HashMap::new()),
                blocked,
                buckets: Mutex::new(Buckets {
                    hits: HashMap::new(),
                }),
                last_rotation: Mutex::new(now),
                on_the_air: Mutex::new(None),
                clock,
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
            // Nothing of ours is on the air, so the next `publish` must reach
            // the radio however little the hint has changed meanwhile.
            *self
                .inner
                .on_the_air
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
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

    /// Peers the rung has not confirmed for longer than `longer_than`.
    ///
    /// Not "peers that have gone" — only peers worth asking about. A rung that
    /// re-reports constantly will never list anything here; one that re-resolves
    /// every ~98 s, as mDNS does, will list a peer that is present and perfectly
    /// well. The answer to that is a probe, not a deletion.
    pub fn unheard_from(&self, now: Instant, longer_than: Duration) -> Vec<PeerId> {
        let last_seen = self
            .inner
            .last_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.inner
            .sightings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|peer| {
                last_seen
                    .get(*peer)
                    .is_none_or(|seen| now.saturating_duration_since(*seen) >= longer_than)
            })
            .cloned()
            .collect()
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
            return self.rotate(now);
        }
        // The epoch grid is shared and our rotation timer is not, so the two
        // drift apart and a hint can fall due long before an id does. Left to
        // the rotation alone, a device holding an open pipe — which refuses to
        // rotate — would keep advertising a hint from epochs ago, and its
        // friends would stop recognising it. `publish` is a no-op unless the
        // bytes have actually moved.
        self.publish()
    }

    /// Rotate now. A rotation while a pipe is open is refused by the rung
    /// (contract rule 4); that is not an error worth surfacing — it means the
    /// device is mid-conversation, and the next tick will try again.
    pub fn rotate(&self, now: Instant) -> Result<(), TransportError> {
        let fresh = ephemeral_id();
        // Before the id moves, not after: see `go_quiet`.
        self.go_quiet()?;
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
            // Refused because a pipe is open. We have already gone quiet, so
            // put the old hint back rather than leaving the advert bare until
            // some later rotation succeeds — it is the same hint under the same
            // id it was already advertising, and links nothing new.
            Err(TransportError::Unavailable(_)) => self.publish(),
            Err(e) => Err(e),
        }
    }

    /// Advertise the hint for right now, if it is not already on the air.
    ///
    /// The payload is the hint and nothing else. The persona stays behind the
    /// endpoint, disclosed to a requester that has identified itself first;
    /// putting it here would hand it to every scanner in range and make that
    /// ordering decorative. The hint is the opposite kind of thing — it names
    /// nobody, and only somebody who has already paired with us can tell it
    /// from noise.
    ///
    /// Returns early when the bytes have not moved. The radio call is not free
    /// and this runs on every tick; the hint only changes when the epoch turns
    /// or our id does.
    fn publish(&self) -> Result<(), TransportError> {
        let want = self.payload_now();
        let mut air = self
            .inner
            .on_the_air
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if air.as_deref() == Some(want.as_slice()) {
            return Ok(());
        }
        self.inner.transport.start_advertising(want.clone())?;
        *air = Some(want);
        Ok(())
    }

    /// The payload we should be advertising at this moment.
    ///
    /// Empty when we do not know what id we are advertising under. The hint is
    /// bound to that id, so one computed over the wrong string is not a weaker
    /// hint — it is eight bytes no friend can match and no stranger can read,
    /// with nothing on any screen to say so. Advertising nothing is the honest
    /// version of the same state, and is what this did before T09a.
    ///
    /// Reachable only by constructing a `Discovery` and advertising without
    /// telling it its id; `Net::new` seeds it for exactly this reason. Hence
    /// the assertion — a caller that gets this wrong should find out during
    /// its own tests, not from a friend who stopped being recognised.
    fn payload_now(&self) -> Vec<u8> {
        let id = self.local_id();
        debug_assert!(
            !id.is_empty(),
            "advertising before the local id was seeded: the hint has nothing to bind to"
        );
        if id.is_empty() {
            log::warn!("advertising without a local id, so no hint goes out");
            return Vec::new();
        }
        let l1 = self
            .inner
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layer1_public();
        hint::hint_for(&l1.0, &id, hint::epoch_at((self.inner.clock)())).to_vec()
    }

    /// Advertise nothing, without saying we have stopped being discoverable.
    ///
    /// Used for the moment in [`Self::rotate`] between one id and the next. The
    /// rung re-advertises the payload it was last given when the id changes, so
    /// without this the new id would go out carrying the *old* hint — the same
    /// eight bytes under two ids, which is the link a rotation exists to break,
    /// readable by any scanner without a key or a pairing.
    fn go_quiet(&self) -> Result<(), TransportError> {
        self.inner.transport.start_advertising(Vec::new())?;
        *self
            .inner
            .on_the_air
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
        Ok(())
    }

    /// Feed a transport event in. Returns whether the nearby list actually
    /// changed.
    ///
    /// The answer is not always yes, and the difference is not small. mDNS
    /// resolves a peer once per interface and address, so a single LAN peer
    /// appearing produced sixteen `PeerFound` events inside one second, then
    /// another every ~98 s for as long as it sat there — measured twice,
    /// identically, in `lan_re_resolves_a_peer_that_never_moved`. Every one of
    /// them used to push a fresh device list across the bridge and rebuild the
    /// screen, for a list that was identical each time.
    pub fn on_event(&self, event: TransportEvent, now: Instant) -> bool {
        match event {
            TransportEvent::PeerFound { peer, payload } => {
                // Recorded on *every* sighting, including the re-resolves that
                // change nothing: the question this answers is "when did the
                // rung last confirm it", which a repeat does confirm.
                self.inner
                    .last_seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.clone(), now);
                // Re-sent whenever the advertised payload changes (T08 rule on
                // PeerFound), so this fires repeatedly for a peer we already
                // know. Overwriting would discard a persona we have already
                // fetched and signature-checked, and the UI would watch names
                // blink out for no reason the user could see.
                // A persona may already be known for this peer — it dialled us
                // and identified itself before its advertisement arrived. Read
                // it out before taking the sightings lock, never both at once.
                let known = self
                    .inner
                    .personas
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&peer)
                    .cloned();
                let mut sightings = self
                    .inner
                    .sightings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let seen = hint::read(&payload);
                match sightings.get_mut(&peer) {
                    // A re-resolve of a peer already listed usually changes
                    // nothing, and the mDNS ones arrive by the dozen. The hint
                    // is the one part that legitimately moves under a settled
                    // id — it turns with the epoch, and it appears for the
                    // first time when a peer that was advertising nothing
                    // starts carrying one. Both change who the row is.
                    //
                    // An advertisement carrying *no* hint does not take away
                    // one we already have. Our own rotation produces exactly
                    // that event — [`Self::rotate`] goes quiet under the old id
                    // before renaming itself — so a sighting that forgot on
                    // sight of it would lose the peer the instant it rotated,
                    // and keep losing it for as long as the rung remembered the
                    // old service. On mDNS that is not prompt, so the peer
                    // stays unresolvable and the duplicate row T09a exists to
                    // prevent comes back, minutes after it was working.
                    //
                    // Keeping it is also the more honest reading: a hint
                    // identifies its peer for its epoch window whether or not
                    // the next advertisement repeats it, and nobody who cannot
                    // compute one can put one there.
                    Some(existing) => match seen {
                        Some(fresh) => {
                            let news = existing.hint != Some(fresh);
                            existing.hint = Some(fresh);
                            news
                        }
                        None => false,
                    },
                    None => {
                        sightings.insert(
                            peer.clone(),
                            Sighting {
                                peer,
                                persona: known,
                                hint: seen,
                            },
                        );
                        true
                    }
                }
            }
            TransportEvent::PeerLost { peer } => {
                self.inner
                    .last_seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer);
                self.inner
                    .personas
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer);
                self.inner
                    .sightings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer)
                    .is_some()
            }
            // Bytes are *not* handled here any more. The pipe carries two
            // protocols, and telling them apart by inspecting content cannot be
            // made reliable — see `engine::pipe`. The caller demultiplexes and
            // hands whole requests to `answer`.
            _ => false,
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

            // A first-contact request carries no pseudonym to gate on — the
            // zero placeholder is not a peer — so it is answered on the
            // allowance alone. That is not a hole a blocked device can walk
            // through: the record it gets back is the same public persona it
            // could read off our advertisement, and every step after it
            // (handshake, session, frames) asks the gate.
            let blocked = !request.is_first_contact()
                && self.inner.blocked.ingress_gate(&request.pseudonym) == Admit::Silence;

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
    /// sighting stayed nameless for the life of the session — the side holding
    /// the larger id, whichever device that happens to be until the next
    /// rotation deals the ids again.
    ///
    /// What arrives here is *better* attested than what `accept_persona`
    /// verifies. A record off the discovery channel proves only that it was
    /// signed; one that came through the handshake is additionally bound to the
    /// static key the peer proved possession of, which is the check that closed
    /// the T10 impersonation hole. So overwriting is an upgrade.
    ///
    /// A peer with no sighting is left alone: nothing has advertised it, so
    /// there is no tile to name.
    ///
    /// `pub(crate)` deliberately. Unlike [`Self::accept_persona`], which
    /// verifies bytes, this takes the verdict as a type — and
    /// `VerifiedPersona` has all-public fields and no private constructor, so
    /// the name is a claim rather than something the type enforces. Keeping
    /// this off the public API means the only way in from outside is the one
    /// that actually checks a signature.
    pub(crate) fn note_persona(&self, peer: &str, persona: identity::VerifiedPersona) {
        // Recorded first and unconditionally, so a persona that arrives before
        // the advertisement is not lost. Locks are taken one at a time and in
        // this order everywhere; this module has already paid for a lock
        // inversion once.
        self.inner
            .personas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(peer.to_string(), persona.clone());
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

/// The wall clock, in milliseconds since the Unix epoch.
///
/// A clock that cannot be read reports zero rather than refusing. The only
/// consumer is the hint's epoch, and a device whose clock is that broken will
/// simply not be recognised by its friends until the clock is set — which is
/// the same outcome as any other disagreement about the time, and better than
/// discovery failing outright over it.
fn system_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
