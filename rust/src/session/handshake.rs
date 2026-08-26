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

use crate::crypto::{dh, sign};
use crate::identity::{self, Identity, VerifiedPersona};
use crate::protocol::PROTOCOL_VERSION;

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
    /// The static Noise proved does not match the persona presented alongside
    /// it. Raised on either side — an initiator replaying someone else's
    /// record, or a responder answering with a persona that is not the one
    /// whose key it used.
    PseudonymMismatch,
    /// A payload larger than [`MAX_HANDSHAKE_PAYLOAD`].
    PayloadTooLarge,
    /// The peer speaks a different protocol version, so the two builds disagree
    /// about what the bytes after this handshake would mean.
    ///
    /// Raised before the persona is even looked at, and it ends the session:
    /// see [`crate::protocol`] for why refusing beats connecting.
    VersionMismatch { theirs: u8, ours: u8 },
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Noise(e) => write!(f, "noise handshake failed: {e}"),
            HandshakeError::Persona(e) => write!(f, "persona in handshake invalid: {e}"),
            HandshakeError::PseudonymMismatch => {
                write!(f, "static key does not match the persona presented with it")
            }
            HandshakeError::PayloadTooLarge => write!(f, "handshake payload too large"),
            HandshakeError::VersionMismatch { theirs, ours } => write!(
                f,
                "peer speaks protocol version {theirs}, this build speaks {ours}"
            ),
        }
    }
}

impl std::error::Error for HandshakeError {}

/// The handshake payload:
/// `[version:1][record_len:2][persona record][signature:64]`.
///
/// The version leads, so it is read before any length is trusted — a peer
/// cannot make us act on a field belonging to a layout we do not speak. See
/// [`crate::protocol`] for why the number is 1 and what a 0 means.
///
/// The signature binds the persona to the static this side is presenting. It
/// has to travel with the record because the verifier cannot derive it: an
/// initiator's static is its pseudonym, from a Layer-1 seed only that device
/// holds. Without the binding a handshake proves that *a* valid persona record
/// exists — and records are public — so anyone who has discovered Alice could
/// present hers and be believed.
///
/// Shared with the F4 pairing ceremony (`crate::pairing::ceremony`), which runs
/// a different Noise pattern against the same problem: prove a persona belongs
/// to the static just used. Deliberately one implementation rather than two —
/// the encoding and its check are a matched pair, and the failure mode of two
/// copies drifting apart is that one side quietly stops catching an
/// impersonation while continuing to look correct.
pub(crate) fn encode_intro(
    us: &Identity,
    our_static: &dh::DhPublic,
) -> Result<Vec<u8>, HandshakeError> {
    let record = us.persona_record();
    if record.len() > MAX_HANDSHAKE_PAYLOAD {
        return Err(HandshakeError::PayloadTooLarge);
    }
    let signature = us.bind_session_static(our_static);
    let mut out = Vec::with_capacity(1 + 2 + record.len() + sign::SIGNATURE_LEN);
    out.push(PROTOCOL_VERSION);
    out.extend_from_slice(&(record.len() as u16).to_be_bytes());
    out.extend_from_slice(&record);
    out.extend_from_slice(&signature.0);
    Ok(out)
}

