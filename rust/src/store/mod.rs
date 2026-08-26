//! The encrypted store (tech spec §11; requirement R0-F9 backend, G-2).
//!
//! All persistent state lives in one SQLCipher database plus an encrypted file
//! store, both keyed under a single 32-byte master. The master is sealed in the
//! platform [`keystore`](crate::identity::keystore); per-file keys are derived
//! from it. That single point of key control is what makes wipe a
//! *crypto-erase*: destroy the master and both the database and every file
//! become undecryptable, before a byte is deleted.
//!
//! `Store` owns a `rusqlite::Connection`, which is `Send` but not `Sync`, so the
//! core accesses the store through a single owner (or a mutex), never shared
//! concurrently.

mod files;
mod records;
mod schema;

pub use records::{
    BlockEntry, Contact, Direction, InboxPosition, InsertOutcome, Message, MessageState,
    NewContact, NewMessage, NewTransfer, Pairing, Transfer, TransferState,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::crypto::rng;
use crate::identity::keystore::{Keystore, KeystoreError};

pub use files::FileStore;

/// Keystore label under which the database master key is sealed.
const MASTER_LABEL: &str = "hoppler/store-master/v1";
const MASTER_LEN: usize = 32;

/// The schema version this build migrates to (exposed for T07 diagnostics).
pub const LATEST_SCHEMA_VERSION: i64 = schema::LATEST_VERSION;

/// Errors from the store. `Db` carries the backend's diagnostic string (for
/// logging, never a secret); the other variants are coarse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The keystore backend failed.
    Keystore(KeystoreError),
    /// A database operation failed.
    Db(String),
    /// The sealed master was not 32 bytes.
    CorruptMaster,
    /// A file-store I/O operation failed.
    FileIo,
    /// A file failed to decrypt (wrong key or tampered).
    Decrypt,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Keystore(e) => write!(f, "keystore: {e}"),
            StoreError::Db(e) => write!(f, "database: {e}"),
            StoreError::CorruptMaster => write!(f, "sealed master is corrupt"),
            StoreError::FileIo => write!(f, "file store I/O error"),
            StoreError::Decrypt => write!(f, "file decryption failed"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<KeystoreError> for StoreError {
    fn from(e: KeystoreError) -> Self {
        StoreError::Keystore(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e.to_string())
    }
}

/// The database master key. Not printable; zeroized on drop.
struct MasterKey(Zeroizing<[u8; MASTER_LEN]>);

impl MasterKey {
    fn generate() -> Self {
        Self(Zeroizing::new(rng::random_array::<MASTER_LEN>()))
    }

    fn from_slice(bytes: &[u8]) -> Result<Self, StoreError> {
        let arr: [u8; MASTER_LEN] = bytes.try_into().map_err(|_| StoreError::CorruptMaster)?;
        Ok(Self(Zeroizing::new(arr)))
    }

    fn as_bytes(&self) -> &[u8; MASTER_LEN] {
        &self.0
    }
}

/// The encrypted store.
pub struct Store {
    conn: Connection,
    keystore: Arc<dyn Keystore>,
    master: MasterKey,
    db_path: PathBuf,
    files_dir: PathBuf,
}

impl Store {
    /// Open (or create) the store at `db_path`, with the encrypted file store in
    /// `files_dir`. The master is unsealed from `keystore`, or generated and
    /// sealed on first launch. Runs forward-only migrations to the current
    /// schema version.
    pub fn open(
        keystore: Arc<dyn Keystore>,
        db_path: impl Into<PathBuf>,
        files_dir: impl Into<PathBuf>,
    ) -> Result<Self, StoreError> {
        let db_path = db_path.into();
        let files_dir = files_dir.into();

        let master = match keystore.unseal(MASTER_LABEL) {
            Ok(bytes) => MasterKey::from_slice(&bytes)?,
            Err(KeystoreError::NotFound) => {
                let m = MasterKey::generate();
                keystore.seal(MASTER_LABEL, m.as_bytes())?;
                m
            }
            Err(e) => return Err(e.into()),
        };

        std::fs::create_dir_all(&files_dir).map_err(|_| StoreError::FileIo)?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| StoreError::FileIo)?;
        }

        let conn = Connection::open(&db_path)?;
        key_database(&conn, master.as_bytes())?;
        // Scrub SQLite's page cache and key schedule on free (SQLCipher 4
        // defaults this OFF): narrows the RAM-forensics window after erase.
        conn.execute_batch("PRAGMA cipher_memory_security = ON;")?;
        // WAL: pairs with the single-writer model and keeps the -wal/-shm the
        // crypto-erase deletes the ones actually in use. SQLCipher encrypts the
        // WAL with the same key, so no plaintext leaks there.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        schema::migrate(&conn)?;

        Ok(Self {
            conn,
            keystore,
            master,
            db_path,
            files_dir,
        })
    }

    /// The current schema version.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        schema::current_version(&self.conn).map_err(Into::into)
    }

    /// The encrypted file store handle.
    pub fn files(&self) -> FileStore<'_> {
        FileStore::new(&self.files_dir, self.master.as_bytes())
    }

    /// Whether a store master is already sealed in `keystore`. Lets a caller
    /// decide, on an open failure, whether a stale DB is safe to reset: only
    /// when no master exists is the ciphertext provably unrecoverable.
    ///
    /// Only a definite `NotFound` counts as "no master". A backend error is
    /// ambiguous and reported as sealed, so a transient keystore failure never
    /// makes a caller destroy a DB whose master may still exist.
    pub fn master_is_sealed(keystore: &dyn Keystore) -> bool {
        !matches!(keystore.unseal(MASTER_LABEL), Err(KeystoreError::NotFound))
    }

    /// Crypto-erase the store (R0-F9). Destroys the keystore master first — the
    /// point of no return, after which the database and all files are
    /// undecryptable even if their bytes survive — then best-effort deletes
    /// them. An interruption after the master is gone still leaves nothing
    /// recoverable.
    pub fn crypto_erase(self) -> Result<(), StoreError> {
        self.keystore.wipe(MASTER_LABEL)?;
        drop(self.conn); // close the database handle before deleting the file
        let _ = std::fs::remove_file(&self.db_path);
        for suffix in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(with_suffix(&self.db_path, suffix));
        }
        let _ = std::fs::remove_dir_all(&self.files_dir);
        Ok(())
    }
}

