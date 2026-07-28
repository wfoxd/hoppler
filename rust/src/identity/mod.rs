//! Two-layer identity (tech spec §3; requirements R0-F1 and R0-F10).
//!
//! Layer 1 is the private device identity; its seed lives in the platform
//! keystore ([`keystore`]) and never serialises except into the pairing
//! ceremony channel. Layer 2 is the public persona shown in Discovery.
//!
//! The per-counterparty **pseudonym** — the handle a block binds to — is
//! derived from *our* Layer-1 secret and *their* Layer-2 public key. That
//! construction gives it three properties the block guarantee depends on:
//! stable toward a given counterpart, unlinkable across counterparts, and
//! unchanged when *we* rotate our own Layer-2.
//!
//! Known limits (all inherent to deterministic pseudonyms; Ring 0 has no
//! rotation flow, so they are latent):
//! - Rotating *our own* Layer-2 changes every inbound pseudonym a peer presents
//!   to us, so a prior block stops matching — a future Layer-2 rotation must
//!   migrate the block list.
//! - Because pseudonyms are a pure function of our Layer-1 and public
//!   counterpart Layer-2 keys, a blocked user who rotates Layer-1 gets fresh
//!   pseudonyms everywhere (block evasion), and a Layer-1 compromise lets an
//!   attacker recompute every past and future pseudonym (retroactive linkage).
//!   These are the accepted price of block *persistence* (R0-F10).
//!
//! Notes for downstream tasks:
//! - **Blocks bind to the pseudonym, never the persona.** A persona record is
//!   public and replayable, so anyone can rebroadcast our name/colour paired
//!   with a pseudonym from *their* Layer-1. §5 pairing MUST prove possession of
//!   the Layer-2 private key and bind that key to the pseudonym it will later
//!   recognise; block lists key on the (robust) pseudonym, not the (spoofable)
//!   persona.
//! - Persona-record bytes are non-canonical (proto3: unknown/duplicate fields,
//!   non-minimal varints). Verification is still sound — the signature covers
//!   the exact carried bytes — but downstream MUST key off `verified.l2_pub`,
//!   never raw record bytes, as a cache/dedup/identity key.
//! - The pseudonym secret is the Noise static key (§5). Its DH uses MUST route
//!   through `crypto::dh::diffie_hellman` so the contributory check rejects a
//!   malicious low-order peer point; `DhSecret` deliberately has no byte export,
//!   so §5 hand-rolls Noise on `crypto::dh` (consistent with crypto confinement).

pub mod keystore;

use prost::Message;
use zeroize::Zeroizing;

use crate::crypto::{dh, kdf, sign};
use crate::proto::v0::{PersonaBody, SignedPersona};

/// Domain-separation label for the pseudonym KDF. Versioned: changing it is a
/// wire-protocol break, so it is frozen for compatible builds.
const PSEUDONYM_LABEL: &[u8] = b"hoppler/pseudonym/v1";
/// Label for the Noise IK static this device answers on.
///
/// Derived rather than converted from the Ed25519 signing key, so no one key is
/// used for both signing and Diffie-Hellman.
///
/// The label is belt-and-braces, not the actual separation: this derives from
/// the *Layer-2* seed salted with the Layer-2 public, while a pseudonym derives
/// from the *Layer-1* seed salted with a counterpart's key, so the two cannot
/// collide even under one label. Said plainly because a mutation test showed
/// swapping this label for `PSEUDONYM_LABEL` changes nothing observable — the
/// comment claiming the label prevented collisions was overstating it.
const SESSION_LABEL: &[u8] = b"hoppler/session-static/v1";

/// Hard cap on an untrusted persona record's wire size (parallels
/// `crypto::envelope::MAX_WIRE_LEN`). A record is ~200 bytes; this is slack.
const MAX_PERSONA_RECORD_LEN: usize = 4096;

/// Hard cap on a persona name, in bytes.
const MAX_PERSONA_NAME_LEN: usize = 64;

/// Colour is a packed 0xRRGGBB value; the top byte is not meaningful.
const COLOUR_MASK: u32 = 0x00ff_ffff;

