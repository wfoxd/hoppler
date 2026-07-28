//! X25519 Diffie–Hellman — session and pseudonym key agreement (tech spec §3, §5).

use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::{rng, CryptoError};

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
/// let s = a.diffie_hellman(&b.public()).unwrap();
/// println!("{:?}", s); // SharedSecret must not implement Debug
/// ```
pub struct SharedSecret(Zeroizing<[u8; SHARED_LEN]>);

impl DhSecret {
    /// Generate a fresh secret from the OS RNG.
    pub fn generate() -> Self {
        // Wrap the raw bytes so the temporary is wiped once copied into StaticSecret.
        let seed = Zeroizing::new(rng::random_array::<SECRET_LEN>());
        Self::from_bytes(&seed)
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

    /// The raw scalar, for handing to a vetted Noise implementation that has to
    /// own the key material itself.
    ///
    /// Deliberately `pub(crate)` and deliberately awkward. `DhSecret` has no
    /// `Debug` and no public accessor precisely so a private key cannot end up
    /// in a log or a store by accident; this is the one door, it returns a
    /// `Zeroizing` copy that wipes on drop, and the only caller is
    /// `session::handshake`. Anything else wanting bytes out of a private key
    /// is a bug worth arguing about rather than a call worth adding.
    pub(crate) fn expose_secret(&self) -> Zeroizing<[u8; SECRET_LEN]> {
        Zeroizing::new(self.inner.to_bytes())
    }

    /// Compute the shared secret with `their_public`.
    ///
    /// Rejects non-contributory exchanges: a low-order (small-subgroup) peer
    /// public forces the shared secret to a fixed all-zero value that the peer
    /// knows without holding any secret. Because `crypto/` is the only door to
    /// the primitive, this guard cannot be re-added downstream — so it lives
    /// here, constant-time via `was_contributory`.
    pub fn diffie_hellman(&self, their_public: &DhPublic) -> Result<SharedSecret, CryptoError> {
        let shared = self.inner.diffie_hellman(&XPublicKey::from(their_public.0));
        if !shared.was_contributory() {
            return Err(CryptoError::InvalidPublicKey);
        }
        Ok(SharedSecret(Zeroizing::new(shared.to_bytes())))
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
        assert_eq!(
            alice.diffie_hellman(&bob.public()).unwrap().as_bytes(),
            &expected
        );
        assert_eq!(
            bob.diffie_hellman(&alice.public()).unwrap().as_bytes(),
            &expected
        );
    }

    #[test]
    fn dh_commutes_for_random_keys() {
        let a = DhSecret::generate();
        let b = DhSecret::generate();
        assert_eq!(
            a.diffie_hellman(&b.public()).unwrap().as_bytes(),
            b.diffie_hellman(&a.public()).unwrap().as_bytes()
        );
    }

    #[test]
    fn low_order_public_keys_are_rejected() {
        let secret = DhSecret::generate();
        // Canonical Curve25519 small-order points (all force a non-contributory
        // exchange → all-zero shared secret). From the x25519-dalek test suite.
        let low_order = [
            [0u8; 32],
            {
                let mut p = [0u8; 32];
                p[0] = 1;
                p
            },
            hex32("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
            hex32("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"),
        ];
        for point in low_order {
            // SharedSecret has no Debug, so match rather than unwrap_err.
            assert!(
                matches!(
                    secret.diffie_hellman(&DhPublic(point)),
                    Err(CryptoError::InvalidPublicKey)
                ),
                "low-order point {} must be rejected",
                hex::encode(point)
            );
        }
    }
}
