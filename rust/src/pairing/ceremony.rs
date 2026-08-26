//! The ceremony itself (T11 slice 2; tech spec §6, R0-F4).
//!
//! ```text
//!   scanner                                              shower
//!     |--- 1. e ------------------------------------------->|
//!     |<-- 2. e, ee, s, es  + shower's persona -------------|
//!     |--- 3. s, se         + scanner's persona ----------->|
//!             both derive the SAS from the transcript
//!             both people look at their screens
//!     |--- confirm ---------------------------------------->|
//!     |<-- confirm -----------------------------------------|
//!     |--- Layer-1 proof ---------------------------------->|
//!     |<-- Layer-1 proof -----------------------------------|
//! ```
//!
//! # Why XX here when sessions use IK
//!
//! [`crate::session::handshake`] explains at length why ordinary sessions had
//! to move from XX to IK: the responder must identify a caller before emitting
//! a byte (R0-F10), and it cannot derive a per-counterparty static without the
//! caller's Layer-2 key. Neither applies here. A ceremony is *invited* — the
//! shower is holding a code up — so there is no blocked-caller silence to keep,
//! and the invite carries an Ed25519 Layer-2 key, not the X25519 static IK
//! would need to be told in advance. XX exchanges both statics inside the
//! handshake, which is exactly what this needs.
//!
//! # What binds this handshake to the code that was scanned
//!
//! Two things, and they catch different attacks.
//!
//! **The nonce is the Noise prologue.** Both sides mix the invite's 32 random
//! bytes into `h` before anything else, so a handshake belonging to one code
//! cannot be replayed into another: the AEAD tag on message 2 fails and the
//! ceremony dies without either person seeing a screen.
//!
//! **The scanner checks the persona against the code.** Message 2 carries the
//! shower's persona record and a signature binding it to the static Noise just
//! proved, and the scanner rejects the ceremony unless that record's Layer-2
//! key is the one printed in the code it read. This is what defeats a pure
//! relay without troubling the humans at all: an attacker who forwards messages
//! between two devices cannot produce the shower's static, so the handshake
//! simply fails. What remains for the SAS to catch is the attacker who
//! *substituted the code* — and that one is why two people compare colours.
//!
//! # Layer-1 crosses last, and only after both confirmations
//!
//! Tech spec §6 puts the Layer-1 exchange in step 3, *before* the humans
//! confirm. R0-F4's acceptance says "no identity crosses without both
//! confirmations". Those disagree, and this module follows the requirement.
//!
//! The difference is not academic. Under a relayed ceremony both sides complete
//! a handshake with the attacker and only the mismatched SAS gives it away. If
//! Layer-1 travels first, the attacker has both long-term public identities
//! before anyone has looked at a screen — stable identifiers that outlive every
//! rotation R0-F2 provides. Sending them last costs one round trip and means an
//! honest person who declines has disclosed nothing.
//!
//! It does not make a relay free: if *your* human confirms a SAS that does not
//! match, your Layer-1 key goes to the attacker, because the attacker can
//! fabricate the peer's confirmation from inside a channel it owns. Nothing
//! short of the humans doing their job fixes that, which is the honest summary
//! of what a SAS is.
//!
//! # Aborting
//!
//! There is no abort message and no state to unwind: dropping a [`Ceremony`]
//! *is* the abort, at any point, and it leaves nothing behind because nothing
//! outside this value has been written. Persistence happens after
//! [`Paired`] comes out, which is slice 3's job.

use crate::protocol::PROTOCOL_VERSION;
use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, TransportState};

use prost::Message;

use zeroize::Zeroizing;

use crate::crypto::{dh, hash, sign};
use crate::identity::{self, Identity, VerifiedPersona};
use crate::proto::v0::L1Proof;
use crate::session::handshake::{decode_intro, encode_intro, HandshakeError};

use super::invite::{Invite, CEREMONY_NONCE_LEN};
use super::sas::{self, Sas};

/// The suite, pinned like the session's. Same primitives, different pattern.
pub const CEREMONY_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Domain separator for what a Layer-1 key signs. Versioned: a change here
/// makes every proof from an older build unverifiable, which is a wire break.
const L1_PROOF_LABEL: &[u8] = b"hoppler/pairing/l1-proof/v1";

/// Noise's ceiling on one message.
const MAX_NOISE_MESSAGE: usize = 65535;

/// Largest post-handshake ceremony frame we will accept. A Layer-1 proof is
/// about a hundred bytes; this is slack, and a bound on what a peer can make us
/// hold.
pub const MAX_CEREMONY_FRAME: usize = 4096;

/// Which end of the code this device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Read the code and dialled. Speaks first.
    Scanner,
    /// Displayed the code and is being dialled.
    Shower,
}

/// Post-handshake frame types, inside the encrypted channel.
///
/// Tagged rather than positional because a confirmation is not synchronous:
/// the other person may tap before or after this one does, so both orders have
/// to be readable. `#[repr(u8)]` for the same reason as
/// [`crate::session::frame::FrameKind`] — the discriminant *is* the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum FrameKind {
    /// This side's human confirmed the SAS. Carries nothing.
    ///
    /// Deliberately empty. Sending the SAS itself, or a hash of it, would hand
    /// a relaying attacker the value it is trying to guess — the one secret the
    /// humans hold that the protocol does not.
    Confirm = 1,
    /// A [`L1Proof`].
    L1 = 2,
}

impl FrameKind {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(FrameKind::Confirm),
            2 => Some(FrameKind::L1),
            _ => None,
        }
    }
}

/// Why a ceremony stopped.
#[derive(Debug)]
pub enum CeremonyError {
    /// The Noise layer refused. A wrong ceremony nonce lands here, as a failed
    /// tag on message 2 — the prologue is not a field anyone can check, it is
    /// simply the difference between the tag verifying and not.
    Noise(String),
    /// The handshake completed but the persona inside it did not.
    Persona(identity::IdentityError),
    /// The persona proved by the handshake is not the one whose code we
    /// scanned. A relay, or a code read from the wrong screen.
    NotTheCodeWeScanned,
    /// A Layer-1 proof whose signature did not verify against the key it
    /// carried, or against this ceremony.
    L1ProofInvalid,
    /// A frame arrived that this point in the ceremony does not accept —
    /// including a Layer-1 proof before both confirmations, which is the
    /// ordering this module exists to enforce.
    OutOfOrder,
    /// A frame that did not parse, or was larger than [`MAX_CEREMONY_FRAME`].
    Malformed,
    /// The peer speaks a different protocol version.
    ///
    /// Normally arrives as [`CeremonyError::Noise`] instead — the version is in
    /// the prologue, so a disagreement means nothing decrypts and there is no
    /// intro left to read a number out of. See [`crate::protocol`].
    VersionMismatch { theirs: u8, ours: u8 },
}

impl std::fmt::Display for CeremonyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CeremonyError::Noise(e) => write!(f, "ceremony handshake failed: {e}"),
            CeremonyError::Persona(e) => write!(f, "persona in the ceremony invalid: {e}"),
            CeremonyError::NotTheCodeWeScanned => {
                write!(f, "the device that answered is not the one on the code")
            }
            CeremonyError::L1ProofInvalid => write!(f, "layer-1 proof did not verify"),
            CeremonyError::OutOfOrder => write!(f, "ceremony message arrived out of order"),
            CeremonyError::Malformed => write!(f, "malformed ceremony message"),
            CeremonyError::VersionMismatch { theirs, ours } => write!(
                f,
                "peer speaks protocol version {theirs}, this build speaks {ours}"
            ),
        }
    }
}

