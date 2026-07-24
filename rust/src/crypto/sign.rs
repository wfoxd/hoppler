//! Ed25519 signatures — identity signing for both layers (tech spec §3).

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use super::{rng, CryptoError};

pub const SEED_LEN: usize = 32;
pub const PUBLIC_KEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

/// An Ed25519 keypair. Holds secret material: not printable, zeroized on drop.
///
/// ```compile_fail
/// let kp = rust_lib_hoppler::crypto::sign::SigningKeyPair::generate();
/// println!("{:?}", kp); // SigningKeyPair must not implement Debug
/// ```
pub struct SigningKeyPair {
    inner: SigningKey,
}

/// An Ed25519 public key (safe to share and print).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PublicKey(pub [u8; PUBLIC_KEY_LEN]);

/// A detached Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature(pub [u8; SIGNATURE_LEN]);

impl SigningKeyPair {
    /// Generate a fresh keypair from the OS RNG.
    pub fn generate() -> Self {
        let seed = Zeroizing::new(rng::random_array::<SEED_LEN>());
        Self::from_seed(&seed)
    }

    /// Deterministic construction from a 32-byte seed (any bytes are valid).
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        Self {
            inner: SigningKey::from_bytes(seed),
        }
    }

    /// The seed, for sealing into the platform keystore (T05). Zeroizes on drop.
    pub fn to_seed(&self) -> Zeroizing<[u8; SEED_LEN]> {
        Zeroizing::new(self.inner.to_bytes())
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.inner.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.inner.sign(message).to_bytes())
    }
}

/// Verify `signature` over `message` by `public`.
///
/// Uses strict verification (rejects malleable/low-order edge cases).
pub fn verify(
    public: &PublicKey,
    message: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    let key = VerifyingKey::from_bytes(&public.0).map_err(|_| CryptoError::InvalidPublicKey)?;
    let sig = ed25519_dalek::Signature::from_bytes(&signature.0);
    key.verify_strict(message, &sig)
        .map_err(|_| CryptoError::SignatureInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(s: &str) -> [u8; 32] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    fn hex64(s: &str) -> [u8; 64] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    // RFC 8032 §7.1 TEST 1 (empty message).
    #[test]
    fn rfc8032_test_1() {
        let kp = SigningKeyPair::from_seed(&hex32(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        ));
        assert_eq!(
            kp.public().0,
            hex32("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        );
        let sig = kp.sign(b"");
        assert_eq!(
            sig.0,
            hex64(
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
            )
        );
        verify(&kp.public(), b"", &sig).unwrap();
    }

    // RFC 8032 §7.1 TEST 2 (one-byte message 0x72).
    #[test]
    fn rfc8032_test_2() {
        let kp = SigningKeyPair::from_seed(&hex32(
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        ));
        assert_eq!(
            kp.public().0,
            hex32("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
        );
        let sig = kp.sign(&[0x72]);
        assert_eq!(
            sig.0,
            hex64(
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"
            )
        );
        verify(&kp.public(), &[0x72], &sig).unwrap();
    }

    #[test]
    fn tampered_signature_rejected() {
        let kp = SigningKeyPair::generate();
        let mut sig = kp.sign(b"hoppler");
        sig.0[0] ^= 0x01;
        assert_eq!(
            verify(&kp.public(), b"hoppler", &sig),
            Err(CryptoError::SignatureInvalid)
        );
    }

    #[test]
    fn wrong_message_rejected() {
        let kp = SigningKeyPair::generate();
        let sig = kp.sign(b"hoppler");
        assert_eq!(
            verify(&kp.public(), b"warren", &sig),
            Err(CryptoError::SignatureInvalid)
        );
    }

    #[test]
    fn wrong_public_key_rejected() {
        let signer = SigningKeyPair::generate();
        let other = SigningKeyPair::generate();
        let sig = signer.sign(b"hoppler");
        // A valid signature must not verify under an unrelated public key.
        assert_eq!(
            verify(&other.public(), b"hoppler", &sig),
            Err(CryptoError::SignatureInvalid)
        );
    }
}
