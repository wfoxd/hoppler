//! X25519 Diffie–Hellman — session and pseudonym key agreement (tech spec §3, §5).

use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::rng;

pub const SECRET_LEN: usize = 32;
pub const PUBLIC_LEN: usize = 32;
pub const SHARED_LEN: usize = 32;

/// An X25519 secret key. Not printable, zeroized on drop.
///
/// ```compile_fail
/// let s = rust_lib_hoppler::crypto::dh::DhSecret::generate();
/// println!("{:?}", s); // DhSecret must not implement Debug
/// ```
pub struct DhSecret {
    inner: StaticSecret,
}

/// An X25519 public key (safe to share and print).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DhPublic(pub [u8; PUBLIC_LEN]);

/// A DH shared secret. Not printable, zeroized on drop. Feed it to the KDF —
/// never use it directly as a key.
///
/// ```compile_fail
/// let a = rust_lib_hoppler::crypto::dh::DhSecret::generate();
/// let b = rust_lib_hoppler::crypto::dh::DhSecret::generate();
/// let s = a.diffie_hellman(&b.public());
/// println!("{:?}", s); // SharedSecret must not implement Debug
/// ```
pub struct SharedSecret(Zeroizing<[u8; SHARED_LEN]>);

impl DhSecret {
    /// Generate a fresh secret from the OS RNG.
    pub fn generate() -> Self {
        Self::from_bytes(&rng::random_array::<SECRET_LEN>())
    }

    /// Deterministic construction (clamping applied internally); any bytes valid.
    pub fn from_bytes(bytes: &[u8; SECRET_LEN]) -> Self {
        Self {
            inner: StaticSecret::from(*bytes),
        }
    }

    pub fn public(&self) -> DhPublic {
        DhPublic(XPublicKey::from(&self.inner).to_bytes())
    }

    pub fn diffie_hellman(&self, their_public: &DhPublic) -> SharedSecret {
        let shared = self.inner.diffie_hellman(&XPublicKey::from(their_public.0));
        SharedSecret(Zeroizing::new(shared.to_bytes()))
    }
}

impl SharedSecret {
    pub fn as_bytes(&self) -> &[u8; SHARED_LEN] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(s: &str) -> [u8; 32] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    // RFC 7748 §6.1 test vector.
    #[test]
    fn rfc7748_vector() {
        let alice = DhSecret::from_bytes(&hex32(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        let bob = DhSecret::from_bytes(&hex32(
            "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
        ));

        assert_eq!(
            alice.public().0,
            hex32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        );
        assert_eq!(
            bob.public().0,
            hex32("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
        );

        let expected = hex32("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
        assert_eq!(alice.diffie_hellman(&bob.public()).as_bytes(), &expected);
        assert_eq!(bob.diffie_hellman(&alice.public()).as_bytes(), &expected);
    }

    #[test]
    fn dh_commutes_for_random_keys() {
        let a = DhSecret::generate();
        let b = DhSecret::generate();
        assert_eq!(
            a.diffie_hellman(&b.public()).as_bytes(),
            b.diffie_hellman(&a.public()).as_bytes()
        );
    }
}