impl std::error::Error for CeremonyError {}

impl From<HandshakeError> for CeremonyError {
    fn from(e: HandshakeError) -> Self {
        match e {
            HandshakeError::Noise(s) => CeremonyError::Noise(s),
            HandshakeError::Persona(p) => CeremonyError::Persona(p),
            HandshakeError::PseudonymMismatch => CeremonyError::Persona(
                // The intro's own binding check failing means the record and
                // the static disagree — the same "this persona is not yours"
                // finding, reported in this module's vocabulary.
                identity::IdentityError::RecordSignatureInvalid,
            ),
            HandshakeError::PayloadTooLarge => CeremonyError::Malformed,
            // Unreachable while the version is also in the prologue: two builds
            // that disagree derive different `h`, so message 1 fails to decrypt
            // and this side never sees an intro to read a version out of. Kept
            // as the belt to that prologue's braces — if the binding is ever
            // loosened, the ceremony still refuses with a reason instead of
            // pairing two builds that cannot read each other.
            HandshakeError::VersionMismatch { theirs, ours } => {
                CeremonyError::VersionMismatch { theirs, ours }
            }
        }
    }
}

/// What the caller should do next. A single read can produce more than one, so
/// these come back as a list — the same shape as [`crate::engine::net`]'s
/// events, and for the same reason.
pub enum Step {
    /// Put these bytes on the wire, in order.
    Send(Vec<u8>),
    /// The handshake is done and verified. Show this to the person, and do not
    /// send anything further until they say yes.
    ShowSas(Sas),
    /// The other person tapped confirm. On its own this means nothing has been
    /// disclosed yet — it is a UI cue, not a result.
    PeerConfirmed,
    /// Both sides confirmed and both Layer-1 proofs checked out. Paired.
    Paired(Box<Paired>),
}

/// A `Debug` that names the variant and refuses to print the payload.
///
/// Written by hand rather than derived because [`Paired`] carries live
/// transport keys, and `#[derive(Debug)]` on this enum is all it would take for
/// a `{:?}` in a log line — or a test failure message — to write session key
/// material to disk. The same reason [`Identity`] has no `Debug` at all.
impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Step::Send(bytes) => write!(f, "Send({} bytes)", bytes.len()),
            Step::ShowSas(sas) => write!(f, "ShowSas({})", sas.spoken()),
            Step::PeerConfirmed => write!(f, "PeerConfirmed"),
            Step::Paired(_) => write!(f, "Paired(..)"),
        }
    }
}

/// What a completed ceremony yields. Everything a contact and a persistent
/// thread need, and the live channel that produced them.
///
/// No `Debug`: it holds the ceremony channel's transport keys.
pub struct Paired {
    /// The counterpart's Layer-2 persona, signature-checked.
    pub persona: VerifiedPersona,
    /// Their Layer-1 public identity — the thing pairing exists to move.
    pub l1_pub: sign::PublicKey,
    /// What both people compared. Kept so the UI can show what was agreed.
    pub sas: Sas,
    /// Our half of the ratchet exchange, kept rather than dropped.
    ///
    /// The side that answers seeds its ratchet with this secret — the other end
    /// was given its public in the Layer-1 proof, so it is the key pair the
    /// first message will turn. Deriving the root and throwing this away would
    /// leave a root nothing could hang a chain off.
    pub ratchet_secret: dh::DhSecret,
    /// Their half. The side that speaks first seeds its ratchet with this.
    pub their_ratchet: dh::DhPublic,
    /// The seed for this thread's Double Ratchet.
    ///
    /// Both sides derive the same 32 bytes from the exchange in the Layer-1
    /// proofs, and neither side chose them. Bound to the ceremony transcript,
    /// so the root a message chain hangs off is authenticated by the same
    /// comparison the two people made with their eyes — swap the transcript and
    /// the root changes with it.
    pub root: Zeroizing<[u8; 32]>,
    /// The ceremony channel, still open. Handed on rather than dropped: it is
    /// an authenticated channel to someone who has just been verified, and
    /// re-establishing one costs a round trip for nothing.
    pub transport: TransportState,
}

/// One side of one ceremony. Dropping it aborts.
pub struct Ceremony {
    role: Role,
    /// The Layer-2 key the code named. For the shower this is its own — it made
    /// the code — and is unused; for the scanner it is what message 2 must
    /// match.
    expected_l2: sign::PublicKey,
    nonce: [u8; CEREMONY_NONCE_LEN],
    stage: Stage,
}

/// What the middle stage carries: a live transport, who the far side proved to
/// be, the transcript the SAS came from, and where each human has got to.
///
/// Its own struct behind a `Box` rather than fields on the variant. `Opening`
/// is a pointer and `Closed` is nothing, so an inline payload here sets the
/// size of every `Stage` — and it grew past what clippy tolerates the moment
/// the SAS went from two colours to three. Boxing the whole thing rather than
/// the one field that grew, because the next thing to grow would trip it again.
struct Confirming {
    transport: Box<TransportState>,
    persona: VerifiedPersona,
    transcript: Vec<u8>,
    sas: Sas,
    /// Our half of the key that will seed this thread's Double Ratchet.
    ///
    /// Generated here rather than at pairing's end because both sides must send
    /// their public with the Layer-1 proof, and the proof is the last thing
    /// either side sends. Contributory on purpose: a root chosen by one device
    /// is a root only that device's RNG has to fail for.
    ratchet_secret: dh::DhSecret,
    we_confirmed: bool,
    they_confirmed: bool,
    /// Set once we have sent ours.
    ///
    /// A second lock with no reachable path today, and recorded as one rather
    /// than left to be found: a peer's repeat confirmation is already refused
    /// by `they_confirmed` in [`Ceremony::read`], so nothing gets as far as
    /// asking this. Removing it therefore breaks no test — the mutant survives
    /// — and that is the expected result, not a hole for somebody to fill by
    /// deleting the field.
    ///
    /// Kept because the guard it duplicates is one branch in a state machine
    /// that a later change could reasonably relax, and the cost of being wrong
    /// here is re-emitting a long-term identity on a peer's say-so.
    we_proved: bool,
}

enum Stage {
    /// Handshaking. The scanner is here after sending message 1; the shower
    /// from the start.
    Opening(Box<HandshakeState>),
    /// Verified, waiting on two humans.
    Confirming(Box<Confirming>),
    /// Finished, either way. Holding no keys.
    Closed,
}

impl Ceremony {
    /// Begin as the scanner, having just read a code. Returns the first
    /// handshake message.
    pub fn scan(us: &Identity, invite: &Invite) -> Result<(Self, Vec<u8>), CeremonyError> {
        let mut state = open_handshake(us, &invite.nonce, Role::Scanner)?;
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        // Message 1 is `-> e` — no key agreement has happened, so anything in
        // its payload would go out in clear. Nothing goes in it.
        let n = state.write_message(&[], &mut buf).map_err(noise)?;
        buf.truncate(n);
        Ok((
            Self {
                role: Role::Scanner,
                expected_l2: invite.l2_pub,
                nonce: invite.nonce,
                stage: Stage::Opening(Box::new(state)),
            },
            buf,
        ))
    }

