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
use rust_lib_hoppler::identity::filekeystore::FileKeystore;
use rust_lib_hoppler::identity::{Identity, Persona};
use rust_lib_hoppler::store::{Direction, MessageState, NewContact, NewMessage};

fn a_persona() -> Persona {
    Persona {
        name: "Wren".into(),
        colour: 0x0044_88ff,
        version: 1,
    }
}

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

/// The identity itself, which is the thing a pairing is *of*.
///
/// A store that persists while the keys underneath it do not is arguably worse
/// than neither: the contact rows survive, so the app looks fine, and every one
/// of them refers to a person this device can no longer prove it is.
#[test]
fn an_identity_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys");

    let (layer1, layer2) = {
        let ks = FileKeystore::open(&path).unwrap();
        assert!(
            Identity::unsealed(&ks, a_persona()).unwrap().is_none(),
            "a device that has sealed nothing must read as a first launch"
        );
        let identity = Identity::generate("Wren", 0x0044_88ff);
        identity.seal_seeds(&ks).unwrap();
        (identity.layer1_public(), identity.layer2_public())
    };

    let ks = FileKeystore::open(&path).unwrap();
    let restored = Identity::unsealed(&ks, a_persona())
        .unwrap()
        .expect("the identity did not survive: this device is a new person");
    assert_eq!(restored.layer1_public().0, layer1.0, "Layer-1 changed");
    assert_eq!(restored.layer2_public().0, layer2.0, "Layer-2 changed");
}

/// A sealing that died between the two seeds must read as no identity rather
/// than as half of one — half would be a device holding one key it can prove
/// and one it invented.
#[test]
fn a_half_sealed_identity_is_no_identity() {
    use rust_lib_hoppler::identity::keystore::Keystore;

    for missing in [Identity::LAYER1_LABEL, Identity::LAYER2_LABEL] {
        let dir = tempfile::tempdir().unwrap();
        let ks = FileKeystore::open(dir.path().join("keys")).unwrap();
        Identity::generate("Wren", 1).seal_seeds(&ks).unwrap();
        ks.wipe(missing).unwrap();

        assert!(
            Identity::unsealed(&ks, a_persona()).unwrap().is_none(),
            "with {missing} gone, half an identity was rebuilt"
        );
    }
}

/// A seed that is not the right length is not a seed this build wrote.
/// Padding or truncating it would produce a *different* identity that looked
/// like a working one, which is the failure with no symptom.
#[test]
fn a_seed_of_the_wrong_length_is_refused() {
    use rust_lib_hoppler::identity::keystore::Keystore;

    let dir = tempfile::tempdir().unwrap();
    let ks = FileKeystore::open(dir.path().join("keys")).unwrap();
    Identity::generate("Wren", 1).seal_seeds(&ks).unwrap();
    ks.seal(Identity::LAYER2_LABEL, &[7u8; 16]).unwrap();

    assert!(
        Identity::unsealed(&ks, a_persona()).is_err(),
        "a 16-byte seed was stretched into an identity"
    );
}

/// A name chosen once is the name next launch, which is the whole point of the
/// step that is about to ask for one.
///
/// Goes through `update_persona`, the call the name screen will make, rather
/// than through the storage helper underneath it. Written the shorter way
/// first, and mutation testing said so: deleting the write inside
/// `update_persona` broke nothing, because the test never went near it.
///
/// The only test in this binary that installs the core, since `CORE` is
/// process-wide.
#[test]
fn a_renamed_persona_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    engine::init_on(path.clone(), engine::Radio::Lan).unwrap();
    engine::update_persona("Wren".into(), 0x0044_88ff).unwrap();

    // Next launch: a fresh store over the same directory, as `load_identity`
    // would open it.
    let store = engine::open_store_for_test(path).unwrap();
    assert_eq!(
        engine::stored_persona_for_test(&store)
            .unwrap()
            .map(|p| p.name),
        Some("Wren".to_string()),
        "the chosen name did not survive the app being closed"
    );
}
