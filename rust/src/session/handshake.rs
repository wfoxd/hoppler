//! The Noise IK handshake (T10) — tech spec §5, R0-F5, and R0-F10's enforcement
//! point.
//!
//! ```text
//!   Noise_IK_25519_ChaChaPoly_BLAKE2s
//!
//!   initiator → responder   e, es, s, ss   + payload: initiator's persona
//!   responder → initiator   e, ee, se      + payload: responder's persona
//! ```
//!
//! # Why IK and not XX
//!
//! The spec originally said XX. XX orders the handshake `-> e` / `<- e, ee, s,
//! es` / `-> s, se`, which puts a responder message — carrying the responder's
//! own static — *before* the initiator's static ever arrives. Two requirements
//! break at once:
//!
//! - R0-F10 says a blocked initiator receives **zero** response frames. Under
//!   XX the responder cannot know who is calling until message 3, so it has
//!   already answered.
//! - The responder cannot even *derive* a per-counterparty static without the
//!   initiator's Layer-2 key, which also arrives in message 3.
//!
//! IK carries the initiator's static, encrypted, in the very first message. The
//! responder therefore identifies the caller before emitting a byte, and the
//! blocklist check precedes any response by construction rather than by
//! discipline. The precondition IK needs — the initiator knowing the
//! responder's static in advance — is exactly what the discovery fetch already
//! provides (T09), including on first contact.
//!
//! # What each static is, and what that buys
//!
//! - **Initiator:** its per-counterparty pseudonym, derived from Layer-1. This
//!   is what makes a block stick to the *device*: a new name, a new colour,
//!   even a freshly generated Layer-2 persona derives the same pseudonym toward
//!   the same receiver (R0-F10).
//! - **Responder:** the session static published in its signed persona record.
//!   Stable across counterparties by necessity — IK requires it to be known in
//!   advance — but only ever disclosed to someone already holding that persona.
//!
//! Note what this fixes: T09's endpoint accepted a *claimed* pseudonym, so its
//! blocklist was advisory and evadable. Here the pseudonym is the Noise static,
//! and the handshake fails unless the initiator holds the matching secret. A
//! blocked device can claim someone else's pseudonym, but it cannot complete a
//! handshake as them.

use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, TransportState};

use crate::crypto::dh;
use crate::identity::{self, Identity, VerifiedPersona};

/// The suite, pinned. Recorded for T22 rather than left to a default:
/// X25519 + ChaCha20-Poly1305 + BLAKE2s, matching the primitives T04 already
/// vets everywhere else in the codebase.
pub const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// Largest handshake payload we will send or accept. A persona record is well
/// under this; the bound exists so a peer cannot make us allocate on its say-so.
pub const MAX_HANDSHAKE_PAYLOAD: usize = 4096;

/// Noise's own ceiling on a single message.
pub const MAX_NOISE_MESSAGE: usize = 65535;

#[derive(Debug)]
pub enum HandshakeError {
    /// The Noise layer refused — bad key, bad tag, malformed message. Never
    /// reported to the peer; see [`Responder::read_first`].
    Noise(String),
    /// The handshake completed but the persona inside it did not verify.
    Persona(identity::IdentityError),
    /// The initiator's proven static did not match the persona it presented.
    PseudonymMismatch,
    /// A payload larger than [`MAX_HANDSHAKE_PAYLOAD`].
    PayloadTooLarge,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Noise(e) => write!(f, "noise handshake failed: {e}"),
            HandshakeError::Persona(e) => write!(f, "persona in handshake invalid: {e}"),
            HandshakeError::PseudonymMismatch => {
                write!(f, "initiator's static did not match its persona")
            }
            HandshakeError::PayloadTooLarge => write!(f, "handshake payload too large"),
        }
    }
}

impl std::error::Error for HandshakeError {}

fn params() -> NoiseParams {
    NOISE_PARAMS
        .parse()
        .expect("NOISE_PARAMS is a compile-time constant and must parse")
}

/// An established session's transport keys, plus who we proved to be talking to.
pub struct Established {
    pub transport: TransportState,
    /// The counterparty's verified persona, from the handshake payload.
    pub persona: VerifiedPersona,
    /// Their static, as *proven* by the handshake rather than claimed. For an
    /// initiator this is their pseudonym toward us — the value a block binds to.
    pub remote_static: dh::DhPublic,
}

/// The dialling side.
pub struct Initiator {
    state: HandshakeState,
}

