//! The fixed v0 crypto suite (tech spec §2, requirement R0-N5).
//!
//! Every primitive the core uses goes through these wrappers; no module
//! outside `crypto/` may import a primitive crate directly. That rule is
//! enforced by the `primitive_crates_are_confined` test below. There is
//! deliberately no algorithm agility: one suite, version-byte on the wire.

pub mod aead;
pub mod dh;
pub mod envelope;
pub mod hash;
pub mod kdf;
pub mod rng;
pub mod sign;

/// Errors across the suite. Deliberately coarse: callers get "it failed",
/// never why, so error surfaces can't become oracles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// A public key failed validation.
    InvalidPublicKey,
    /// Signature verification failed.
    SignatureInvalid,
    /// AEAD open failed (wrong key, nonce, AAD, or tampered ciphertext).
    AeadOpenFailed,
    /// Requested KDF output length exceeds the HKDF-SHA256 maximum.
    KdfOutputTooLong,
    /// Wire data was empty.
    EmptyWire,
    /// Leading wire version byte is not one we speak.
    UnsupportedWireVersion(u8),
    /// Version byte accepted but the protobuf body failed to parse.
    MalformedEnvelope,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::InvalidPublicKey => write!(f, "invalid public key"),
            CryptoError::SignatureInvalid => write!(f, "signature verification failed"),
            CryptoError::AeadOpenFailed => write!(f, "AEAD open failed"),
            CryptoError::KdfOutputTooLong => write!(f, "KDF output length too long"),
            CryptoError::EmptyWire => write!(f, "empty wire data"),
            CryptoError::UnsupportedWireVersion(v) => {
                write!(f, "unsupported wire version {v:#04x}")
            }
            CryptoError::MalformedEnvelope => write!(f, "malformed envelope"),
        }
    }
}

impl std::error::Error for CryptoError {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn visit_rs_files(dir: &Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                visit_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let contents = std::fs::read_to_string(&path).expect("readable source file");
                out.push((path, contents));
            }
        }
    }

    /// The G-3 / R0-N5 confinement rule: primitive crates are an
    /// implementation detail of `crypto/`. Everything else uses the wrappers.
    #[test]
    fn primitive_crates_are_confined_to_this_module() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let crypto_dir = src.join("crypto");
        let forbidden = [
            "ed25519_dalek",
            "x25519_dalek",
            "chacha20poly1305",
            "hkdf::",
            "sha2::",
            "blake3::",
            "getrandom::",
        ];

        let mut files = Vec::new();
        visit_rs_files(&src, &mut files);

        let mut offenders = Vec::new();
        for (path, contents) in files {
            if path.starts_with(&crypto_dir) {
                continue;
            }
            for name in forbidden {
                if contents.contains(name) {
                    offenders.push(format!("{} mentions {name}", path.display()));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "primitive crates imported outside src/crypto/ — use the crypto wrappers instead:\n{}",
            offenders.join("\n")
        );
    }
}