/// Decode and fully check an intro against the static Noise proved.
pub(crate) fn decode_intro(
    payload: &[u8],
    proven_static: &dh::DhPublic,
) -> Result<VerifiedPersona, HandshakeError> {
    if payload.len() > MAX_HANDSHAKE_PAYLOAD {
        return Err(HandshakeError::PayloadTooLarge);
    }
    // The version first, and before any length is believed. Everything below
    // this line is a layout claim, and a peer that does not speak our version
    // has made no promise about any of it.
    let (&theirs, rest) = payload.split_first().ok_or(HandshakeError::Persona(
        identity::IdentityError::MalformedRecord,
    ))?;
    if theirs != PROTOCOL_VERSION {
        return Err(HandshakeError::VersionMismatch {
            theirs,
            ours: PROTOCOL_VERSION,
        });
    }
    if rest.len() < 2 + sign::SIGNATURE_LEN {
        return Err(HandshakeError::Persona(
            identity::IdentityError::MalformedRecord,
        ));
    }
    let record_len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    if rest.len() != 2 + record_len + sign::SIGNATURE_LEN {
        return Err(HandshakeError::Persona(
            identity::IdentityError::MalformedRecord,
        ));
    }
    let record = &rest[2..2 + record_len];
    let sig_bytes: [u8; sign::SIGNATURE_LEN] = rest[2 + record_len..]
        .try_into()
        .map_err(|_| HandshakeError::Persona(identity::IdentityError::MalformedRecord))?;

    let persona = identity::verify_persona_record(record).map_err(HandshakeError::Persona)?;
    // The record is valid — but so is a copy of anyone else's. This is the
    // check that makes it *theirs*.
    identity::verify_session_binding(&persona.l2_pub, proven_static, &sign::Signature(sig_bytes))
        .map_err(|_| HandshakeError::PseudonymMismatch)?;
    Ok(persona)
}

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

        let payload = encode_intro(us, &our_static.public())?;
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

        let remote_static = remote_static_of(&self.state)?;
        let persona = decode_intro(&buf, &remote_static)?;

        // Belt and braces on the responder's side: its static is *published* in
        // the signed record, so it must also match that. The binding above
        // would catch a substitution anyway; this catches a record whose own
        // published key disagrees with the key it just used.
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

        let remote_static = remote_static_of(&state)?;
        // Binds the persona to the static Noise just proved they hold. Without
        // it an initiator could present a third party's record — records are
        // public — while authenticating with its own key, and we would
        // attribute the whole session to the wrong person.
        let persona = decode_intro(&buf, &remote_static)?;

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
        let payload = encode_intro(us, &us.session_public())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// These live inside the crate rather than in `tests/session.rs` because
    /// they must build a handshake by hand — a forgery the honest API gives no
    /// way to express. The alternative was widening `DhSecret::expose_secret`
    /// to `pub` so an integration test could reach it, which would have traded
    /// a real API guarantee for test convenience.
    fn forged_first_message(us: &Identity, them: &VerifiedPersona, payload: &[u8]) -> Vec<u8> {
        let our_static = us.pseudonym_secret_toward(&them.l2_pub);
        let mut state = Builder::new(params())
            .local_private_key(&*our_static.expose_secret())
            .unwrap()
            .remote_public_key(&them.session_pub.0)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state.write_message(payload, &mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    fn pair() -> (Identity, Identity, VerifiedPersona) {
        let alice = Identity::generate("Alice", 1);
        let bob = Identity::generate("Bob", 2);
        let bob_persona = identity::verify_persona_record(&bob.persona_record()).unwrap();
        (alice, bob, bob_persona)
    }

    /// An intro built the way every build before the version gate built one.
    fn intro_before_the_gate(us: &Identity, our_static: &dh::DhPublic) -> Vec<u8> {
        let record = us.persona_record();
        let signature = us.bind_session_static(our_static);
        let mut out = Vec::new();
        out.extend_from_slice(&(record.len() as u16).to_be_bytes());
        out.extend_from_slice(&record);
        out.extend_from_slice(&signature.0);
        out
    }

    /// The release blocker, as a test.
    ///
    /// A build from before the gate sends `[record_len:2][record][sig]`, and a
    /// persona record is a couple of hundred bytes, so its first byte is zero.
    /// That is why [`PROTOCOL_VERSION`] starts at 1: the older peer is turned
    /// away *and named*, rather than producing some length that happens not to
    /// parse.
    #[test]
    fn a_build_from_before_the_gate_is_refused_and_identified() {
        let (alice, bob, bob_persona) = pair();
        let alice_static = alice.pseudonym_toward(&bob_persona.l2_pub);
        let old = intro_before_the_gate(&alice, &alice_static);
        let message = forged_first_message(&alice, &bob_persona, &old);

        assert!(
            matches!(
                Responder::read_first(&bob, &message),
                Err(HandshakeError::VersionMismatch { theirs: 0, ours: 1 })
            ),
            "an older build was not refused by version"
        );
    }

    /// And a version from the future is refused the same way, rather than being
    /// read as far as it happens to parse.
    #[test]
    fn a_version_we_do_not_speak_is_refused() {
        let (alice, bob, bob_persona) = pair();
        let alice_static = alice.pseudonym_toward(&bob_persona.l2_pub);
        let mut future = encode_intro(&alice, &alice_static).unwrap();
        future[0] = 200;
        let message = forged_first_message(&alice, &bob_persona, &future);

        assert!(matches!(
            Responder::read_first(&bob, &message),
            Err(HandshakeError::VersionMismatch {
                theirs: 200,
                ours: 1
            })
        ));
    }

    /// The version is read before any length is believed.
    ///
    /// A peer that does not speak our version has made no promise about the
    /// bytes behind it, so acting on one of its length fields — even to reject
    /// it — is trusting a layout we have just established we do not share. This
    /// payload is *only* a wrong version; there is nothing behind it at all.
    #[test]
    fn the_version_is_checked_before_the_length() {
        let (_alice, bob, _bob_persona) = pair();
        assert!(matches!(
            decode_intro(&[9], &dh::DhPublic([0; 32])),
            Err(HandshakeError::VersionMismatch { theirs: 9, ours: 1 })
        ));
        let _ = bob;
    }

    /// An empty payload is malformed, not a version mismatch — there is no
    /// version in it to disagree with.
    #[test]
    fn an_empty_intro_is_malformed() {
        assert!(matches!(
            decode_intro(&[], &dh::DhPublic([0; 32])),
            Err(HandshakeError::Persona(
                identity::IdentityError::MalformedRecord
            ))
        ));
    }

    /// Two builds that agree still talk. The gate must refuse the wrong version
    /// without refusing the right one.
    #[test]
    fn matching_versions_still_complete() {
        let (alice, bob, bob_persona) = pair();
        let (initiator, first) = Initiator::start(&alice, &bob_persona).unwrap();
        let pending = Responder::read_first(&bob, &first).unwrap();
        let (_established, reply) = pending.accept(&bob).unwrap();
        assert!(initiator.finish(&reply).is_ok());
    }

    /// The impersonation the binding exists to stop.
    ///
    /// A persona record is public — anyone who has discovered Alice holds hers.
    /// If the handshake only checked that the record *verifies*, Mallory could
    /// present Alice's record while authenticating with her own key, and the
    /// responder would attribute the whole session to Alice.
    ///
    /// The responder cannot catch this by derivation: an initiator's static is
    /// its pseudonym, from a Layer-1 seed only that device holds. So the
    /// binding is asserted by the holder and checked here.
    #[test]
    fn replaying_a_third_partys_persona_record_is_rejected() {
        let (alice, bob, bob_persona) = pair();
        let mallory = Identity::generate("Mallory", 9);
        let alice_record = alice.persona_record();

        let mut payload = Vec::new();
        // A current version, so the forgery is refused for the reason under
        // test rather than turned away at the version gate.
        payload.push(PROTOCOL_VERSION);
        payload.extend_from_slice(&(alice_record.len() as u16).to_be_bytes());
        payload.extend_from_slice(&alice_record);
        payload.extend_from_slice(&[0u8; sign::SIGNATURE_LEN]);
        let forged = forged_first_message(&mallory, &bob_persona, &payload);

        assert!(
            matches!(
                Responder::read_first(&bob, &forged),
                Err(HandshakeError::PseudonymMismatch)
            ),
            "Bob accepted Alice's persona from Mallory's handshake"
        );
    }

    /// Even Mallory's *own* signature over her *own* static cannot carry
    /// Alice's record: the binding is verified against the record's Layer-2
    /// key, not against whoever happened to sign.
    #[test]
    fn a_binding_signed_by_the_wrong_key_is_rejected() {
        let (alice, bob, bob_persona) = pair();
        let mallory = Identity::generate("Mallory", 9);
        let alice_record = alice.persona_record();
        let mallory_static = mallory.pseudonym_toward(&bob_persona.l2_pub);
        let sig = mallory.bind_session_static(&mallory_static);

        let mut payload = Vec::new();
        // A current version, so the forgery is refused for the reason under
        // test rather than turned away at the version gate.
        payload.push(PROTOCOL_VERSION);
        payload.extend_from_slice(&(alice_record.len() as u16).to_be_bytes());
        payload.extend_from_slice(&alice_record);
        payload.extend_from_slice(&sig.0);
        let forged = forged_first_message(&mallory, &bob_persona, &payload);

        assert!(matches!(
            Responder::read_first(&bob, &forged),
            Err(HandshakeError::PseudonymMismatch)
        ));
    }

    /// The cap is this layer's own invariant, not one inherited from whatever
    /// the persona verifier happens to allow.
    #[test]
    fn an_oversized_payload_is_refused_on_read() {
        let (alice, bob, bob_persona) = pair();
        let bloated = forged_first_message(&alice, &bob_persona, &vec![0u8; 8192]);
        assert!(matches!(
            Responder::read_first(&bob, &bloated),
            Err(HandshakeError::PayloadTooLarge)
        ));
    }

    /// A malformed intro must not be read as a short one.
    #[test]
    fn an_intro_whose_lengths_disagree_is_refused() {
        let (alice, bob, bob_persona) = pair();
        for payload in [vec![], vec![0u8; 10], {
            // Declares a 4096-byte record but carries almost nothing.
            let mut p = vec![0x10, 0x00];
            p.extend_from_slice(&[0u8; sign::SIGNATURE_LEN]);
            p
        }] {
            let msg = forged_first_message(&alice, &bob_persona, &payload);
            assert!(
                Responder::read_first(&bob, &msg).is_err(),
                "accepted an intro of {} bytes",
                payload.len()
            );
        }
    }
}