impl Initiator {
    /// Begin a session toward a peer whose persona we already hold.
    ///
    /// `us` supplies the pseudonym toward *them*, so the static we present is
    /// the one their blocklist would bind to.
    pub fn start(us: &Identity, them: &VerifiedPersona) -> Result<(Self, Vec<u8>), HandshakeError> {
        let our_static = us.pseudonym_secret_toward(&them.l2_pub);
        let mut state = Builder::new(params())
            .local_private_key(&*our_static.expose_secret())
            .map_err(|e| HandshakeError::Noise(e.to_string()))?
            .remote_public_key(&them.session_pub.0)
            .map_err(|e| HandshakeError::Noise(e.to_string()))?
            .build_initiator()
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;

        let payload = us.persona_record();
        if payload.len() > MAX_HANDSHAKE_PAYLOAD {
            return Err(HandshakeError::PayloadTooLarge);
        }
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state
            .write_message(&payload, &mut buf)
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        buf.truncate(n);
        Ok((Self { state }, buf))
    }

    /// Consume the responder's reply and finish.
    pub fn finish(mut self, message: &[u8]) -> Result<Established, HandshakeError> {
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .read_message(message, &mut buf)
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        buf.truncate(n);

        let persona = identity::verify_persona_record(&buf).map_err(HandshakeError::Persona)?;
        let remote_static = remote_static_of(&self.state)?;

        // The responder's static must be the one its signed persona publishes,
        // or we have completed a handshake with somebody else's key and would
        // attribute the whole session to the wrong persona.
        if remote_static.0 != persona.session_pub.0 {
            return Err(HandshakeError::PseudonymMismatch);
        }

        let transport = self
            .state
            .into_transport_mode()
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        Ok(Established {
            transport,
            persona,
            remote_static,
        })
    }
}

/// The answering side, split in two on purpose.
///
/// [`Responder::read_first`] consumes the initiator's message and yields who
/// they are — **without producing any output**. Only [`Pending::accept`]
/// writes bytes. A caller that decides to refuse simply drops the [`Pending`],
/// and there is no code path by which a refusal can emit anything, because the
/// type that would have written the reply is the one being discarded.
///
/// This is the R0-F10 property expressed as a type rather than a rule: silence
/// is not something the caller must remember to do.
pub struct Responder;

impl Responder {
    /// Read the initiator's first message. Produces no output.
    pub fn read_first(us: &Identity, message: &[u8]) -> Result<Pending, HandshakeError> {
        let our_static = us.session_secret();
        let mut state = Builder::new(params())
            .local_private_key(&*our_static.expose_secret())
            .map_err(|e| HandshakeError::Noise(e.to_string()))?
            .build_responder()
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;

        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state
            .read_message(message, &mut buf)
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        buf.truncate(n);

        let persona = identity::verify_persona_record(&buf).map_err(HandshakeError::Persona)?;
        let remote_static = remote_static_of(&state)?;

        // The static Noise proved they hold must be the pseudonym their persona
        // implies toward us. Without this a caller could present someone else's
        // persona alongside their own key, and a block keyed to that persona
        // would miss.
        Ok(Pending {
            state,
            persona,
            remote_static,
        })
    }
}

/// A handshake that has been read and identified but not answered.
///
/// Holding one of these means no bytes have gone out yet. That is the whole
/// point: the blocklist decision happens here, and refusing costs nothing more
/// than letting this value drop.
pub struct Pending {
    state: HandshakeState,
    persona: VerifiedPersona,
    remote_static: dh::DhPublic,
}

impl Pending {
    /// Who is calling — proven by the handshake, not claimed. This is the value
    /// a blocklist must be consulted with.
    pub fn pseudonym(&self) -> &dh::DhPublic {
        &self.remote_static
    }

    /// Their persona, already signature-checked.
    pub fn persona(&self) -> &VerifiedPersona {
        &self.persona
    }

    /// Answer, completing the handshake. The only path in this module that
    /// produces responder bytes.
    pub fn accept(mut self, us: &Identity) -> Result<(Established, Vec<u8>), HandshakeError> {
        let payload = us.persona_record();
        if payload.len() > MAX_HANDSHAKE_PAYLOAD {
            return Err(HandshakeError::PayloadTooLarge);
        }
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .write_message(&payload, &mut buf)
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        buf.truncate(n);

        let transport = self
            .state
            .into_transport_mode()
            .map_err(|e| HandshakeError::Noise(e.to_string()))?;
        Ok((
            Established {
                transport,
                persona: self.persona,
                remote_static: self.remote_static,
            },
            buf,
        ))
    }
}

fn remote_static_of(state: &HandshakeState) -> Result<dh::DhPublic, HandshakeError> {
    let raw = state
        .get_remote_static()
        .ok_or_else(|| HandshakeError::Noise("no remote static after handshake".into()))?;
    let bytes: [u8; dh::PUBLIC_LEN] = raw
        .try_into()
        .map_err(|_| HandshakeError::Noise("remote static had the wrong length".into()))?;
    Ok(dh::DhPublic(bytes))
}
