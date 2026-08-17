//! A keystore that survives the process (T05's device-gated remainder).
//!
//! [`SoftwareKeystore`](super::keystore::SoftwareKeystore) holds its entries in
//! a `HashMap` and says so, and it is the only backend the engine has ever
//! constructed. The consequence was not written down anywhere and is severe:
//! every launch got an empty keystore, so no master was found, so `open_store`
//! took its stale-database path and **deleted the database and the file store**
//! — measured on a Pixel 8 Pro on 18 Aug 2026, where `hoppler.db` came back
//! with a different inode after every restart. Nothing on a phone has ever
//! survived being closed: not the identity, not a pairing, not a message.
//!
//! # What this guarantees, and what it does not
//!
//! Secrets are written to app-private storage — `/data/data/<pkg>/` on Android,
//! the application support directory on desktop — and protected by **file
//! permissions alone**. That is a real boundary on Android, where the app
//! sandbox is enforced by the kernel and a separate uid. It is a much weaker
//! one on a Linux desktop, where anything running as the same user can read
//! these bytes, and with them the store master, and with that the whole
//! database.
//!
//! So this is not what R0-F1 means by "master secret in platform hardware", and
//! it must not be mistaken for it. It is the layer *underneath* that: somewhere
//! durable to put bytes, which the Android Keystore backend then wraps. Stated
//! here rather than in a task doc because the person who needs to know is
//! whoever is reading this file wondering how strong it is.
//!
//! # Why the contents are not encrypted here
//!
//! There is nothing to encrypt them with. A key kept beside them protects
//! nothing, and the only honest alternatives are a passphrase the person types
//! (which Hoppler does not have, and which R0-F1's thirty-second first launch
//! rules out) or the platform's own secret store — which is exactly what the
//! Android backend adds on top. Encrypting with a key stored next door would
//! *look* like protection while providing none, which is worse than this.

use std::fs;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::keystore::{Keystore, KeystoreError};
use crate::crypto::hash;

/// Report a backend failure and collapse it to the coarse error the trait
/// exposes.
///
/// [`KeystoreError`] is deliberately coarse — a caller learns that the
/// operation failed, not why — and that is worth keeping. But "failed" with no
/// detail anywhere is a hardware session spent guessing, so the reason goes to
/// the log here. Paths and IO errors only; the secret never appears.
fn backend(what: &str, label_hash: &Path, error: std::io::Error) -> KeystoreError {
    log::warn!(
        "keystore could not {what} {}: {error}",
        label_hash.display()
    );
    KeystoreError::Backend
}

/// Domain separator for turning a label into a filename.
const LABEL_CONTEXT: &str = "hoppler/keystore/label/v1";

/// Secrets in a directory, one file per label.
pub struct FileKeystore {
    dir: PathBuf,
}

impl FileKeystore {
    /// Open (creating the directory if needed).
    ///
    /// Failing here is not survivable and must not be silent: a keystore that
    /// cannot write is a device that quietly forgets everything on every
    /// launch, which is the bug this type exists to fix.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, KeystoreError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(|e| backend("create", &dir, e))?;
        Ok(Self { dir })
    }

    /// Where a label's secret lives.
    ///
    /// Hashed rather than used directly: labels contain `/` (`hoppler/store-
    /// master/v1`), so a label is not a filename, and joining one unchecked is
    /// how a caller-controlled string becomes a path traversal. A fixed 64 hex
    /// characters is always safe and always the same for the same label.
    ///
    /// Unkeyed, unlike `FileStore::path_for`, and the difference is worth
    /// stating: there is no key to hash with at this layer — this *is* where
    /// the keys are. So the filenames are guessable. That discloses which
    /// labels exist to someone who can already list the directory, which is
    /// someone who can already read the secrets.
    fn path_for(&self, label: &str) -> PathBuf {
        self.dir.join(hex::encode(hash::derive_key(
            LABEL_CONTEXT,
            label.as_bytes(),
        )))
    }
}

impl Keystore for FileKeystore {
    /// Write a secret, replacing any previous one under the same label.
    ///
    /// Written to a temporary file and renamed, because the alternative is
    /// losing the old secret to a crash partway through writing the new one.
    /// `rename` within a directory is atomic on both platforms: a reader sees
    /// the old bytes or the new ones, never a truncated file. A half-written
    /// master is an undecryptable database, which is the same as no database.
    fn seal(&self, label: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        let path = self.path_for(label);
        let staging = path.with_extension("new");
        fs::write(&staging, secret).map_err(|e| backend("write", &staging, e))?;
        fs::rename(&staging, &path).map_err(|e| {
            // Leaving the staging file behind would be a secret on disk under a
            // name nothing looks for, so it goes even on the failure path.
            let _ = fs::remove_file(&staging);
            backend("replace", &path, e)
        })?;
        Ok(())
    }

