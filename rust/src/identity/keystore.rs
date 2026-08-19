//! Keystore boundary for Layer-1 / Layer-2 secret seeds (tech spec §3).
//!
//! The production backend on Android is
//! [`WrappedKeystore`](super::wrapped::WrappedKeystore) over
//! [`FileKeystore`](super::filekeystore::FileKeystore): the seeds live in
//! app-private storage, encrypted under a key generated in the Android Keystore
//! that cannot be exported. Its device-only "attempted extraction" acceptance
//! test is the remaining device-gated piece of T05.
//!
//! This module defines the trait every backend implements, plus an in-memory
//! [`SoftwareKeystore`] used by the core's tests and by desktop dev builds.
//! On mobile the seeds are sealed by hardware; "hardware-held" there means the
//! enclave guards a master key that wraps these seeds (the enclave's own curve
//! is not Ed25519, so it cannot hold the signing key directly).
//!
//! Trait-shape limit, now settled rather than open: [`Keystore::unseal`]
//! returns the raw seed, so the hardware backend wraps and unwraps it and the
//! seed lands in process RAM at unseal time. Keeping it sealed throughout would
//! need operation methods (sign-in-enclave, derive-in-enclave) instead of
//! export — and the enclave's own curve is not Ed25519, so it could not hold
//! the signing key even then. What wrapping buys is stated exactly in
//! [`wrapped`](super::wrapped): storage read anywhere but the running app is
//! useless.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroizing;

/// Errors from a keystore backend. Coarse by design — a caller learns that an
/// operation failed, not the backend's internal reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeystoreError {
    /// No secret is sealed under that label.
    NotFound,
    /// The backend rejected or failed the operation.
    Backend,
}

impl std::fmt::Display for KeystoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeystoreError::NotFound => write!(f, "no secret under that label"),
            KeystoreError::Backend => write!(f, "keystore backend error"),
        }
    }
}

impl std::error::Error for KeystoreError {}

/// Seals and unseals secret material by label.
///
/// Implementations: the native hardware keystore (production on mobile) or
/// [`SoftwareKeystore`] (tests and desktop). `Send + Sync` so the core can hold
/// one behind a shared reference across its worker threads.
pub trait Keystore: Send + Sync {
    /// Seal `secret` under `label`, replacing any existing entry.
    fn seal(&self, label: &str, secret: &[u8]) -> Result<(), KeystoreError>;
    /// Unseal the secret sealed under `label`.
    fn unseal(&self, label: &str) -> Result<Zeroizing<Vec<u8>>, KeystoreError>;
    /// Destroy the secret under `label`. Idempotent — wiping an absent label is Ok.
    fn wipe(&self, label: &str) -> Result<(), KeystoreError>;
}

/// In-memory software fallback.
///
/// NOT hardware-backed: secrets live in process memory (zeroized on wipe and
/// drop) with no OS non-exportability guarantee. For tests and desktop dev
/// only — never the production path on a mobile device.
#[derive(Default)]
pub struct SoftwareKeystore {
    entries: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
}

impl SoftwareKeystore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Keystore for SoftwareKeystore {
    fn seal(&self, label: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        // Recover from a poisoned lock: these critical sections only touch the
        // map, so its invariants can't be broken by a panic elsewhere, and
        // recovering keeps wipe/unseal available after such a panic.
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(label.to_owned(), Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn unseal(&self, label: &str) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.get(label).cloned().ok_or(KeystoreError::NotFound)
    }

    fn wipe(&self, label: &str) -> Result<(), KeystoreError> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.remove(label);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_unseal_round_trips() {
        let ks = SoftwareKeystore::new();
        ks.seal("layer1", &[1, 2, 3, 4]).unwrap();
        assert_eq!(&*ks.unseal("layer1").unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn unseal_missing_is_not_found() {
        let ks = SoftwareKeystore::new();
        assert_eq!(ks.unseal("absent").unwrap_err(), KeystoreError::NotFound);
    }

    #[test]
    fn seal_replaces_existing() {
        let ks = SoftwareKeystore::new();
        ks.seal("k", &[1]).unwrap();
        ks.seal("k", &[9, 9]).unwrap();
        assert_eq!(&*ks.unseal("k").unwrap(), &[9, 9]);
    }

    #[test]
    fn wipe_removes_and_is_idempotent() {
        let ks = SoftwareKeystore::new();
        ks.seal("k", &[1]).unwrap();
        ks.wipe("k").unwrap();
        assert_eq!(ks.unseal("k").unwrap_err(), KeystoreError::NotFound);
        ks.wipe("k").unwrap(); // absent label is fine
    }

    #[test]
    fn concurrent_access_is_safe() {
        use std::sync::Arc;
        use std::thread;

        let ks = Arc::new(SoftwareKeystore::new());
        let handles: Vec<_> = (0..8u8)
            .map(|i| {
                let ks = Arc::clone(&ks);
                thread::spawn(move || {
                    let label = format!("k{i}");
                    for _ in 0..200 {
                        ks.seal(&label, &[i; 32]).unwrap();
                        let _ = ks.unseal(&label);
                        ks.wipe(&label).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn identity_seeds_round_trip_through_keystore() {
        use crate::identity::Identity;

        let ks = SoftwareKeystore::new();
        let id = Identity::generate("Alice", 7);
        ks.seal("l1", &*id.layer1_seed()).unwrap();
        ks.seal("l2", &*id.layer2_seed()).unwrap();

        let l1: [u8; 32] = (*ks.unseal("l1").unwrap()).as_slice().try_into().unwrap();
        let l2: [u8; 32] = (*ks.unseal("l2").unwrap()).as_slice().try_into().unwrap();
        let restored = Identity::from_parts(&l1, &l2, id.persona().clone());

        assert_eq!(restored.layer1_public().0, id.layer1_public().0);
        assert_eq!(restored.layer2_public().0, id.layer2_public().0);
    }
}
