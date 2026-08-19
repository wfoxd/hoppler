//! The seeds, wrapped by a key the platform holds and will not hand over.
//!
//! [`FileKeystore`](super::filekeystore) puts secrets somewhere durable and
//! protects them with file permissions, and its own module doc is careful to
//! say that this is *not* what R0-F1 means by "master secret in platform
//! hardware". This is the layer that closes that gap: the bytes on disk become
//! ciphertext under a key that lives in the Android Keystore — generated there,
//! never exported, unusable off the device that made it.
//!
//! # What this defends against, and what it does not
//!
//! It defends against **the storage being read somewhere other than the running
//! app**: a flashed data partition, a backup, a device handed to someone who
//! can pull `/data` but not execute as Hoppler's uid. Those bytes are now
//! useless without the hardware key, which cannot leave the device.
//!
//! It does **not** defend against code running as Hoppler's own uid. Anything
//! with that can ask the same hardware key to unwrap the same blobs, exactly as
//! the app does. Wrapping does not change what a rooted device can do, and
//! nothing in this module should be read as claiming otherwise.
//!
//! That boundary decides a design question below — see [`WrappedKeystore`] on
//! why an unwrapped secret is still accepted.
//!
//! # Where the crypto happens
//!
//! Not here. [`Hardware`] is one call out to the platform, which does the
//! encryption itself with a key the process never sees; a Rust implementation
//! would have to hold the key in memory to use it, which is the thing being
//! avoided. So this module has no cipher in it, and the format of a wrapped
//! blob is the platform's business — everything past the prefix is opaque.

use zeroize::Zeroizing;

use super::keystore::{Keystore, KeystoreError};

/// Marks a stored secret as wrapped: `HPKW`, then a format version.
///
/// Its job is telling a wrapped secret from one written before wrapping
/// existed, which is what makes the upgrade in [`WrappedKeystore::unseal`]
/// possible. Five bytes rather than one because a bare seed is arbitrary bytes
/// and could begin with anything; four fixed bytes make a collision something
/// that has to be arranged rather than something that can happen.
const WRAPPED_PREFIX: &[u8] = b"HPKW\x01";

/// A wrapping key held by the platform.
///
/// One key serves every label, so the label is passed on each call and is
/// expected to be bound in as associated data. Without that, a blob is
/// interchangeable with any other: whoever could move the Layer-2 seed's file
/// over the store master's would get a clean unwrap and a store keyed by
/// something that is not its master. The label costs nothing to include and
/// removes the question.
///
/// `Send + Sync` to match [`Keystore`], which the core holds across threads.
pub trait Hardware: Send + Sync {
    /// Wrap `secret` for `label`. The returned bytes are opaque — whatever the
    /// platform needs to reverse it later, including any nonce it chose.
    fn wrap(&self, label: &str, secret: &[u8]) -> Result<Vec<u8>, KeystoreError>;

    /// Reverse [`wrap`](Hardware::wrap) for the same label.
    ///
    /// Every failure should be [`KeystoreError::Backend`], including one caused
    /// by the key being gone — see [`WrappedKeystore::unseal`] for why that
    /// matters more than it looks. `unseal` does not rely on this: it maps
    /// whatever comes back, because a promise in a trait doc is not a thing the
    /// caller can check and the cost of an implementor forgetting is a lost
    /// identity.
    fn unwrap(&self, label: &str, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeystoreError>;
}

/// A keystore that wraps everything it stores.
///
/// Composed rather than folded into [`FileKeystore`](super::filekeystore)
/// because the two answer different questions — where the bytes live, and what
/// makes them unreadable elsewhere — and only one of them has a platform
/// answer. Linux desktop has no equivalent of the Android Keystore, so there it
/// stays the file keystore alone and the module doc there is the honest
/// description.
pub struct WrappedKeystore<K, H> {
    inner: K,
    hardware: H,
}

impl<K: Keystore, H: Hardware> WrappedKeystore<K, H> {
    /// Put `hardware` in front of `inner`.
    pub fn new(inner: K, hardware: H) -> Self {
        Self { inner, hardware }
    }
}

impl<K: Keystore, H: Hardware> Keystore for WrappedKeystore<K, H> {
    fn seal(&self, label: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        let mut blob = Vec::with_capacity(WRAPPED_PREFIX.len() + secret.len() + 32);
        blob.extend_from_slice(WRAPPED_PREFIX);
        blob.extend_from_slice(&self.hardware.wrap(label, secret)?);
        self.inner.seal(label, &blob)
    }