/// Apply the SQLCipher raw key. The key must be a literal `x'HEX'`, so it can't
/// be a bound parameter — this is the one place key bytes touch a format string.
fn key_database(conn: &Connection, master: &[u8; MASTER_LEN]) -> Result<(), StoreError> {
    // Both the hex of the key and the assembled PRAGMA are Zeroizing, so no
    // un-scrubbed copy of the master survives on the heap.
    let hex_key = Zeroizing::new(hex::encode(master));
    let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex_key.as_str()));
    conn.execute_batch(&pragma)?;
    // Force a read so a wrong key fails now, not later.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::keystore::SoftwareKeystore;

    fn temp_store() -> (Store, tempfile::TempDir, Arc<dyn Keystore>) {
        let dir = tempfile::tempdir().unwrap();
        let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        let store = Store::open(
            Arc::clone(&ks),
            dir.path().join("hoppler.db"),
            dir.path().join("files"),
        )
        .unwrap();
        (store, dir, ks)
    }

    #[test]
    fn opens_and_reports_current_schema_version() {
        let (store, _dir, _ks) = temp_store();
        assert_eq!(store.schema_version().unwrap(), schema::LATEST_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let (store, _dir, _ks) = temp_store();
        // Re-running migrate on an up-to-date DB is a no-op, not an error.
        schema::migrate(&store.conn).unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::LATEST_VERSION);
    }

    #[test]
    fn reopen_with_same_keystore_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        let db = dir.path().join("hoppler.db");
        let files = dir.path().join("files");
        {
            let s = Store::open(Arc::clone(&ks), &db, &files).unwrap();
            s.settings_set("k", b"v").unwrap();
        }
        // Reopen: same master unsealed, data intact.
        let s = Store::open(Arc::clone(&ks), &db, &files).unwrap();
        assert_eq!(s.settings_get("k").unwrap().as_deref(), Some(&b"v"[..]));
    }

    #[test]
    fn contacts_and_messages_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        let db = dir.path().join("hoppler.db");
        let files = dir.path().join("files");
        {
            let s = Store::open(Arc::clone(&ks), &db, &files).unwrap();
            let c = s
                .add_contact(&NewContact {
                    pseudonym: [4u8; 32],
                    l2_pub: [5u8; 32],
                    name: "Bob".into(),
                    colour: 0x00abcdef,
                    persona_version: 1,
                    first_seen: 10,
                })
                .unwrap();
            let t = s.create_thread(c, 11).unwrap();
            s.add_message(&NewMessage {
                thread_id: t,
                seq: 1,
                msg_id: b"m1".to_vec(),
                body: b"hi".to_vec(),
                direction: Direction::Incoming,
                state: MessageState::Delivered,
                created_at: 12,
            })
            .unwrap();
        }
        // Reopen: migrate must not wipe, and blobs must persist.
        let s = Store::open(Arc::clone(&ks), &db, &files).unwrap();
        let c = s.contact_by_pseudonym(&[4u8; 32]).unwrap().unwrap();
        assert_eq!(c.name, "Bob");
        let t = s.thread_for_contact(c.id).unwrap().unwrap();
        assert_eq!(s.messages_for_thread(t).unwrap()[0].body, b"hi");
    }

    #[test]
    fn wrong_master_cannot_open_existing_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hoppler.db");
        let files = dir.path().join("files");
        {
            let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
            let _ = Store::open(ks, &db, &files).unwrap();
        }
        // A fresh keystore has a different master — the encrypted DB won't open.
        let other: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        assert!(Store::open(other, &db, &files).is_err());
    }

    #[test]
    fn database_file_has_no_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hoppler.db");
        {
            let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
            let s = Store::open(ks, &db, dir.path().join("files")).unwrap();
            s.settings_set("secret", b"TOPSECRET-BURROW-CONTENTS")
                .unwrap();
            // Ensure it is flushed to the main db file.
            s.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
        }
        let raw = std::fs::read(&db).unwrap();
        assert!(
            !contains_subslice(&raw, b"TOPSECRET-BURROW-CONTENTS"),
            "plaintext leaked into the database file"
        );
        // The SQLCipher header is not the plaintext "SQLite format 3" magic.
        assert!(!raw.starts_with(b"SQLite format 3\0"));
    }

    #[test]
    fn crypto_erase_destroys_master_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let ks: Arc<dyn Keystore> = Arc::new(SoftwareKeystore::new());
        let db = dir.path().join("hoppler.db");
        let files_dir = dir.path().join("files");
        let s = Store::open(Arc::clone(&ks), &db, &files_dir).unwrap();
        s.settings_set("k", b"v").unwrap();
        s.files().put("attachment", b"photo bytes").unwrap();
        assert!(files_dir.exists());

        // Copy the encrypted DB bytes before erasing.
        let pre_erase = std::fs::read(&db).unwrap();
        s.crypto_erase().unwrap();

        // The master is gone: nothing can key the surviving copy.
        assert_eq!(
            ks.unseal(MASTER_LABEL).unwrap_err(),
            KeystoreError::NotFound
        );
        assert!(!db.exists());
        assert!(!files_dir.exists()); // the file store directory is gone too
        assert!(!pre_erase.is_empty()); // the copy exists but is now orphaned ciphertext
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
