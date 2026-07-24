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

/// Hard cap on an untrusted persona record's wire size (parallels
/// `crypto::envelope::MAX_WIRE_LEN`). A record is ~200 bytes; this is slack.
const MAX_PERSONA_RECORD_LEN: usize = 4096;

/// Hard cap on a persona name, in bytes.
const MAX_PERSONA_NAME_LEN: usize = 64;

/// Colour is a packed 0xRRGGBB value; the top byte is not meaningful.
const COLOUR_MASK: u32 = 0x00ff_ffff;

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
            persona: Persona {
                name: name.into(),
                colour,
                version: 1,
            },
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
            persona,
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
    /// round-tripping Layer-1 secret material through the caller. Colour is
    /// normalised to 24 bits.
    pub fn update_persona(&mut self, name: impl Into<String>, colour: u32) {
        self.persona = Persona {
            name: name.into(),
            colour: colour & COLOUR_MASK,
            version: self.persona.version.saturating_add(1),
        };
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
    pub fn persona_record(&self) -> Vec<u8> {
        let body = PersonaBody {
            l2_pub: self.layer2.public().0.to_vec(),
            name: self.persona.name.clone(),
            colour: self.persona.colour,
            version: self.persona.version,
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

    let l2_pub = sign::PublicKey(l2_bytes);
    let signature = sign::Signature(sig_bytes);
    sign::verify(&l2_pub, &signed.body, &signature)
        .map_err(|_| IdentityError::RecordSignatureInvalid)?;

    Ok(VerifiedPersona {
        l2_pub,
        name: body.name,
        colour: body.colour & COLOUR_MASK,
        version: body.version,
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
            "0a2c0a208139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3941202416c18b3c444200112405980c0b297ad871a3d17791c37b3fa3d0c8e59ad256a1b785e19b1aea7a044a7fbbb6d26d2e83217ef9929762ca3d5a1e5420ebb80972fc8d92bb85d94375f00"
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
        assert_eq!(
            verify_persona_record(&vec![0u8; MAX_PERSONA_RECORD_LEN + 1]),
            Err(IdentityError::MalformedRecord)
        );
        // A validly-signed record whose name exceeds the cap is still rejected.
        let id = Identity::generate(&"n".repeat(MAX_PERSONA_NAME_LEN + 1), 0);
        assert_eq!(
            verify_persona_record(&id.persona_record()),
            Err(IdentityError::MalformedRecord)
        );
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
