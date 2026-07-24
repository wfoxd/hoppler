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
//! Known limit (Ring 0 has no rotation flow, so it is latent): the pseudonym a
//! peer presents to us is keyed on *our* Layer-2 public, so if we ever rotate
//! our own Layer-2, every inbound pseudonym changes and a prior block stops
//! matching. Any future Layer-2 rotation must therefore migrate the block list.

pub mod keystore;

use prost::Message;
use zeroize::Zeroizing;

use crate::crypto::{dh, kdf, sign};
use crate::proto::v0::{PersonaBody, SignedPersona};

/// Domain-separation label for the pseudonym KDF. Versioned: changing it is a
/// wire-protocol break, so it is frozen for compatible builds.
const PSEUDONYM_LABEL: &[u8] = b"hoppler/pseudonym/v1";

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
    /// Derivation: `HKDF(ikm = our Layer-1 seed, salt = label, info = their
    /// Layer-2 public)`. The label domain-separates this use of the seed; the
    /// counterpart key makes it per-counterparty.
    pub fn pseudonym_secret_toward(&self, counterpart_l2: &sign::PublicKey) -> dh::DhSecret {
        let seed = self.layer1.to_seed();
        let scalar = kdf::derive_32(&*seed, PSEUDONYM_LABEL, &counterpart_l2.0);
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
    let signed = SignedPersona::decode(wire).map_err(|_| IdentityError::MalformedRecord)?;
    let body =
        PersonaBody::decode(signed.body.as_slice()).map_err(|_| IdentityError::MalformedRecord)?;

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
        colour: body.colour,
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
