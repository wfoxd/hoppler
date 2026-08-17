//! Does anything survive the app being closed?
//!
//! Until 18 Aug 2026 the answer on a phone was no, and nothing noticed. The
//! keystore holding the store's master was an in-memory `HashMap`, so a fresh
//! launch found no master, concluded the database on disk was unkeyable, and
//! deleted it. Measured on a Pixel 8 Pro: `hoppler.db` came back with a
//! different inode after every restart.
//!
//! No test caught it because every test that opened a store opened it once.
//! That is the shape of the gap — not a wrong assertion, an absent second act —
//! so these tests are written as two acts with the first one's handles dropped
//! in between.

use rust_lib_hoppler::engine;
use rust_lib_hoppler::store::{Direction, MessageState, NewContact, NewMessage};

/// A contact written before a restart is there after it.
///
/// Goes through the same `open_store` the app uses rather than constructing a
/// `Store` directly: what broke was not the store but the keystore underneath
/// it, and a test that supplied its own keystore would have passed throughout.
#[test]
fn a_contact_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let contact = {
        let store = engine::open_store_for_test(path.clone()).unwrap();
        store
            .add_contact(&NewContact {
                pseudonym: [4u8; 32],
                l2_pub: [5u8; 32],
                name: "Wren".into(),
                colour: 0x0044_88ff,
                persona_version: 1,
                first_seen: 1000,
            })
            .unwrap()
    };

    // Second launch. The first store is dropped, its connection closed, and
    // nothing is carried across but the directory.
    let store = engine::open_store_for_test(path).unwrap();
    let found = store.contact_by_id(contact).unwrap();
    assert_eq!(
        found.map(|c| c.name),
        Some("Wren".to_string()),
        "the contact did not survive — the database was reset on the second \
         open, which is what an empty keystore causes"
    );
}

/// And the thing T12 spent three slices making durable: a thread's ratchet
/// state and the message that moved it.
///
/// The point is not that the store can hold them — that has its own tests —
/// but that they are still there next time, which is the only sense in which
/// "persistent threads" means anything.
#[test]
fn a_conversation_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let thread = {
        let store = engine::open_store_for_test(path.clone()).unwrap();
        let contact = store
            .add_contact(&NewContact {
                pseudonym: [6u8; 32],
                l2_pub: [7u8; 32],
                name: "Wren".into(),
                colour: 1,
                persona_version: 1,
                first_seen: 1000,
            })
            .unwrap();
        let thread = store.pair_contact(contact, &[8u8; 32], 2000).unwrap();
        store
            .commit_received(
                b"ratchet state",
                &Default::default(),
                &NewMessage {
                    thread_id: thread,
                    seq: 1,
                    msg_id: vec![1u8; 16],
                    body: b"still here?".to_vec(),
                    direction: Direction::Incoming,
                    state: MessageState::Delivered,
                    created_at: 3000,
                },
                3000,
            )
            .unwrap();
        thread
    };

    let store = engine::open_store_for_test(path).unwrap();
    assert_eq!(
        store.ratchet_state(thread).unwrap().map(|s| s.to_vec()),
        Some(b"ratchet state".to_vec()),
        "the ratchet did not survive — every paired conversation would have to \
         be started again, with no way to say why"
    );
    let messages = store.messages_for_thread(thread).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, b"still here?");
}

/// The reset path is still there and still right — but only when it is genuinely
/// the case that nothing can decrypt the database.
///
/// Destroying the keystore is what a wipe (R0-F9) leaves behind, and a database
/// whose master is gone is bytes nobody will ever read. Resetting it is correct;
/// what was wrong was that this happened on every ordinary launch.
#[test]
fn a_database_whose_key_is_gone_is_reset_rather_than_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    {
        let store = engine::open_store_for_test(path.clone()).unwrap();
        store
            .add_contact(&NewContact {
                pseudonym: [9u8; 32],
                l2_pub: [9u8; 32],
                name: "Gone".into(),
                colour: 1,
                persona_version: 1,
                first_seen: 1000,
            })
            .unwrap();
    }

    std::fs::remove_dir_all(dir.path().join("keys")).unwrap();

    let store = engine::open_store_for_test(path)
        .expect("a database with no key must open as a fresh one, not fail");
    assert!(
        store.list_contacts().unwrap().is_empty(),
        "the old rows came back, which would mean they were readable without \
         the master"
    );
}
