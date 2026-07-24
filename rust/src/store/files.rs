//! Encrypted file store.
//!
//! Each file is sealed with a key derived from the master and the file id, so
//! no per-file key is ever stored and crypto-erasing the master makes every
//! file undecryptable. The on-disk name is a *keyed* hash of the id (keyed by a
//! master-derived key): unguessable and unlinkable without the master, and
//! always a traversal-safe fixed-length filename. The id is also the AEAD
//! associated data, binding each ciphertext to its own id.

use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::StoreError;
use crate::crypto::{aead, hash, kdf};

const FILE_KEY_LABEL: &[u8] = b"hoppler/filekey/v1";
const FILE_NAME_CONTEXT: &str = "hoppler/filename/v1";

/// A handle to the encrypted file store, borrowing the store's master key.
pub struct FileStore<'a> {
    dir: &'a Path,
    master: &'a [u8; 32],
    /// Keyed-hash key for on-disk filenames, derived from the master (zeroized
    /// on drop).
    name_key: Zeroizing<[u8; 32]>,
}

impl<'a> FileStore<'a> {
    pub(super) fn new(dir: &'a Path, master: &'a [u8; 32]) -> Self {
        Self {
            dir,
            master,
            name_key: hash::derive_key(FILE_NAME_CONTEXT, master),
        }
    }

    fn key_for(&self, id: &str) -> aead::AeadKey {
        let derived = kdf::derive_32(self.master, id.as_bytes(), FILE_KEY_LABEL);
        aead::AeadKey::from_bytes(*derived)
    }

    fn path_for(&self, id: &str) -> PathBuf {
        // Keyed hash: the on-disk name is unguessable and unlinkable without the
        // master, so a directory-reader can't confirm a guessed id's presence.
        // Fixed 64-hex output is also always a traversal-safe filename.
        self.dir
            .join(hex::encode(hash::keyed_hash(&self.name_key, id.as_bytes())))
    }

    /// Seal `plaintext` under `id`. On-disk layout is `nonce || ciphertext`.
    /// Written atomically (temp file + fsync + rename) so a crash mid-write
    /// cannot truncate or clobber an existing file.
    pub fn put(&self, id: &str, plaintext: &[u8]) -> Result<(), StoreError> {
        let key = self.key_for(id);
        let nonce = aead::random_nonce();
        let ciphertext = aead::seal(&key, &nonce, id.as_bytes(), plaintext);

        let mut out = Vec::with_capacity(nonce.len() + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);

        let path = self.path_for(id);
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp).map_err(|_| StoreError::FileIo)?;
            f.write_all(&out).map_err(|_| StoreError::FileIo)?;
            f.sync_all().map_err(|_| StoreError::FileIo)?;
        }
        std::fs::rename(&tmp, &path).map_err(|_| StoreError::FileIo)?;
        // fsync the directory so the rename itself is durable across power loss.
        // Best-effort: opening a directory as a file is Unix-only (all Ring 0
        // targets), and a failure here doesn't corrupt the just-written file.
        if let Ok(dir) = std::fs::File::open(self.dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Read and open the file under `id`. The returned plaintext zeroizes on
    /// drop and is not copied out of its `Zeroizing` buffer.
    pub fn get(&self, id: &str) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        let raw = std::fs::read(self.path_for(id)).map_err(|_| StoreError::FileIo)?;
        if raw.len() < aead::NONCE_LEN {
            return Err(StoreError::Decrypt);
        }
        let (nonce, ciphertext) = raw.split_at(aead::NONCE_LEN);
        let nonce: [u8; aead::NONCE_LEN] = nonce.try_into().expect("checked length above");
        let key = self.key_for(id);
        aead::open(&key, &nonce, id.as_bytes(), ciphertext).map_err(|_| StoreError::Decrypt)
    }

    /// Delete the file under `id`. Absent files are not an error.
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path_for(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreError::FileIo),
        }
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path_for(id).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; 32];
        let fs = FileStore::new(dir.path(), &master);
        fs.put("photo-1", b"full-resolution bytes").unwrap();
        assert_eq!(
            fs.get("photo-1").unwrap().as_slice(),
            b"full-resolution bytes"
        );
    }

    #[test]
    fn file_bytes_are_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; 32];
        let fs = FileStore::new(dir.path(), &master);
        fs.put("f", b"PLAINTEXT-MARKER-XYZ").unwrap();
        let raw = std::fs::read(fs.path_for("f")).unwrap();
        assert!(!raw.windows(20).any(|w| w == b"PLAINTEXT-MARKER-XYZ"));
    }

    #[test]
    fn tampered_file_fails_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; 32];
        let fs = FileStore::new(dir.path(), &master);
        fs.put("f", b"data").unwrap();

        let path = fs.path_for("f");
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(&path, bytes).unwrap();

        assert!(matches!(fs.get("f"), Err(StoreError::Decrypt)));
    }

    #[test]
    fn wrong_master_cannot_open() {
        let dir = tempfile::tempdir().unwrap();
        FileStore::new(dir.path(), &[1u8; 32])
            .put("f", b"data")
            .unwrap();
        // The filename is keyed by the master, so a wrong master can't even
        // locate the file (a stronger property than a decrypt failure).
        assert!(FileStore::new(dir.path(), &[2u8; 32]).get("f").is_err());
    }

    #[test]
    fn delete_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; 32];
        let fs = FileStore::new(dir.path(), &master);
        fs.put("f", b"x").unwrap();
        assert!(fs.exists("f"));
        fs.delete("f").unwrap();
        assert!(!fs.exists("f"));
        fs.delete("f").unwrap(); // absent is not an error
    }

    #[test]
    fn arbitrary_id_is_a_safe_filename() {
        let dir = tempfile::tempdir().unwrap();
        let fs = FileStore::new(dir.path(), &[7u8; 32]);
        // A traversal-looking id must not escape the directory.
        fs.put("../../etc/passwd", b"x").unwrap();
        assert_eq!(fs.get("../../etc/passwd").unwrap().as_slice(), b"x");
        assert!(!dir.path().parent().unwrap().join("etc").exists());
    }
}