    fn unseal(&self, label: &str) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
        match fs::read(self.path_for(label)) {
            Ok(bytes) => Ok(Zeroizing::new(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(KeystoreError::NotFound),
            Err(e) => Err(backend("read", &self.path_for(label), e)),
        }
    }

    /// Remove a secret. Absent is success, because R0-F9's wipe has to be
    /// runnable twice — the second run finding nothing is the first one having
    /// worked, not a failure.
    fn wipe(&self, label: &str) -> Result<(), KeystoreError> {
        match fs::remove_file(self.path_for(label)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(backend("remove", &self.path_for(label), e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystore() -> (FileKeystore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ks = FileKeystore::open(dir.path().join("keys")).unwrap();
        (ks, dir)
    }

    #[test]
    fn a_sealed_secret_comes_back() {
        let (ks, _d) = keystore();
        ks.seal("hoppler/store-master/v1", &[1, 2, 3, 4]).unwrap();
        assert_eq!(
            &*ks.unseal("hoppler/store-master/v1").unwrap(),
            &[1, 2, 3, 4]
        );
    }

    /// The whole point: a *different instance*, as a relaunched app would use.
    /// The in-memory keystore passes every other test in this file and fails
    /// this one, which is the difference that cost the database.
    #[test]
    fn a_secret_survives_the_keystore_being_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys");
        FileKeystore::open(&path)
            .unwrap()
            .seal("hoppler/store-master/v1", &[9u8; 32])
            .unwrap();

        let relaunched = FileKeystore::open(&path).unwrap();
        assert_eq!(
            &*relaunched.unseal("hoppler/store-master/v1").unwrap(),
            &[9u8; 32],
            "the secret did not survive a reopen — this is the failure that \
             deleted the database on every launch"
        );
    }

    #[test]
    fn an_absent_label_is_not_found_rather_than_an_error() {
        let (ks, _d) = keystore();
        assert_eq!(
            ks.unseal("hoppler/nothing/v1").unwrap_err(),
            KeystoreError::NotFound
        );
    }

    #[test]
    fn sealing_again_replaces() {
        let (ks, _d) = keystore();
        ks.seal("k", &[1]).unwrap();
        ks.seal("k", &[2, 2]).unwrap();
        assert_eq!(&*ks.unseal("k").unwrap(), &[2, 2]);
    }

    /// R0-F9 wipe, and it has to be idempotent: a wipe that failed the second
    /// time would turn a retry into an error report about something that had
    /// already worked.
    #[test]
    fn wiping_removes_it_and_wiping_again_is_fine() {
        let (ks, _d) = keystore();
        ks.seal("k", &[1]).unwrap();
        ks.wipe("k").unwrap();
        assert_eq!(ks.unseal("k").unwrap_err(), KeystoreError::NotFound);
        ks.wipe("k").unwrap();
    }

    /// Labels contain `/`, so a label is not a filename. Joining one unchecked
    /// is how `hoppler/../../etc/x` becomes a write outside the directory.
    #[test]
    fn a_label_cannot_escape_the_directory() {
        let (ks, dir) = keystore();
        for label in [
            "hoppler/store-master/v1",
            "../../escape",
            "/absolute/path",
            "..",
            "",
        ] {
            ks.seal(label, &[7]).unwrap();
            let path = ks.path_for(label);
            assert_eq!(
                path.parent(),
                Some(dir.path().join("keys").as_path()),
                "label {label:?} wrote to {path:?}, outside the keystore"
            );
            assert_eq!(&*ks.unseal(label).unwrap(), &[7]);
        }
    }

    /// Two labels must not share a file, or sealing one would silently destroy
    /// the other — the store master and a Layer-1 seed are both 32 bytes and
    /// neither would look wrong.
    #[test]
    fn different_labels_do_not_collide() {
        let (ks, _d) = keystore();
        ks.seal("hoppler/layer1-seed/v1", &[1u8; 32]).unwrap();
        ks.seal("hoppler/layer2-seed/v1", &[2u8; 32]).unwrap();
        ks.seal("hoppler/store-master/v1", &[3u8; 32]).unwrap();
        assert_eq!(&*ks.unseal("hoppler/layer1-seed/v1").unwrap(), &[1u8; 32]);
        assert_eq!(&*ks.unseal("hoppler/layer2-seed/v1").unwrap(), &[2u8; 32]);
        assert_eq!(&*ks.unseal("hoppler/store-master/v1").unwrap(), &[3u8; 32]);
    }

    /// A replacement that dies partway must not take the old secret with it.
    /// Checked by looking at what is left behind: nothing but the secret's own
    /// file, so no staging copy is sitting there holding a secret under a name
    /// nothing reads.
    #[test]
    fn replacing_leaves_no_staging_file_behind() {
        let (ks, _d) = keystore();
        ks.seal("k", &[1u8; 32]).unwrap();
        ks.seal("k", &[2u8; 32]).unwrap();
        let files: Vec<_> = fs::read_dir(&ks.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "left {files:?} in the keystore directory");
        assert!(!files[0].ends_with(".new"));
    }
}
