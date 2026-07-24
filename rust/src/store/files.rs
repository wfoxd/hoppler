//! Encrypted file store.
//!
//! Each file is sealed with a key derived from the master and the file id, so
//! no per-file key is ever stored and crypto-erasing the master makes every
//! file undecryptable. The on-disk name is a hash of the id, so any id string
//! is a safe filename (no path traversal). The id is also the AEAD associated
//! data, binding each ciphertext to its own id.

use std::path::{Path, PathBuf};

use super::StoreError;
use crate::crypto::{aead, hash, kdf};

const FILE_KEY_LABEL: &[u8] = b"hoppler/filekey/v1";

/// A handle to the encrypted file store, borrowing the store's master key.
pub struct FileStore<'a> {
    dir: &'a Path,
    master: &'a [u8; 32],
}

impl<'a> FileStore<'a> {
    pub(super) fn new(dir: &'a Path, master: &'a [u8; 32]) -> Self {
        Self { dir, master }
    }

    fn key_for(&self, id: &str) -> aead::AeadKey {
        let derived = kdf::derive_32(self.master, id.as_bytes(), FILE_KEY_LABEL);
        aead::AeadKey::from_bytes(*derived)
    }

    fn path_for(&self, id: &str) -> PathBuf {
        // Hash the id to a fixed hex name: any id is a safe filename.
        self.dir.join(hex::encode(hash::hash(id.as_bytes())))
    }

    /// Seal `plaintext` under `id`. On-disk layout is `nonce || ciphertext`.
    pub fn put(&self, id: &str, plaintext: &[u8]) -> Result<(), StoreError> {
        let key = self.key_for(id);
        let nonce = aead::random_nonce();
        let ciphertext = aead::seal(&key, &nonce, id.as_bytes(), plaintext);

        let mut out = Vec::with_capacity(nonce.len() + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        std::fs::write(self.path_for(id), out).map_err(|_| StoreError::FileIo)
    }

    /// Read and open the file under `id`.
    pub fn get(&self, id: &str) -> Result<Vec<u8>, StoreError> {
        let raw = std::fs::read(self.path_for(id)).map_err(|_| StoreError::FileIo)?;
        if raw.len() < aead::NONCE_LEN {
            return Err(StoreError::Decrypt);
        }
        let (nonce, ciphertext) = raw.split_at(aead::NONCE_LEN);
        let nonce: [u8; aead::NONCE_LEN] = nonce.try_into().expect("checked length above");
        let key = self.key_for(id);
        let plaintext =
            aead::open(&key, &nonce, id.as_bytes(), ciphertext).map_err(|_| StoreError::Decrypt)?;
        Ok(plaintext.to_vec())
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
        assert_eq!(fs.get("photo-1").unwrap(), b"full-resolution bytes");
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
        assert!(matches!(
            FileStore::new(dir.path(), &[2u8; 32]).get("f"),
            Err(StoreError::Decrypt)
        ));
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
        assert_eq!(fs.get("../../etc/passwd").unwrap(), b"x");
        assert!(!dir.path().parent().unwrap().join("etc").exists());
    }
}
