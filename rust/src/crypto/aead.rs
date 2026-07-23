//! XChaCha20-Poly1305 — the only symmetric cipher on any plane (tech spec §2).
//!
//! The 24-byte nonce is large enough to draw randomly per message without
//! bookkeeping — which is exactly how [`random_nonce`] is meant to be used.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use super::{rng, CryptoError};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

/// A symmetric key. Not printable, zeroized on drop.
///
/// ```compile_fail
/// let k = rust_lib_hoppler::crypto::aead::AeadKey::generate();
/// println!("{:?}", k); // AeadKey must not implement Debug
/// ```
pub struct AeadKey(Zeroizing<[u8; KEY_LEN]>);

impl AeadKey {
    pub fn generate() -> Self {
        Self(Zeroizing::new(rng::random_array::<KEY_LEN>()))
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

/// A fresh random nonce. Never reuse a nonce with the same key.
pub fn random_nonce() -> [u8; NONCE_LEN] {
    rng::random_array::<NONCE_LEN>()
}

/// Encrypt-and-authenticate. Output is `ciphertext || tag` (`TAG_LEN` longer
/// than the plaintext). `aad` is authenticated but not encrypted.
///
/// The caller supplies `nonce` and MUST NOT reuse one with the same `key` —
/// keystream reuse is catastrophic for a stream-cipher AEAD. Use
/// [`random_nonce`] unless a protocol layer derives nonces deterministically
/// (e.g. a message-key ratchet).
pub fn seal(key: &AeadKey, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new((&*key.0).into());
    cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption is infallible for in-memory buffers")
}

/// Verify-and-decrypt. Fails on any mismatch of key, nonce, AAD, or a single
/// flipped ciphertext bit. The plaintext zeroizes on drop.
pub fn open(
    key: &AeadKey,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let cipher = XChaCha20Poly1305::new((&*key.0).into());
    cipher
        .decrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::AeadOpenFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // draft-irtf-cfrg-xchacha §A.3.1 test vector.
    #[test]
    fn xchacha_draft_vector() {
        let key = AeadKey::from_bytes(
            hex::decode("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
                .unwrap()
                .try_into()
                .unwrap(),
        );
        let nonce: [u8; NONCE_LEN] =
            hex::decode("404142434445464748494a4b4c4d4e4f5051525354555657")
                .unwrap()
                .try_into()
                .unwrap();
        let aad = hex::decode("50515253c0c1c2c3c4c5c6c7").unwrap();
        let plaintext = hex::decode(
            "4c616469657320616e642047656e746c656d656e206f662074686520636c617373206f66202739393a204966204920636f756c64206f6666657220796f75206f6e6c79206f6e652074697020666f7220746865206675747572652c2073756e73637265656e20776f756c642062652069742e"
        )
        .unwrap();
        let expected_ct_and_tag = hex::decode(concat!(
            "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb",
            "731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452",
            "2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9",
            "21f9664c97637da9768812f615c68b13b52e",
            "c0875924c1c7987947deafd8780acf49"
        ))
        .unwrap();

        let sealed = seal(&key, &nonce, &aad, &plaintext);
        assert_eq!(sealed, expected_ct_and_tag);

        let opened = open(&key, &nonce, &aad, &sealed).unwrap();
        assert_eq!(*opened, plaintext);
    }

    #[test]
    fn tamper_detection() {
        let key = AeadKey::generate();
        let nonce = random_nonce();
        let sealed = seal(&key, &nonce, b"aad", b"burrow contents");

        for i in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[i] ^= 0x01;
            assert_eq!(
                open(&key, &nonce, b"aad", &tampered).unwrap_err(),
                CryptoError::AeadOpenFailed,
                "flip at byte {i} must fail"
            );
        }
    }

    #[test]
    fn wrong_aad_rejected() {
        let key = AeadKey::generate();
        let nonce = random_nonce();
        let sealed = seal(&key, &nonce, b"aad", b"msg");
        assert!(open(&key, &nonce, b"other", &sealed).is_err());
    }

    #[test]
    fn wrong_nonce_rejected() {
        let key = AeadKey::generate();
        let sealed = seal(&key, &random_nonce(), b"", b"msg");
        assert!(open(&key, &random_nonce(), b"", &sealed).is_err());
    }
}
