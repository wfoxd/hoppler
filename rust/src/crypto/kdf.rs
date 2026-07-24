//! HKDF-SHA256 — all key derivation, including the per-counterparty
//! pseudonym derivation (tech spec §3).

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::CryptoError;

/// Expand `ikm` into `okm.len()` output bytes with HKDF-SHA256.
///
/// `info` is the domain-separation label — always use a versioned constant
/// like `b"hoppler/pseudonym/v1"`, never an empty string.
pub fn hkdf_sha256(
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    okm: &mut [u8],
) -> Result<(), CryptoError> {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, okm)
        .map_err(|_| CryptoError::KdfOutputTooLong)
}

/// Convenience: derive exactly 32 bytes (the common key-sized case).
pub fn derive_32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    hkdf_sha256(ikm, salt, info, out.as_mut()).expect("32 bytes is always a valid HKDF length");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 5869 A.1 (Test Case 1, SHA-256, L=42).
    #[test]
    fn rfc5869_test_case_1() {
        let ikm = [0x0bu8; 22];
        let salt = hex::decode("000102030405060708090a0b0c").unwrap();
        let info = hex::decode("f0f1f2f3f4f5f6f7f8f9").unwrap();

        let mut okm = [0u8; 42];
        hkdf_sha256(&ikm, &salt, &info, &mut okm).unwrap();

        assert_eq!(
            okm.to_vec(),
            hex::decode(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
            )
            .unwrap()
        );
    }

    #[test]
    fn label_separation() {
        let a = derive_32(b"ikm", b"salt", b"hoppler/test/v1");
        let b = derive_32(b"ikm", b"salt", b"hoppler/test/v2");
        assert_ne!(*a, *b);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            *derive_32(b"ikm", b"salt", b"hoppler/test/v1"),
            *derive_32(b"ikm", b"salt", b"hoppler/test/v1")
        );
    }

    #[test]
    fn over_long_output_rejected() {
        // HKDF-SHA256 max output is 255 * 32 = 8160 bytes.
        let mut okm = vec![0u8; 8161];
        assert_eq!(
            hkdf_sha256(b"ikm", b"salt", b"info", &mut okm),
            Err(CryptoError::KdfOutputTooLong)
        );
    }
}
