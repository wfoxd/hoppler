//! BLAKE3 — hashing, keyed hashing, and context key derivation.
//! (Chunk-tree transfer hashing arrives with T16 and will build on this.)

use zeroize::Zeroizing;

pub const HASH_LEN: usize = 32;

/// Plain hash. Output is a public digest.
pub fn hash(data: &[u8]) -> [u8; HASH_LEN] {
    *blake3::hash(data).as_bytes()
}

/// Keyed hash (MAC-like). Output is a tag, not key material.
pub fn keyed_hash(key: &[u8; HASH_LEN], data: &[u8]) -> [u8; HASH_LEN] {
    *blake3::keyed_hash(key, data).as_bytes()
}

/// Derive a key from `key_material` in a named context.
/// `context` must be a hardcoded, versioned constant (BLAKE3 rules).
///
/// Returns key material, so the output zeroizes on drop (matching
/// [`super::kdf::derive_32`]).
pub fn derive_key(context: &str, key_material: &[u8]) -> Zeroizing<[u8; HASH_LEN]> {
    Zeroizing::new(blake3::derive_key(context, key_material))
}

#[cfg(test)]
mod tests {
    use super::*;

    // From the official BLAKE3 test vectors: hash of the empty input.
    #[test]
    fn empty_input_vector() {
        assert_eq!(
            hash(b"").to_vec(),
            hex::decode("af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262")
                .unwrap()
        );
    }

    #[test]
    fn keyed_differs_from_unkeyed() {
        let key = [7u8; HASH_LEN];
        assert_ne!(hash(b"data"), keyed_hash(&key, b"data"));
    }

    #[test]
    fn keyed_differs_by_key() {
        assert_ne!(
            keyed_hash(&[1u8; 32], b"data"),
            keyed_hash(&[2u8; 32], b"data")
        );
    }

    #[test]
    fn derive_key_context_separation() {
        assert_ne!(
            *derive_key("hoppler/test/v1", b"material"),
            *derive_key("hoppler/test/v2", b"material")
        );
    }
}