    /// Unseal, unwrapping if it was wrapped.
    ///
    /// # A failed unwrap is never `NotFound`
    ///
    /// The distinction is the whole safety of this type. `NotFound` means "this
    /// device has no identity", and the engine answers it by generating one —
    /// so reporting a *broken* secret as missing would seal a brand new
    /// identity over the top of one that was merely unreadable, and R0-F1's
    /// "this device stays the same person" would be false in the one case where
    /// it is most visible. `Identity::unsealed` had precisely this bug with the
    /// arms the other way round. Every unwrap failure is therefore
    /// [`KeystoreError::Backend`], which stops the launch instead.
    ///
    /// Failures here are real and not hypothetical: Android invalidates keys
    /// when the lock screen credential is removed, and app data restored onto a
    /// different device arrives with blobs no local key can open.
    ///
    /// # An unwrapped secret is accepted, and upgraded
    ///
    /// Devices already running Hoppler have bare seeds written by the file
    /// keystore before this type existed. Refusing them would be a build that
    /// forgets who the device is on the launch after the update — the exact
    /// failure this whole line of work exists to fix. So a secret with no
    /// prefix is returned as it is, and re-sealed wrapped on the way past.
    ///
    /// It does mean somebody who can write into app-private storage could put a
    /// bare secret there and have it accepted. Per the module doc, that
    /// attacker is executing as Hoppler's uid, and can therefore ask the
    /// hardware key to wrap one just as convincingly — the passthrough gives
    /// them nothing they did not already have. It is the attacker holding the
    /// storage *without* the device that wrapping shuts out, and they cannot
    /// write anything anywhere.
    fn unseal(&self, label: &str) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
        let stored = self.inner.unseal(label)?;
        let Some(wrapped) = stored.strip_prefix(WRAPPED_PREFIX) else {
            log::info!("wrapping a secret written before the keystore had hardware");
            // Best effort: the read succeeded, and failing it because the
            // upgrade could not be written would turn a device that works into
            // one that will not launch. It is retried on the next unseal.
            if let Err(e) = self.seal(label, &stored) {
                log::warn!("could not wrap it on the way past ({e}); will retry");
            }
            return Ok(stored);
        };
        // Mapped rather than propagated. The trait doc asks for `Backend`, but
        // an implementation is free to get that wrong, and one that answered
        // `NotFound` for a key it could not use would walk straight into the
        // failure this method is written to prevent — silently, and only on the
        // devices whose keys had actually gone. Costing a line here to make it
        // unreachable is better than a rule an implementor has to remember.
        self.hardware
            .unwrap(label, wrapped)
            .map_err(|_| KeystoreError::Backend)
    }