/// Normalise persona fields so a locally-held persona always satisfies the same
/// bounds `verify_persona_record` enforces on peers — colour masked to 24 bits,
/// name truncated at a UTF-8 boundary to the byte cap. This makes an
/// unverifiable local persona unrepresentable: `persona_record()` always
/// verifies. Applied at every point a persona is set.
fn normalise_persona(name: impl Into<String>, colour: u32, version: u32) -> Persona {
    let mut name = name.into();
    if name.len() > MAX_PERSONA_NAME_LEN {
        let mut end = MAX_PERSONA_NAME_LEN;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    Persona {
        name,
        colour: colour & COLOUR_MASK,
        version,
    }
}

/// The mutable, public-facing half of an identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Persona {
    pub name: String,
    /// Packed 0xRRGGBB.
    pub colour: u32,
    pub version: u32,
}

/// A full local identity: both key layers plus the persona.
///
/// Holds Layer-1 secret material, so it is deliberately not printable and its
/// seeds zeroize on drop (via the underlying key pairs).
///
/// ```compile_fail
/// let id = rust_lib_hoppler::identity::Identity::generate("a", 0);
/// println!("{:?}", id); // Identity must not implement Debug
/// ```
pub struct Identity {
    layer1: sign::SigningKeyPair,
    layer2: sign::SigningKeyPair,
    persona: Persona,
}

/// A persona record decoded from the wire and verified against its own
/// embedded Layer-2 key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPersona {
    pub l2_pub: sign::PublicKey,
    pub name: String,
    pub colour: u32,
    pub version: u32,
    /// The X25519 static to open a Noise IK session toward this device.
    /// Signed alongside the rest, so it cannot be swapped in flight.
    pub session_pub: dh::DhPublic,
}