    /// Begin as the shower, having just put a code on screen. Sends nothing —
    /// the scanner speaks first.
    ///
    /// `invite` must be the one being displayed. Its nonce is what binds this
    /// ceremony, so showing one code and running the ceremony against another
    /// fails the handshake rather than pairing the wrong pair.
    pub fn show(us: &Identity, invite: &Invite) -> Result<Self, CeremonyError> {
        let state = open_handshake(us, &invite.nonce, Role::Shower)?;
        Ok(Self {
            role: Role::Shower,
            expected_l2: invite.l2_pub,
            nonce: invite.nonce,
            stage: Stage::Opening(Box::new(state)),
        })
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// The SAS, once there is one.
    pub fn sas(&self) -> Option<Sas> {
        match &self.stage {
            Stage::Confirming(c) => Some(c.sas),
            _ => None,
        }
    }

    /// This device's human confirmed the colours match.
    ///
    /// Emits our confirmation, and our Layer-1 proof too if the other side has
    /// already confirmed. That ordering is the point: see the module header.
    pub fn confirm(&mut self, us: &Identity) -> Result<Vec<Step>, CeremonyError> {
        let Stage::Confirming(c) = &mut self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        if c.we_confirmed {
            return Err(CeremonyError::OutOfOrder);
        }
        c.we_confirmed = true;
        let mut steps = vec![Step::Send(self.seal(FrameKind::Confirm, &[])?)];
        steps.extend(self.prove_if_ready(us)?);
        Ok(steps)
    }

    /// Consume a message from the peer.
    pub fn read(&mut self, us: &Identity, message: &[u8]) -> Result<Vec<Step>, CeremonyError> {
        match &mut self.stage {
            Stage::Opening(_) => self.read_handshake(us, message),
            Stage::Confirming(_) => self.read_frame(us, message),
            Stage::Closed => Err(CeremonyError::OutOfOrder),
        }
    }

    // ── handshake ───────────────────────────────────────────────────────────

    /// Consume one handshake message and produce whatever it obliges us to do.
    ///
    /// Which of the three messages just arrived is read off
    /// `is_handshake_finished` plus the role, rather than tracked in a counter.
    /// XX has exactly three messages and each side reads a fixed subset of
    /// them, so the state is already there in the Noise machine — a second copy
    /// in a field of ours would be one more thing that can disagree with it.
    ///
    /// - not finished, shower → that was message 1; answer with message 2.
    /// - not finished, scanner → that was message 2; check it, send message 3.
    /// - finished, shower → that was message 3; done.
    ///
    /// The fourth combination does not occur: the scanner never reads a message
    /// after the handshake completes, because there is not one to read.
    fn read_handshake(
        &mut self,
        us: &Identity,
        message: &[u8],
    ) -> Result<Vec<Step>, CeremonyError> {
        let (finished, payload) = {
            let Stage::Opening(state) = &mut self.stage else {
                return Err(CeremonyError::OutOfOrder);
            };
            let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
            let n = state.read_message(message, &mut buf).map_err(noise)?;
            buf.truncate(n);
            (state.is_handshake_finished(), buf)
        };

        match (finished, self.role) {
            // Message 1 read. It carries nothing — no key agreement has
            // happened yet, so anything in it would have travelled in clear.
            // Answer with our static and the persona that owns it.
            (false, Role::Shower) => {
                let intro = encode_intro(us, &us.session_public())?;
                Ok(vec![Step::Send(self.write_handshake(&intro)?)])
            }
            // Message 2 read. This is the check that makes a relay fail without
            // troubling either person: the persona proved by the handshake must
            // be the one whose code this device photographed.
            (false, Role::Scanner) => {
                let persona = self.peer_intro(&payload)?;
                if persona.l2_pub != self.expected_l2 {
                    self.stage = Stage::Closed;
                    return Err(CeremonyError::NotTheCodeWeScanned);
                }
                let intro = encode_intro(us, &us.session_public())?;
                let out = self.write_handshake(&intro)?;
                self.finish_with(persona)?;
                Ok(vec![
                    Step::Send(out),
                    Step::ShowSas(self.sas().expect("just set")),
                ])
            }
            // Message 3 read. The shower learns who scanned only now — it had
            // no way to know in advance, and nothing about the code says.
            (true, Role::Shower) => {
                let persona = self.peer_intro(&payload)?;
                self.finish_with(persona)?;
                Ok(vec![Step::ShowSas(self.sas().expect("just set"))])
            }
            (true, Role::Scanner) => Err(CeremonyError::OutOfOrder),
        }
    }

    fn write_handshake(&mut self, payload: &[u8]) -> Result<Vec<u8>, CeremonyError> {
        let Stage::Opening(state) = &mut self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state.write_message(payload, &mut out).map_err(noise)?;
        out.truncate(n);
        Ok(out)
    }

    /// The peer's persona, checked against the static Noise just proved they
    /// hold. Shared by both roles, because both face the same question.
    fn peer_intro(&self, payload: &[u8]) -> Result<VerifiedPersona, CeremonyError> {
        let Stage::Opening(state) = &self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        let remote = remote_static_of(state)?;
        Ok(decode_intro(payload, &remote)?)
    }

    /// Move into transport mode and render the SAS.
    ///
    /// The transcript is taken *before* the conversion, because it is the
    /// handshake's own hash — both sides compute the same value over everything
    /// that was said, including the prologue, and that is precisely what a
    /// relay cannot make agree.
    fn finish_with(&mut self, persona: VerifiedPersona) -> Result<(), CeremonyError> {
        let Stage::Opening(state) = std::mem::replace(&mut self.stage, Stage::Closed) else {
            return Err(CeremonyError::OutOfOrder);
        };
        let transcript = state.get_handshake_hash().to_vec();
        let transport = state.into_transport_mode().map_err(noise)?;
        let sas = sas::derive(&transcript, &self.nonce);
        self.stage = Stage::Confirming(Box::new(Confirming {
            transport: Box::new(transport),
            persona,
            transcript,
            sas,
            ratchet_secret: dh::DhSecret::generate(),
            we_confirmed: false,
            they_confirmed: false,
            we_proved: false,
        }));
        Ok(())
    }

    // ── after the handshake ─────────────────────────────────────────────────

    fn read_frame(&mut self, us: &Identity, message: &[u8]) -> Result<Vec<Step>, CeremonyError> {
        let (kind, payload) = self.open(message)?;
        match kind {
            FrameKind::Confirm => {
                let Stage::Confirming(c) = &mut self.stage else {
                    return Err(CeremonyError::OutOfOrder);
                };
                if c.they_confirmed {
                    // A second confirmation is not additional consent. Refusing
                    // it keeps `prove_if_ready` from being re-entered by a peer
                    // that simply repeats itself.
                    return Err(CeremonyError::OutOfOrder);
                }
                c.they_confirmed = true;
                let mut steps = vec![Step::PeerConfirmed];
                steps.extend(self.prove_if_ready(us)?);
                Ok(steps)
            }
            FrameKind::L1 => self.read_l1(&payload),
        }
    }

    /// Send our Layer-1 proof, but only once both people have confirmed.
    ///
    /// The single place a Layer-1 key is written to the wire, and the gate is
    /// the whole reason it is a function rather than three lines at each call
    /// site: two callers reach it — our own confirmation and the peer's — and
    /// whichever arrives second is the one that fires.
    fn prove_if_ready(&mut self, us: &Identity) -> Result<Vec<Step>, CeremonyError> {
        let Stage::Confirming(c) = &mut self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        if !(c.we_confirmed && c.they_confirmed) || c.we_proved {
            return Ok(Vec::new());
        }
        c.we_proved = true;
        let l1_pub = us.layer1_public();
        let ratchet_pub = c.ratchet_secret.public();
        let message = l1_proof_message(&c.transcript, &l1_pub, &us.layer2_public(), &ratchet_pub);
        let proof = L1Proof {
            l1_pub: l1_pub.0.to_vec(),
            signature: us.sign_with_layer1(&message).0.to_vec(),
            ratchet_pub: ratchet_pub.0.to_vec(),
        }
        .encode_to_vec();
        Ok(vec![Step::Send(self.seal(FrameKind::L1, &proof)?)])
    }

    fn read_l1(&mut self, payload: &[u8]) -> Result<Vec<Step>, CeremonyError> {
        let Stage::Confirming(c) = &self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        // The ordering rule, enforced on the receiving side too. A peer that
        // sends its Layer-1 key early is not doing us a favour — accepting it
        // would pair us with someone neither person has confirmed.
        if !(c.we_confirmed && c.they_confirmed) {
            return Err(CeremonyError::OutOfOrder);
        }

        let (l1_pub, their_ratchet) = verify_l1_proof(&c.transcript, &c.persona.l2_pub, payload)?;

        // Taken by value now the checks have passed, so the stage closes and
        // its keys go with it. Read through the borrow above until this point.
        let Stage::Confirming(c) = std::mem::replace(&mut self.stage, Stage::Closed) else {
            return Err(CeremonyError::OutOfOrder);
        };
        let root = ratchet_root(&c.ratchet_secret, &their_ratchet, &c.transcript)?;
        Ok(vec![Step::Paired(Box::new(Paired {
            persona: c.persona,
            l1_pub,
            sas: c.sas,
            ratchet_secret: c.ratchet_secret,
            their_ratchet,
            root,
            transport: *c.transport,
        }))])
    }

    // ── framing inside the channel ──────────────────────────────────────────

    fn seal(&mut self, kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>, CeremonyError> {
        if payload.len() > MAX_CEREMONY_FRAME {
            return Err(CeremonyError::Malformed);
        }
        let Stage::Confirming(c) = &mut self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        let mut plain = Vec::with_capacity(KIND_LEN + payload.len());
        plain.push(kind as u8);
        plain.extend_from_slice(payload);
        let mut out = vec![0u8; MAX_NOISE_MESSAGE];
        let n = c.transport.write_message(&plain, &mut out).map_err(noise)?;
        out.truncate(n);
        Ok(out)
    }

    fn open(&mut self, message: &[u8]) -> Result<(FrameKind, Vec<u8>), CeremonyError> {
        let Stage::Confirming(c) = &mut self.stage else {
            return Err(CeremonyError::OutOfOrder);
        };
        if message.len() > MAX_CEREMONY_CIPHERTEXT {
            return Err(CeremonyError::Malformed);
        }
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = c.transport.read_message(message, &mut buf).map_err(noise)?;
        buf.truncate(n);
        let (kind, payload) = buf.split_first().ok_or(CeremonyError::Malformed)?;
        if payload.len() > MAX_CEREMONY_FRAME {
            return Err(CeremonyError::Malformed);
        }
        let kind = FrameKind::from_byte(*kind).ok_or(CeremonyError::Malformed)?;
        Ok((kind, payload.to_vec()))
    }
}

/// The kind byte ahead of every post-handshake payload.
const KIND_LEN: usize = 1;

/// The AEAD tag Noise adds to a transport message.
const MAX_NOISE_OVERHEAD: usize = 16;

/// The outer bound, on the ciphertext as it arrives.
///
/// Composed from the parts rather than written as a number, because the first
/// version was not: it bounded the ciphertext at payload + tag and forgot the
/// kind byte, so a frame carrying a full [`MAX_CEREMONY_FRAME`] payload was
/// rejected as malformed by the outer check that the inner one would have
/// allowed. Harmless today — a Layer-1 proof is a hundred-odd bytes — but the
/// two checks disagreeing about what the limit *is* is the kind of thing that
/// only shows up once something legitimately grows.
const MAX_CEREMONY_CIPHERTEXT: usize = KIND_LEN + MAX_CEREMONY_FRAME + MAX_NOISE_OVERHEAD;

/// Check a Layer-1 proof and return the key it establishes.
///
/// Split out from the state machine because it is the security decision, and
/// because a decision reachable only through six messages of Noise is one a
/// test can barely get at: driving a forgery through the channel means matching
/// the transport's nonce sequence exactly, which is a fact about `snow` rather
/// than about this protocol. As a function it takes the three things that
/// matter — this ceremony's transcript, the Layer-2 key the handshake proved,
/// and the bytes the peer sent — and is testable with all three varied.
///
/// `peer_l2` comes from the handshake and never from the payload. Taking it
/// from the message would let the sender pick which key its proof is bound to,
/// which is the whole substitution this is meant to stop.
/// The root both sides end up with.
///
/// Keyed on the Diffie-Hellman output and bound to the transcript, rather than
/// the DH output used raw: the transcript is what the SAS is computed over, so
/// binding it means the colours the two people compared authenticate the root
/// as well as the identities. A root that survived a transcript substitution
/// would be a root an attacker could have arranged.
fn ratchet_root(
    ours: &dh::DhSecret,
    theirs: &dh::DhPublic,
    transcript: &[u8],
) -> Result<Zeroizing<[u8; 32]>, CeremonyError> {
    let shared = ours
        .diffie_hellman(theirs)
        .map_err(|_| CeremonyError::Malformed)?;
    Ok(Zeroizing::new(hash::keyed_hash(
        shared.as_bytes(),
        &[b"hoppler/ratchet-root/v1".as_slice(), transcript].concat(),
    )))
}

fn verify_l1_proof(
    transcript: &[u8],
    peer_l2: &sign::PublicKey,
    payload: &[u8],
) -> Result<(sign::PublicKey, dh::DhPublic), CeremonyError> {
    let proof = L1Proof::decode(payload).map_err(|_| CeremonyError::Malformed)?;
    let l1_bytes: [u8; sign::PUBLIC_KEY_LEN] = proof
        .l1_pub
        .as_slice()
        .try_into()
        .map_err(|_| CeremonyError::Malformed)?;
    let sig_bytes: [u8; sign::SIGNATURE_LEN] = proof
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| CeremonyError::Malformed)?;
    let l1_pub = sign::PublicKey(l1_bytes);
    let ratchet_bytes: [u8; 32] = proof
        .ratchet_pub
        .as_slice()
        .try_into()
        .map_err(|_| CeremonyError::Malformed)?;
    let ratchet_pub = dh::DhPublic(ratchet_bytes);

    // The ratchet key is inside what the Layer-1 key signed, so a substituted
    // one fails here rather than quietly seeding a root the attacker owns.
    let message = l1_proof_message(transcript, &l1_pub, peer_l2, &ratchet_pub);
    sign::verify(&l1_pub, &message, &sign::Signature(sig_bytes))
        .map_err(|_| CeremonyError::L1ProofInvalid)?;
    Ok((l1_pub, ratchet_pub))
}

/// What a Layer-1 key signs.
///
/// Three ingredients, each closing one substitution:
/// - the **transcript**, so the proof cannot be lifted into another ceremony;
/// - the **Layer-1 key**, so it is a claim about this key and not a bare
///   signature that could be replayed alongside a different one;
/// - the sender's **Layer-2 key**, so the proof binds the two layers together —
///   without it, someone could pass off a Layer-1 key they had merely watched
///   go past in some other pairing.
fn l1_proof_message(
    transcript: &[u8],
    l1_pub: &sign::PublicKey,
    l2_pub: &sign::PublicKey,
    ratchet_pub: &dh::DhPublic,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(L1_PROOF_LABEL.len() + transcript.len() + 96);
    msg.extend_from_slice(L1_PROOF_LABEL);
    msg.extend_from_slice(transcript);
    msg.extend_from_slice(&l1_pub.0);
    msg.extend_from_slice(&l2_pub.0);
    // A fourth ingredient, closing a fourth substitution: the ratchet key. It
    // is the only one of these that is ephemeral, and the only one whose
    // replacement would leave both screens looking exactly right while an
    // attacker held the root of the conversation.
    msg.extend_from_slice(&ratchet_pub.0);
    msg
}

/// Build the Noise state for one side.
///
/// The whole build happens inside this function rather than a `Builder` being
/// handed back, because `snow`'s builder *borrows* the private key bytes: a
/// returned builder would need the exposed copy kept alive by the caller, which
/// is precisely the sort of "who is holding the key material now" question this
/// codebase avoids having. Here the exposed copy is one `Zeroizing` binding
/// with a function-sized lifetime, wiped on the way out.
fn open_handshake(
    us: &Identity,
    nonce: &[u8],
    role: Role,
) -> Result<HandshakeState, CeremonyError> {
    let params: NoiseParams = CEREMONY_PARAMS
        .parse()
        .expect("CEREMONY_PARAMS is a compile-time constant and must parse");
    let secret = us.session_secret();
    let exposed = secret.expose_secret();
    // The nonce binds the ceremony to the code that was scanned; the version
    // binds it to a build that means the same thing by it. A pairing writes a
    // contact, a Layer-1 key and a ratchet seed, so two builds that disagree
    // about the wire would be writing each other durable state neither can
    // read — worse than not pairing at all. Both go into `h` before a single
    // message goes out, so a mismatch in either means nothing decrypts.
    let mut prologue = Vec::with_capacity(nonce.len() + 1);
    prologue.extend_from_slice(nonce);
    prologue.push(PROTOCOL_VERSION);
    let builder = Builder::new(params)
        .prologue(&prologue)
        .map_err(noise)?
        .local_private_key(&*exposed)
        .map_err(noise)?;
    match role {
        Role::Scanner => builder.build_initiator(),
        Role::Shower => builder.build_responder(),
    }
    .map_err(noise)
}

fn noise(e: snow::Error) -> CeremonyError {
    CeremonyError::Noise(e.to_string())
}

fn remote_static_of(state: &HandshakeState) -> Result<dh::DhPublic, CeremonyError> {
    let raw = state
        .get_remote_static()
        .ok_or_else(|| CeremonyError::Noise("no remote static in the ceremony".into()))?;
    let bytes: [u8; dh::PUBLIC_LEN] = raw
        .try_into()
        .map_err(|_| CeremonyError::Noise("remote static had the wrong length".into()))?;
    Ok(dh::DhPublic(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both sides of a ceremony, driven to the point where two people are
    /// looking at colours.
    struct Halfway {
        shower_id: Identity,
        scanner_id: Identity,
        shower: Ceremony,
        scanner: Ceremony,
    }

    fn sends(steps: &[Step]) -> Vec<Vec<u8>> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Send(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    fn one_send(steps: &[Step]) -> Vec<u8> {
        let out = sends(steps);
        assert_eq!(out.len(), 1, "expected exactly one message, got {steps:?}");
        out.into_iter().next().unwrap()
    }

    /// Run the three handshake messages and hand back both sides.
    ///
    /// `code` is what the scanner read, `shown` what the shower is displaying.
    /// They are the same in every honest case; the tests that pass different
    /// ones are the substituted-code and stale-code attacks.
    fn run_handshake(
        shower_id: &Identity,
        scanner_id: &Identity,
        shown: &Invite,
        code: &Invite,
    ) -> (Ceremony, Ceremony) {
        let mut shower = Ceremony::show(shower_id, shown).unwrap();
        let (mut scanner, m1) = Ceremony::scan(scanner_id, code).unwrap();
        let m2 = one_send(&shower.read(shower_id, &m1).unwrap());
        let m3 = one_send(&scanner.read(scanner_id, &m2).unwrap());
        shower.read(shower_id, &m3).unwrap();
        (shower, scanner)
    }

    fn halfway() -> Halfway {
        let shower_id = Identity::generate("Ada", 0x11_2233);
        let scanner_id = Identity::generate("Bo", 0x44_5566);
        let invite = Invite::fresh(shower_id.layer2_public(), "a1b2c3d4");
        let (shower, scanner) = run_handshake(&shower_id, &scanner_id, &invite, &invite);
        Halfway {
            shower_id,
            scanner_id,
            shower,
            scanner,
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// A pairing across two protocol versions must not complete.
    ///
    /// This is the half of the version gate that protects *durable* state. A
    /// ceremony writes a contact row, a Layer-1 key and a ratchet seed; two
    /// builds that disagree about the wire would be writing each other records
    /// neither can read, which is worse than not pairing at all.
    ///
    /// The failure lands on message 2, not message 1 — XX's first message is
    /// `-> e` with nothing authenticated yet, so a prologue disagreement is
    /// invisible until the first tag has to verify. That is the same place a
    /// wrong ceremony *nonce* fails, and for the same reason: the prologue is
    /// not a field anyone checks, it is the difference between a tag verifying
    /// and not.
    #[test]
    fn a_ceremony_between_two_protocol_versions_cannot_complete() {
        let shower_id = Identity::generate("Ada", 0x11_2233);
        let scanner_id = Identity::generate("Bo", 0x44_5566);
        let invite = Invite::fresh(shower_id.layer2_public(), "a1b2c3d4");

        // A scanner from some other build: same code, same everything, one
        // different number folded into the prologue.
        let params: NoiseParams = CEREMONY_PARAMS.parse().unwrap();
        let secret = scanner_id.session_secret();
        let mut foreign_prologue = invite.nonce.to_vec();
        foreign_prologue.push(PROTOCOL_VERSION.wrapping_add(1));
        let mut foreign = Builder::new(params)
            .prologue(&foreign_prologue)
            .unwrap()
            .local_private_key(&*secret.expose_secret())
            .unwrap()
            .build_initiator()
            .unwrap();

        let mut buf = vec![0u8; MAX_CEREMONY_FRAME];
        let n = foreign.write_message(&[], &mut buf).unwrap();
        buf.truncate(n);

        let mut shower = Ceremony::show(&shower_id, &invite).unwrap();
        // Message 1 carries no tag, so this side has nothing to object to yet.
        let m2 = one_send(&shower.read(&shower_id, &buf).unwrap());

        // Message 2 is where the disagreement becomes arithmetic.
        let mut back = vec![0u8; MAX_CEREMONY_FRAME];
        assert!(
            foreign.read_message(&m2, &mut back).is_err(),
            "a ceremony completed across two protocol versions"
        );

        // The other half, and the one that actually pins the prologue: the same
        // hand-built peer using `nonce || PROTOCOL_VERSION` gets through.
        // Without this the test above passes for a build that never put the
        // version in at all — two prologues that differ are two prologues that
        // differ, whichever of them is the odd one out.
        let params: NoiseParams = CEREMONY_PARAMS.parse().unwrap();
        let mut agreed_prologue = invite.nonce.to_vec();
        agreed_prologue.push(PROTOCOL_VERSION);
        let mut agreed = Builder::new(params)
            .prologue(&agreed_prologue)
            .unwrap()
            .local_private_key(&*secret.expose_secret())
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut buf = vec![0u8; MAX_CEREMONY_FRAME];
        let n = agreed.write_message(&[], &mut buf).unwrap();
        buf.truncate(n);
        let mut shower = Ceremony::show(&shower_id, &invite).unwrap();
        let m2 = one_send(&shower.read(&shower_id, &buf).unwrap());
        let mut back = vec![0u8; MAX_CEREMONY_FRAME];
        assert!(
            agreed.read_message(&m2, &mut back).is_ok(),
            "the prologue is not `nonce || PROTOCOL_VERSION`"
        );
    }

    /// The nonce is still in the prologue beside the version. Folding a second
    /// thing in must not displace the binding that was already there.
    #[test]
    fn a_wrong_code_still_fails_with_the_version_in_the_prologue() {
        let shower_id = Identity::generate("Ada", 0x11_2233);
        let scanner_id = Identity::generate("Bo", 0x44_5566);
        let shown = Invite::fresh(shower_id.layer2_public(), "a1b2c3d4");
        let other = Invite::fresh(shower_id.layer2_public(), "a1b2c3d4");

        let mut shower = Ceremony::show(&shower_id, &shown).unwrap();
        let (mut scanner, m1) = Ceremony::scan(&scanner_id, &other).unwrap();
        let m2 = one_send(&shower.read(&shower_id, &m1).unwrap());
        assert!(
            scanner.read(&scanner_id, &m2).is_err(),
            "a ceremony ran against a code that was never shown"
        );
    }

    #[test]
    fn a_ceremony_both_people_confirm_pairs_them() {
        let mut h = halfway();
        assert_eq!(h.shower.sas(), h.scanner.sas(), "the two screens disagreed");

        // One tap is not two people agreeing: only a confirmation goes out.
        let from_scanner = h.scanner.confirm(&h.scanner_id).unwrap();
        assert_eq!(sends(&from_scanner).len(), 1);

        let seen = h
            .shower
            .read(&h.shower_id, &sends(&from_scanner)[0])
            .unwrap();
        assert!(matches!(seen[0], Step::PeerConfirmed));
        assert!(
            sends(&seen).is_empty(),
            "a proof went out on one confirmation"
        );

        // The second tap releases the proof, on this side and then the other.
        let from_shower = sends(&h.shower.confirm(&h.shower_id).unwrap());
        assert_eq!(from_shower.len(), 2, "expected a confirmation and a proof");

        let scanner_proof = one_send(&h.scanner.read(&h.scanner_id, &from_shower[0]).unwrap());
        let done = h.scanner.read(&h.scanner_id, &from_shower[1]).unwrap();
        let Step::Paired(scanner_view) = &done[0] else {
            panic!("the scanner did not pair: {done:?}")
        };
        let done = h.shower.read(&h.shower_id, &scanner_proof).unwrap();
        let Step::Paired(shower_view) = &done[0] else {
            panic!("the shower did not pair: {done:?}")
        };

        assert_eq!(scanner_view.l1_pub, h.shower_id.layer1_public());
        assert_eq!(shower_view.l1_pub, h.scanner_id.layer1_public());
        assert_eq!(scanner_view.persona.name, "Ada");
        assert_eq!(shower_view.persona.name, "Bo");
        assert_eq!(scanner_view.sas, shower_view.sas);

        // The root both sides will hang a ratchet off. Equal, or the first
        // message either of them sends is undecryptable by the other; and not
        // all-zeroes, which is what a Diffie-Hellman with a substituted or
        // missing key would tend to produce.
        // Compared, never printed. `assert_eq!` formats both sides on failure,
        // which would put a live ratchet root into the test output and from
        // there into a CI log that outlives the run. The same rule the log
        // lines follow for Bluetooth addresses: a secret that reaches a
        // transcript is a secret for as long as the transcript is kept.
        assert!(
            scanner_view.root == shower_view.root,
            "the two sides derived different ratchet roots"
        );
        assert!(*scanner_view.root != [0u8; 32], "the root is not a secret");
    }

    /// The two ends can actually talk once the ceremony is done.
    ///
    /// Agreeing a root is not the same as having a working ratchet: the two
    /// sides must also take *different* roles, seeded from opposite halves of
    /// the same exchange. Both picking responder is a conversation neither can
    /// start; both picking initiator is two chains that cannot read each other.
    /// Equal roots would pass the test above and still fail here.
    ///
    /// Roles are decided by Layer-1 key order, the same rule the engine uses.
    #[test]
    fn the_paired_sides_seed_ratchets_that_understand_each_other() {
        use crate::session::ratchet::Ratchet;

        let mut h = halfway();
        let from_scanner = sends(&h.scanner.confirm(&h.scanner_id).unwrap());
        h.shower.read(&h.shower_id, &from_scanner[0]).unwrap();
        let from_shower = sends(&h.shower.confirm(&h.shower_id).unwrap());
        let scanner_proof = one_send(&h.scanner.read(&h.scanner_id, &from_shower[0]).unwrap());
        let scanner_done = h.scanner.read(&h.scanner_id, &from_shower[1]).unwrap();
        let shower_done = h.shower.read(&h.shower_id, &scanner_proof).unwrap();
        // Owned, not borrowed: the ratchet secret is deliberately not `Clone`,
        // so seeding a responder means moving it out of the ceremony's result.
        let take = |done: Vec<Step>| match done.into_iter().next() {
            Some(Step::Paired(view)) => *view,
            other => panic!("expected a pairing, got {other:?}"),
        };
        let scanner_view = take(scanner_done);
        let shower_view = take(shower_done);

        // Same rule as `Net::seed_ratchet`: smaller Layer-1 key speaks first.
        let scanner_first = h.scanner_id.layer1_public().0 < h.shower_id.layer1_public().0;
        let (mut speaker, mut listener) = if scanner_first {
            (
                Ratchet::initiator(*scanner_view.root, scanner_view.their_ratchet).unwrap(),
                Ratchet::responder(*shower_view.root, shower_view.ratchet_secret),
            )
        } else {
            (
                Ratchet::initiator(*shower_view.root, shower_view.their_ratchet).unwrap(),
                Ratchet::responder(*scanner_view.root, scanner_view.ratchet_secret),
            )
        };

        let (header, sealed) = speaker.encrypt(b"are you there?").unwrap();
        let opened = listener
            .decrypt(header, &sealed)
            .expect("the other side could not read the first message");
        assert_eq!(&opened[..], b"are you there?");
    }

    /// Two ceremonies do not share a root.
    ///
    /// The property that makes the root worth deriving rather than fixing: it
    /// is fresh per pairing, so compromising one conversation's chain says
    /// nothing about another's. Runs the same two identities twice, so what
    /// varies is only the ephemeral exchange — if the root came from the
    /// long-term keys alone these would match.
    #[test]
    fn each_pairing_gets_its_own_root() {
        fn root_of(shower_id: &Identity, scanner_id: &Identity) -> [u8; 32] {
            let invite = Invite::fresh(shower_id.layer2_public(), "a1b2c3d4");
            let (mut shower, mut scanner) = run_handshake(shower_id, scanner_id, &invite, &invite);
            let from_scanner = sends(&scanner.confirm(scanner_id).unwrap());
            shower.read(shower_id, &from_scanner[0]).unwrap();
            let from_shower = sends(&shower.confirm(shower_id).unwrap());
            scanner.read(scanner_id, &from_shower[0]).unwrap();
            let done = scanner.read(scanner_id, &from_shower[1]).unwrap();
            let Step::Paired(view) = &done[0] else {
                panic!("did not pair: {done:?}")
            };
            *view.root
        }

        let ada = Identity::generate("Ada", 0x11_2233);
        let bo = Identity::generate("Bo", 0x44_5566);
        assert!(
            root_of(&ada, &bo) != root_of(&ada, &bo),
            "the same two people pairing twice reused a root"
        );
    }

    /// R0-F4 as a test: no identity crosses without both confirmations.
    ///
    /// Tech spec §6 orders the Layer-1 exchange before the humans; the
    /// requirement says otherwise, and this asserts the requirement. See the
    /// module header for why that is the resolution.
    #[test]
    fn layer_1_does_not_cross_before_both_confirmations() {
        let mut h = halfway();
        let scanner_l1 = h.scanner_id.layer1_public().0;
        let shower_l1 = h.shower_id.layer1_public().0;

        let mut wire = Vec::new();
        for bytes in sends(&h.scanner.confirm(&h.scanner_id).unwrap()) {
            wire.extend_from_slice(&bytes);
        }
        assert!(
            !contains(&wire, &scanner_l1),
            "our own Layer-1 key went out"
        );
        assert!(!contains(&wire, &shower_l1));

        // And the rule does not rest on the sender keeping to it: a proof that
        // arrives early is refused.
        let mut early = halfway();
        let premature = early.scanner.seal(FrameKind::L1, &[0u8; 8]).unwrap();
        assert!(matches!(
            early.shower.read(&early.shower_id, &premature),
            Err(CeremonyError::OutOfOrder)
        ));
    }

    /// A relay never gets as far as the humans: it does not hold the static the
    /// code names, so message 2 is rejected.
    #[test]
    fn a_relay_is_caught_before_anyone_looks_at_a_screen() {
        let ada = Identity::generate("Ada", 1);
        let bo = Identity::generate("Bo", 2);
        let mallory = Identity::generate("Mallory", 3);
        let adas_code = Invite::fresh(ada.layer2_public(), "ada");

        let mut mallory_side = Ceremony::show(&mallory, &adas_code).unwrap();
        let (mut bo_side, m1) = Ceremony::scan(&bo, &adas_code).unwrap();
        let m2 = one_send(&mallory_side.read(&mallory, &m1).unwrap());

        assert!(
            matches!(
                bo_side.read(&bo, &m2),
                Err(CeremonyError::NotTheCodeWeScanned)
            ),
            "an impostor answering someone else's code was accepted"
        );
    }

    /// Substituting the *code* is the attack the colours exist for: both
    /// handshakes succeed, and the two screens are what disagree.
    #[test]
    fn a_substituted_code_shows_two_different_sas() {
        let ada = Identity::generate("Ada", 1);
        let bo = Identity::generate("Bo", 2);
        let mallory = Identity::generate("Mallory", 3);

        // Bo scans Mallory's code, believing it is Ada's.
        let mallorys_code = Invite::fresh(mallory.layer2_public(), "m");
        let (_, bo_side) = run_handshake(&mallory, &bo, &mallorys_code, &mallorys_code);

        // Mallory meanwhile scans Ada's real code.
        let adas_code = Invite::fresh(ada.layer2_public(), "a");
        let (ada_side, _) = run_handshake(&ada, &mallory, &adas_code, &adas_code);

        assert_ne!(
            bo_side.sas().unwrap(),
            ada_side.sas().unwrap(),
            "a relayed ceremony rendered the same on both screens"
        );
    }

    /// The nonce is the prologue, so a handshake for one code cannot be run
    /// against another. Nothing checks a field — the tag simply fails.
    #[test]
    fn a_ceremony_bound_to_another_code_does_not_open() {
        let ada = Identity::generate("Ada", 1);
        let bo = Identity::generate("Bo", 2);
        let shown = Invite::fresh(ada.layer2_public(), "a");
        // Same key, same hint, different nonce: the code from a moment ago.
        let stale = Invite::fresh(ada.layer2_public(), "a");

        let mut shower = Ceremony::show(&ada, &shown).unwrap();
        let (mut scanner, m1) = Ceremony::scan(&bo, &stale).unwrap();
        let m2 = one_send(&shower.read(&ada, &m1).unwrap());
        assert!(
            matches!(scanner.read(&bo, &m2), Err(CeremonyError::Noise(_))),
            "a ceremony ran against a code it was not bound to"
        );
    }

    /// A Layer-1 key is only worth recording with a signature by that key.
    ///
    /// Tested against the verifier rather than pushed through the channel: a
    /// forged frame has to land on the exact transport nonce the receiver
    /// expects, and arranging that says more about `snow`'s counters than about
    /// this protocol. The end-to-end path is covered by the pairing test above;
    /// what these vary is the three inputs the decision is actually made on.
    #[test]
    fn a_layer_1_proof_signed_by_the_wrong_key_is_refused() {
        let ada = Identity::generate("Ada", 1);
        let mallory = Identity::generate("Mallory", 9);
        let transcript = [0x33u8; 32];

        // Mallory's signature over Ada's key: someone handing over an identity
        // that is not theirs.
        let claimed = ada.layer1_public();
        let ratchet = dh::DhSecret::generate().public();
        let message = l1_proof_message(&transcript, &claimed, &ada.layer2_public(), &ratchet);
        let forged = L1Proof {
            l1_pub: claimed.0.to_vec(),
            signature: mallory.sign_with_layer1(&message).0.to_vec(),
            ratchet_pub: ratchet.0.to_vec(),
        }
        .encode_to_vec();

        assert!(matches!(
            verify_l1_proof(&transcript, &ada.layer2_public(), &forged),
            Err(CeremonyError::L1ProofInvalid)
        ));
        // The honest article, for contrast — otherwise the test above passes
        // just as well against a verifier that refuses everything.
        let honest = L1Proof {
            l1_pub: claimed.0.to_vec(),
            signature: ada.sign_with_layer1(&message).0.to_vec(),
            ratchet_pub: ratchet.0.to_vec(),
        }
        .encode_to_vec();
        assert_eq!(
            verify_l1_proof(&transcript, &ada.layer2_public(), &honest).unwrap(),
            (claimed, ratchet)
        );
    }

    /// A genuine proof is worthless in another ceremony, or beside another
    /// Layer-2 identity: the transcript and the peer key are both signed.
    #[test]
    fn a_proof_does_not_transfer_across_ceremonies_or_identities() {
        let ada = Identity::generate("Ada", 1);
        let transcript = [0x33u8; 32];
        let l1 = ada.layer1_public();
        let ratchet = dh::DhSecret::generate().public();
        let message = l1_proof_message(&transcript, &l1, &ada.layer2_public(), &ratchet);
        let proof = L1Proof {
            l1_pub: l1.0.to_vec(),
            signature: ada.sign_with_layer1(&message).0.to_vec(),
            ratchet_pub: ratchet.0.to_vec(),
        }
        .encode_to_vec();

        // Same proof, a different ceremony.
        assert!(matches!(
            verify_l1_proof(&[0x44u8; 32], &ada.layer2_public(), &proof),
            Err(CeremonyError::L1ProofInvalid)
        ));
        // Same proof, presented as belonging to someone else's Layer-2.
        let other = Identity::generate("Bo", 2);
        assert!(matches!(
            verify_l1_proof(&transcript, &other.layer2_public(), &proof),
            Err(CeremonyError::L1ProofInvalid)
        ));
    }

    /// Swapping the ratchet key invalidates the proof.
    ///
    /// The attack this closes: the Layer-1 key and the signature are left
    /// untouched and only the ephemeral key is replaced, so both people still
    /// see the right identity and the right colours while the attacker holds
    /// the root of every message that follows. Nothing on either screen would
    /// differ. The proof covers the ratchet key precisely so this fails.
    #[test]
    fn a_substituted_ratchet_key_is_refused() {
        let ada = Identity::generate("Ada", 1);
        let transcript = [0x33u8; 32];
        let l1 = ada.layer1_public();
        let honest = dh::DhSecret::generate().public();
        let message = l1_proof_message(&transcript, &l1, &ada.layer2_public(), &honest);
        let signature = ada.sign_with_layer1(&message).0.to_vec();

        let theirs = dh::DhSecret::generate().public();
        assert_ne!(honest.0, theirs.0);
        let tampered = L1Proof {
            l1_pub: l1.0.to_vec(),
            signature: signature.clone(),
            ratchet_pub: theirs.0.to_vec(),
        }
        .encode_to_vec();
        assert!(matches!(
            verify_l1_proof(&transcript, &ada.layer2_public(), &tampered),
            Err(CeremonyError::L1ProofInvalid)
        ));

        // Untampered, so the test above cannot pass against a verifier that
        // refuses everything.
        let intact = L1Proof {
            l1_pub: l1.0.to_vec(),
            signature,
            ratchet_pub: honest.0.to_vec(),
        }
        .encode_to_vec();
        assert_eq!(
            verify_l1_proof(&transcript, &ada.layer2_public(), &intact).unwrap(),
            (l1, honest)
        );
    }

    /// The transcript is part of the root, not just part of the proof.
    ///
    /// Same two ephemeral keys, different ceremony: the roots must differ. What
    /// this buys is that the colours the two people compared — computed over
    /// the transcript — authenticate the root as well as the identities. Using
    /// the Diffie-Hellman output raw would still agree on both sides and still
    /// be secret, so nothing else here would notice its absence.
    #[test]
    fn the_transcript_binds_the_root() {
        let ours = dh::DhSecret::generate();
        let theirs = dh::DhSecret::generate();
        let one = ratchet_root(&ours, &theirs.public(), b"ceremony one").unwrap();
        let other = ratchet_root(&ours, &theirs.public(), b"ceremony two").unwrap();
        assert!(*one != *other, "the transcript did not reach the root");

        // And both sides of one ceremony still agree, which is the point.
        assert!(
            *ratchet_root(&ours, &theirs.public(), b"ceremony one").unwrap()
                == *ratchet_root(&theirs, &ours.public(), b"ceremony one").unwrap(),
            "the two sides of one ceremony disagreed"
        );
    }

    /// Junk, and fields of the wrong size, are refused as malformed rather than
    /// reaching the signature check with a truncated key.
    #[test]
    fn a_malformed_proof_is_refused() {
        let ada = Identity::generate("Ada", 1);
        // The ratchet key is a third field that has to be exactly 32 bytes, and
        // a short one must be refused here rather than reaching the DH.
        for (l1_len, sig_len, ratchet_len) in [
            (31, 64, 32),
            (33, 64, 32),
            (32, 63, 32),
            (0, 64, 32),
            (32, 0, 32),
            (32, 64, 31),
            (32, 64, 0),
        ] {
            let bad = L1Proof {
                l1_pub: vec![0u8; l1_len],
                signature: vec![0u8; sig_len],
                ratchet_pub: vec![0u8; ratchet_len],
            }
            .encode_to_vec();
            assert!(
                matches!(
                    verify_l1_proof(&[0u8; 32], &ada.layer2_public(), &bad),
                    Err(CeremonyError::Malformed)
                ),
                "accepted a proof with a {l1_len}-byte key, a {sig_len}-byte signature \
                 and a {ratchet_len}-byte ratchet key"
            );
        }
    }

    /// Tapping confirm twice is not two people agreeing.
    #[test]
    fn confirming_twice_is_refused() {
        let mut h = halfway();
        h.scanner.confirm(&h.scanner_id).unwrap();
        assert!(matches!(
            h.scanner.confirm(&h.scanner_id),
            Err(CeremonyError::OutOfOrder)
        ));
    }

    /// Nor is a peer repeating its confirmation. Sealed freshly rather than
    /// replayed, so this tests the ceremony's own guard and not Noise's
    /// nonce handling, which would reject a literal replay first.
    #[test]
    fn a_repeated_peer_confirmation_shakes_out_no_second_proof() {
        let mut h = halfway();
        let first = one_send(&h.scanner.confirm(&h.scanner_id).unwrap());
        h.shower.read(&h.shower_id, &first).unwrap();
        let again = h.scanner.seal(FrameKind::Confirm, &[]).unwrap();
        assert!(matches!(
            h.shower.read(&h.shower_id, &again),
            Err(CeremonyError::OutOfOrder)
        ));
    }

    /// The two size checks have to agree about what the limit is.
    ///
    /// The outer one bounds the ciphertext before decrypting; the inner one
    /// bounds the payload after. They are computed from different quantities,
    /// so nothing but a test at the exact edge keeps them consistent — and the
    /// first version of the outer check forgot the kind byte, which made a
    /// maximum-size frame fail the outer test that the inner one allowed.
    #[test]
    fn a_maximum_size_frame_survives_both_length_checks() {
        let mut h = halfway();
        let full = h
            .scanner
            .seal(FrameKind::L1, &vec![0u8; MAX_CEREMONY_FRAME])
            .expect("a full-size frame must be sealable");
        assert!(full.len() <= MAX_CEREMONY_CIPHERTEXT);

        // It reaches the payload check rather than being turned away at the
        // door: an early Layer-1 proof is refused for being early, not for
        // being big.
        assert!(matches!(
            h.shower.read(&h.shower_id, &full),
            Err(CeremonyError::OutOfOrder)
        ));

        // And one byte more is refused, on the way out and on the way in.
        assert!(matches!(
            h.scanner
                .seal(FrameKind::L1, &vec![0u8; MAX_CEREMONY_FRAME + 1]),
            Err(CeremonyError::Malformed)
        ));
        assert!(matches!(
            h.shower
                .read(&h.shower_id, &vec![0u8; MAX_CEREMONY_CIPHERTEXT + 1]),
            Err(CeremonyError::Malformed)
        ));
    }

    /// Nothing works before the handshake finishes.
    #[test]
    fn a_ceremony_that_has_not_opened_yet_does_nothing() {
        let ada = Identity::generate("Ada", 1);
        let invite = Invite::fresh(ada.layer2_public(), "a");
        let mut shower = Ceremony::show(&ada, &invite).unwrap();
        assert!(shower.sas().is_none());
        assert!(matches!(
            shower.confirm(&ada),
            Err(CeremonyError::OutOfOrder)
        ));
    }
}