    /// Remove the secret.
    ///
    /// The wrapping key is left alone, because one key serves every label and
    /// destroying it here would take the other secrets with it. R0-F9's wipe
    /// does not need it destroyed either: a wrapped secret whose ciphertext is
    /// gone is gone, whatever key could have opened it.
    fn wipe(&self, label: &str) -> Result<(), KeystoreError> {
        self.inner.wipe(label)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::filekeystore::FileKeystore;
    use super::super::keystore::SoftwareKeystore;
    use super::*;

    /// Stands in for the platform: a reversible transform that is not the
    /// identity, binds the label, and can be told to fail.
    ///
    /// Deliberately not a cipher. What these tests check is the composition —
    /// which bytes reach the inner keystore, which errors come back out — and
    /// a real AEAD here would test the AEAD crate instead. The real one is
    /// Android's, and is checked on a device.
    #[derive(Default)]
    struct FakeHardware {
        broken: Mutex<bool>,
    }

    impl FakeHardware {
        fn break_it(&self) {
            *self.broken.lock().unwrap() = true;
        }
    }

    impl Hardware for FakeHardware {
        fn wrap(&self, label: &str, secret: &[u8]) -> Result<Vec<u8>, KeystoreError> {
            if *self.broken.lock().unwrap() {
                return Err(KeystoreError::Backend);
            }
            let mut out = format!("{}:", label.len()).into_bytes();
            out.extend_from_slice(label.as_bytes());
            out.extend(secret.iter().map(|b| !b));
            Ok(out)
        }

        fn unwrap(&self, label: &str, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
            if *self.broken.lock().unwrap() {
                return Err(KeystoreError::Backend);
            }
            let expected = {
                let mut e = format!("{}:", label.len()).into_bytes();
                e.extend_from_slice(label.as_bytes());
                e
            };
            let body = wrapped
                .strip_prefix(expected.as_slice())
                .ok_or(KeystoreError::Backend)?;
            Ok(Zeroizing::new(body.iter().map(|b| !b).collect()))
        }
    }

    fn keystore() -> WrappedKeystore<SoftwareKeystore, FakeHardware> {
        WrappedKeystore::new(SoftwareKeystore::new(), FakeHardware::default())
    }

    #[test]
    fn a_wrapped_secret_comes_back() {
        let ks = keystore();
        ks.seal("hoppler/store-master/v1", &[7u8; 32]).unwrap();
        assert_eq!(&*ks.unseal("hoppler/store-master/v1").unwrap(), &[7u8; 32]);
    }

    /// The point of the type. Checked against the *file* keystore because what
    /// matters is the bytes that land on disk, which is what an attacker
    /// holding the storage gets.
    #[test]
    fn the_secret_is_not_what_reaches_the_storage() {
        let dir = tempfile::tempdir().unwrap();
        let secret = [7u8; 32];
        let ks = WrappedKeystore::new(
            FileKeystore::open(dir.path().join("keys")).unwrap(),
            FakeHardware::default(),
        );
        ks.seal("hoppler/store-master/v1", &secret).unwrap();

        let on_disk = std::fs::read_dir(dir.path().join("keys"))
            .unwrap()
            .map(|e| std::fs::read(e.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(on_disk.len(), 1);
        assert!(
            !on_disk[0].windows(secret.len()).any(|w| w == secret),
            "the seed is sitting in the file in the clear"
        );
    }

    /// A blob moved from one label to another must not open. Otherwise the
    /// Layer-2 seed could be made to serve as the store master by renaming a
    /// file, and nothing downstream would notice.
    #[test]
    fn a_blob_does_not_open_under_a_different_label() {
        let inner = SoftwareKeystore::new();
        let ks = WrappedKeystore::new(inner, FakeHardware::default());
        ks.seal("hoppler/layer2-seed/v1", &[2u8; 32]).unwrap();
        let stolen = ks.inner.unseal("hoppler/layer2-seed/v1").unwrap();
        ks.inner.seal("hoppler/store-master/v1", &stolen).unwrap();

        assert_eq!(
            ks.unseal("hoppler/store-master/v1").unwrap_err(),
            KeystoreError::Backend
        );
    }

    /// The one that keeps R0-F1 true. A hardware key that has gone — Android
    /// drops them when the lock screen credential is removed — must not read as
    /// "no identity here", because the engine answers that by making a new one
    /// and sealing it over the top.
    #[test]
    fn a_broken_hardware_key_is_a_backend_error_not_a_missing_secret() {
        let ks = keystore();
        ks.seal("hoppler/layer1-seed/v1", &[1u8; 32]).unwrap();
        ks.hardware.break_it();

        assert_eq!(
            ks.unseal("hoppler/layer1-seed/v1").unwrap_err(),
            KeystoreError::Backend,
            "an unreadable identity reported as an absent one is a device that \
             becomes a different person after a lock screen change"
        );
    }

    /// The same property, but against the hardware layer itself getting it
    /// wrong. `Hardware`'s doc asks for `Backend`; an implementation that
    /// answered `NotFound` for a key it could not use would hand the engine
    /// "no identity here" for an identity that is right there, and this type
    /// must not let a doc comment be the only thing standing in the way.
    #[test]
    fn a_hardware_that_answers_not_found_still_does_not_lose_the_identity() {
        struct Confused;
        impl Hardware for Confused {
            fn wrap(&self, _: &str, secret: &[u8]) -> Result<Vec<u8>, KeystoreError> {
                Ok(secret.to_vec())
            }
            fn unwrap(&self, _: &str, _: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
                Err(KeystoreError::NotFound)
            }
        }

        let ks = WrappedKeystore::new(SoftwareKeystore::new(), Confused);
        ks.seal("hoppler/layer1-seed/v1", &[1u8; 32]).unwrap();
        assert_eq!(
            ks.unseal("hoppler/layer1-seed/v1").unwrap_err(),
            KeystoreError::Backend,
            "a hardware backend's mistake became the engine's instruction to \
             generate a new identity"
        );
    }

    /// Nothing sealed is still nothing sealed — the wrapper must not turn a
    /// genuine first launch into an error, or no device could ever start.
    #[test]
    fn an_absent_label_is_still_not_found() {
        let ks = keystore();
        assert_eq!(
            ks.unseal("hoppler/nothing/v1").unwrap_err(),
            KeystoreError::NotFound
        );
    }

    /// Every device already running Hoppler has bare seeds. Refusing them would
    /// be an update that forgets who the device is.
    #[test]
    fn a_secret_from_before_wrapping_is_still_readable() {
        let inner = SoftwareKeystore::new();
        inner.seal("hoppler/layer1-seed/v1", &[3u8; 32]).unwrap();
        let ks = WrappedKeystore::new(inner, FakeHardware::default());

        assert_eq!(&*ks.unseal("hoppler/layer1-seed/v1").unwrap(), &[3u8; 32]);
    }

    /// And it is only readable-as-bare once: the upgrade has to actually write,
    /// or every launch would keep an unwrapped seed on disk forever and the
    /// type would be decorative.
    #[test]
    fn reading_an_old_secret_wraps_it() {
        let inner = SoftwareKeystore::new();
        inner.seal("hoppler/layer1-seed/v1", &[3u8; 32]).unwrap();
        let ks = WrappedKeystore::new(inner, FakeHardware::default());
        ks.unseal("hoppler/layer1-seed/v1").unwrap();

        let stored = ks.inner.unseal("hoppler/layer1-seed/v1").unwrap();
        assert!(
            stored.starts_with(WRAPPED_PREFIX),
            "the seed is still unwrapped after being read"
        );
        assert_eq!(&*ks.unseal("hoppler/layer1-seed/v1").unwrap(), &[3u8; 32]);
    }

    /// The upgrade is a bonus, not a precondition. A device that cannot write
    /// it must still launch — losing the identity to a failed *optimisation*
    /// would be worse than the unwrapped seed it was trying to fix.
    #[test]
    fn an_old_secret_still_reads_when_it_cannot_be_wrapped() {
        let inner = SoftwareKeystore::new();
        inner.seal("hoppler/layer1-seed/v1", &[3u8; 32]).unwrap();
        let ks = WrappedKeystore::new(inner, FakeHardware::default());
        ks.hardware.break_it();

        assert_eq!(&*ks.unseal("hoppler/layer1-seed/v1").unwrap(), &[3u8; 32]);
    }

    #[test]
    fn sealing_again_replaces() {
        let ks = keystore();
        ks.seal("k", &[1u8; 32]).unwrap();
        ks.seal("k", &[2u8; 32]).unwrap();
        assert_eq!(&*ks.unseal("k").unwrap(), &[2u8; 32]);
    }

    /// R0-F9, and idempotent for the same reason as everywhere else: a wipe has
    /// to be runnable twice.
    #[test]
    fn wiping_removes_it_and_wiping_again_is_fine() {
        let ks = keystore();
        ks.seal("k", &[1u8; 32]).unwrap();
        ks.wipe("k").unwrap();
        assert_eq!(ks.unseal("k").unwrap_err(), KeystoreError::NotFound);
        ks.wipe("k").unwrap();
    }

    /// A hardware refusal on the way in must fail the seal rather than fall
    /// back to storing the secret in the clear — a keystore that silently stops
    /// wrapping is the worst of both.
    #[test]
    fn a_hardware_refusal_fails_the_seal_rather_than_storing_it_bare() {
        let ks = keystore();
        ks.hardware.break_it();
        assert_eq!(
            ks.seal("k", &[1u8; 32]).unwrap_err(),
            KeystoreError::Backend
        );
        assert_eq!(ks.unseal("k").unwrap_err(), KeystoreError::NotFound);
    }
}