/// Errors from persona-record verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    /// The record did not parse, or a key/signature field had the wrong length.
    MalformedRecord,
    /// The record parsed but its self-signature did not verify.
    RecordSignatureInvalid,
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::MalformedRecord => write!(f, "malformed persona record"),
            IdentityError::RecordSignatureInvalid => write!(f, "persona record signature invalid"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl Identity {
    /// Generate a fresh identity locally. The only external call is the OS
    /// CSPRNG (via [`crate::crypto::rng`]); there is no network or disk I/O,
    /// which is what makes F1's zero-network requirement hold by construction.
    pub fn generate(name: impl Into<String>, colour: u32) -> Self {
        Self {
            layer1: sign::SigningKeyPair::generate(),
            layer2: sign::SigningKeyPair::generate(),
            persona: normalise_persona(name, colour, 1),
        }
    }

    /// Reconstruct from seeds unsealed from the keystore plus the stored persona.
    pub fn from_parts(
        layer1_seed: &[u8; sign::SEED_LEN],
        layer2_seed: &[u8; sign::SEED_LEN],
        persona: Persona,
    ) -> Self {
        Self {
            layer1: sign::SigningKeyPair::from_seed(layer1_seed),
            layer2: sign::SigningKeyPair::from_seed(layer2_seed),
            persona: normalise_persona(persona.name, persona.colour, persona.version),
        }
    }

    pub fn layer1_public(&self) -> sign::PublicKey {
        self.layer1.public()
    }

    pub fn layer2_public(&self) -> sign::PublicKey {
        self.layer2.public()
    }

    pub fn persona(&self) -> &Persona {
        &self.persona
    }

    /// Replace the displayed persona and bump its version. The Layer-2 key is
    /// unchanged, so pairings and pseudonyms survive — only the name/colour
    /// shown in Discovery change. This is T07's edit-persona path; it avoids
    /// round-tripping Layer-1 secret material through the caller. Name and
    /// colour are normalised to the persona bounds (see `normalise_persona`).
    pub fn update_persona(&mut self, name: impl Into<String>, colour: u32) {
        self.persona = normalise_persona(name, colour, self.persona.version.saturating_add(1));
    }

    /// Layer-1 seed, for sealing into the keystore. Never serialise it anywhere
    /// else — this is the one exception to "Layer-1 stays on device".
    pub fn layer1_seed(&self) -> Zeroizing<[u8; sign::SEED_LEN]> {
        self.layer1.to_seed()
    }

    /// Layer-2 seed, for sealing into the keystore.
    pub fn layer2_seed(&self) -> Zeroizing<[u8; sign::SEED_LEN]> {
        self.layer2.to_seed()
    }

    /// Encode a self-signed persona record for Discovery and unpaired Pings.
    /// The X25519 secret this device answers Noise IK on (tech spec §5).
    ///
    /// Derived from the Layer-2 seed under its own label. Stable across
    /// counterparties by necessity — IK requires the initiator to know it in
    /// advance — which is why it is the *responder's* side only. The
    /// initiator's static stays [`Self::pseudonym_toward`], so a block still
    /// binds to Layer-1 and survives a regenerated persona (R0-F10).
    pub fn session_secret(&self) -> dh::DhSecret {
        let seed = self.layer2.to_seed();
        let scalar = kdf::derive_32(&*seed, &self.layer2.public().0, SESSION_LABEL);
        dh::DhSecret::from_bytes(&scalar)
    }

    /// The public half, published in the persona record.
    pub fn session_public(&self) -> dh::DhPublic {
        self.session_secret().public()
    }

    pub fn persona_record(&self) -> Vec<u8> {
        let body = PersonaBody {
            l2_pub: self.layer2.public().0.to_vec(),
            name: self.persona.name.clone(),
            colour: self.persona.colour,
            version: self.persona.version,
            session_pub: self.session_public().0.to_vec(),
        };
        let body_bytes = body.encode_to_vec();
        let signature = self.layer2.sign(&body_bytes).0.to_vec();
        SignedPersona {
            body: body_bytes,
            signature,
        }
        .encode_to_vec()
    }

    /// The private pseudonym secret toward a counterpart — the Noise static key
    /// for a session with them (tech spec §5).
    ///
    /// Derivation: `HKDF(ikm = our Layer-1 seed, salt = their Layer-2 public,
    /// info = label)`. The counterpart key (salt) makes it per-counterparty;
    /// the versioned label (info) domain-separates this use of the seed —
    /// matching `kdf`'s convention that `info` carries the label.
    pub fn pseudonym_secret_toward(&self, counterpart_l2: &sign::PublicKey) -> dh::DhSecret {
        let seed = self.layer1.to_seed();
        let scalar = kdf::derive_32(&*seed, &counterpart_l2.0, PSEUDONYM_LABEL);
        dh::DhSecret::from_bytes(&scalar)
    }

    /// The stable public pseudonym we present to a counterpart — what their
    /// block list binds to (R0-F10). Stable toward them, unlinkable across
    /// counterparts, and unchanged if we rotate our own Layer-2.
    pub fn pseudonym_toward(&self, counterpart_l2: &sign::PublicKey) -> dh::DhPublic {
        self.pseudonym_secret_toward(counterpart_l2).public()
    }
}

/// Verify a persona record against its own embedded Layer-2 key. Returns the
/// verified fields, or an error if the record is malformed or the signature
/// does not check out. A verified record proves integrity and that `l2_pub`
/// controls this persona — not that the persona belongs to any particular
/// person (identity is the key, by design).
pub fn verify_persona_record(wire: &[u8]) -> Result<VerifiedPersona, IdentityError> {
    // Bound untrusted input before parsing, matching envelope::decode's
    // discipline — records arrive from unpaired peers over Discovery/Pings.
    if wire.len() > MAX_PERSONA_RECORD_LEN {
        return Err(IdentityError::MalformedRecord);
    }
    let signed = SignedPersona::decode(wire).map_err(|_| IdentityError::MalformedRecord)?;
    let body =
        PersonaBody::decode(signed.body.as_slice()).map_err(|_| IdentityError::MalformedRecord)?;

    if body.name.len() > MAX_PERSONA_NAME_LEN {
        return Err(IdentityError::MalformedRecord);
    }

    let l2_bytes: [u8; sign::PUBLIC_KEY_LEN] = body
        .l2_pub
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::MalformedRecord)?;
    let sig_bytes: [u8; sign::SIGNATURE_LEN] = signed
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::MalformedRecord)?;

    // Length-checked before the signature, like every other field: a record
    // that verifies but carries a truncated session key would be accepted and
    // then fail deep inside a handshake, where the cause is far less obvious.
    let session_bytes: [u8; dh::PUBLIC_LEN] = body
        .session_pub
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::MalformedRecord)?;

    let l2_pub = sign::PublicKey(l2_bytes);
    let signature = sign::Signature(sig_bytes);
    sign::verify(&l2_pub, &signed.body, &signature)
        .map_err(|_| IdentityError::RecordSignatureInvalid)?;

    Ok(VerifiedPersona {
        l2_pub,
        name: body.name,
        colour: body.colour & COLOUR_MASK,
        version: body.version,
        session_pub: dh::DhPublic(session_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_two_distinct_layers() {
        let id = Identity::generate("Alice", 0xff8800);
        assert_ne!(id.layer1_public().0, id.layer2_public().0);
        assert_eq!(id.persona().name, "Alice");
        assert_eq!(id.persona().version, 1);
    }

    #[test]
    fn persona_record_round_trips_and_verifies() {
        let id = Identity::generate("Alice", 0xff8800);
        let record = id.persona_record();
        let verified = verify_persona_record(&record).unwrap();
        assert_eq!(verified.l2_pub.0, id.layer2_public().0);
        assert_eq!(verified.name, "Alice");
        assert_eq!(verified.colour, 0xff8800);
        assert_eq!(verified.version, 1);
    }

    #[test]
    fn tampered_record_rejected() {
        let id = Identity::generate("Alice", 1);
        let record = id.persona_record();

        // Flipping any single byte must break parse or signature — never verify.
        for i in 0..record.len() {
            let mut bad = record.clone();
            bad[i] ^= 0x01;
            assert!(
                verify_persona_record(&bad).is_err(),
                "tamper at byte {i} verified unexpectedly"
            );
        }
    }

    #[test]
    fn foreign_signature_rejected() {
        // A record whose body claims one key but is signed by another must fail.
        let alice = Identity::generate("Alice", 1);
        let mallory = Identity::generate("Mallory", 2);
        let body = PersonaBody {
            l2_pub: alice.layer2_public().0.to_vec(),
            name: "Alice".into(),
            colour: 1,
            version: 1,
            session_pub: alice.session_public().0.to_vec(),
        };
        let body_bytes = body.encode_to_vec();
        let forged = SignedPersona {
            signature: mallory.layer2.sign(&body_bytes).0.to_vec(),
            body: body_bytes,
        }
        .encode_to_vec();
        assert_eq!(
            verify_persona_record(&forged),
            Err(IdentityError::RecordSignatureInvalid)
        );
    }

    #[test]
    fn pseudonym_is_stable_toward_a_counterpart() {
        let me = Identity::generate("me", 0);
        let peer = Identity::generate("peer", 0).layer2_public();
        assert_eq!(me.pseudonym_toward(&peer).0, me.pseudonym_toward(&peer).0);
    }

    #[test]
    fn pseudonym_differs_across_counterparts() {
        let me = Identity::generate("me", 0);
        let bob = Identity::generate("bob", 0).layer2_public();
        let carol = Identity::generate("carol", 0).layer2_public();
        assert_ne!(me.pseudonym_toward(&bob).0, me.pseudonym_toward(&carol).0);
    }

    #[test]
    fn pseudonym_golden_vector() {
        // Freezes the exact derivation (KDF arg order + label). A silent change
        // to the wire-visible pseudonym — or the block guarantee's stability
        // across versions — fails here, where the relational tests would not.
        let me = Identity::from_parts(
            &[1u8; 32],
            &[9u8; 32],
            Persona {
                name: "x".into(),
                colour: 0,
                version: 1,
            },
        );
        let peer = sign::SigningKeyPair::from_seed(&[2u8; 32]).public();
        assert_eq!(
            hex::encode(me.pseudonym_toward(&peer).0),
            "7d34ee21203bd1b041db35ff34f9eaf579fec233b74242ceb6ef0eb3b31c846d"
        );
    }

    #[test]
    fn persona_record_golden_bytes() {
        // Ed25519 signatures are deterministic, so a fixed identity + fields
        // yields fixed record bytes — guards field numbers and the signing
        // input against drift with the generated Dart types.
        //
        // Updated once, deliberately, when T10 added `session_pub` (field 5)
        // for the Noise IK static. The record is a signed wire format, so a
        // change here is a compatibility break by definition; this test failing
        // is the intended way to find that out, and the value below was
        // regenerated rather than the assertion relaxed.
        let id = Identity::from_parts(
            &[1u8; 32],
            &[2u8; 32],
            Persona {
                name: "Al".into(),
                colour: 0x11_2233,
                version: 1,
            },
        );
        assert_eq!(
            hex::encode(id.persona_record()),
            "0a4e0a208139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3941202416c18b3c44420012a205d68c2494c6dac04b0b0c1ca0d2428f33238e9758dc813b2fe3ba648ffc1440b12404dce095dae543983530dcf9ac2480a87abaa9d5df3c34f51bc4c0fc21c6ab24e27a0432949f5daf6c80189b3ebc81a44710016da2dfe99417c4a84ad043dbf0e"
        );
    }

    #[test]
    fn pseudonym_secret_completes_a_dh() {
        // Exercises the secret half directly (T10's Noise static-key path):
        // it must be a usable X25519 secret whose DH commutes.
        let me = Identity::generate("me", 0);
        let peer = Identity::generate("peer", 0).layer2_public();
        let my_secret = me.pseudonym_secret_toward(&peer);
        let other = dh::DhSecret::generate();
        let s1 = my_secret.diffie_hellman(&other.public()).unwrap();
        let s2 = other.diffie_hellman(&my_secret.public()).unwrap();
        assert_eq!(s1.as_bytes(), s2.as_bytes());
    }

    #[test]
    fn update_persona_bumps_version_masks_colour_keeps_l2() {
        let mut id = Identity::generate("Alice", 1);
        let l2_before = id.layer2_public().0;
        id.update_persona("Alicia", 0xAA_BBCCDD);
        assert_eq!(id.persona().name, "Alicia");
        assert_eq!(id.persona().colour, 0x00_BBCCDD); // top byte masked off
        assert_eq!(id.persona().version, 2);
        assert_eq!(id.layer2_public().0, l2_before); // key unchanged: pairings survive
    }

    #[test]
    fn oversized_and_overlong_records_rejected() {
        // Oversized wire.
        assert_eq!(
            verify_persona_record(&vec![0u8; MAX_PERSONA_RECORD_LEN + 1]),
            Err(IdentityError::MalformedRecord)
        );
        // A validly-signed record from a peer that did NOT normalise its name
        // (over the byte cap) is rejected on the read side.
        let signer = sign::SigningKeyPair::generate();
        let body = PersonaBody {
            l2_pub: signer.public().0.to_vec(),
            name: "n".repeat(MAX_PERSONA_NAME_LEN + 1),
            colour: 0,
            version: 1,
            session_pub: vec![0u8; dh::PUBLIC_LEN],
        };
        let body_bytes = body.encode_to_vec();
        let record = SignedPersona {
            signature: signer.sign(&body_bytes).0.to_vec(),
            body: body_bytes,
        }
        .encode_to_vec();
        assert_eq!(
            verify_persona_record(&record),
            Err(IdentityError::MalformedRecord)
        );
    }

    #[test]
    fn our_own_record_always_verifies_despite_extreme_input() {
        // Normalisation at creation makes an unverifiable local persona
        // unrepresentable: even absurd input yields a record we can verify.
        let id = Identity::generate(&"x".repeat(500), 0xffff_ffff);
        let verified = verify_persona_record(&id.persona_record()).unwrap();
        assert!(verified.name.len() <= MAX_PERSONA_NAME_LEN);
        assert_eq!(verified.colour, 0x00ff_ffff);
    }

    #[test]
    fn name_truncation_respects_utf8_boundaries() {
        // A multi-byte char straddling the cap is dropped whole, never split.
        let id = Identity::generate("é".repeat(40), 0); // 'é' = 2 bytes -> 80 bytes
        assert!(id.persona().name.len() <= MAX_PERSONA_NAME_LEN);
        assert!(id.persona().name.chars().all(|c| c == 'é'));
    }

    #[test]
    fn outbound_pseudonym_survives_own_layer2_rotation() {
        // Same Layer-1, different Layer-2 (a rotation of our own persona key):
        // our pseudonym toward a fixed counterpart must not change.
        let l1 = sign::SigningKeyPair::generate().to_seed();
        let peer = Identity::generate("peer", 0).layer2_public();

        let before = Identity::from_parts(
            &l1,
            &sign::SigningKeyPair::generate().to_seed(),
            Persona {
                name: "v1".into(),
                colour: 0,
                version: 1,
            },
        );
        let after = Identity::from_parts(
            &l1,
            &sign::SigningKeyPair::generate().to_seed(),
            Persona {
                name: "v2".into(),
                colour: 0,
                version: 2,
            },
        );
        assert_eq!(
            before.pseudonym_toward(&peer).0,
            after.pseudonym_toward(&peer).0
        );
    }
}
