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
        let thread = store
            .pair_contact_for_test(contact, &[8u8; 32], 2000)
            .unwrap();
        store
            .commit_received(
                Some(b"ratchet state"),
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
    let store = engine::open_store_for_test(path.clone()).unwrap();
    assert_eq!(
        engine::stored_persona_for_test(&store)
            .unwrap()
            .map(|p| p.name),
        Some("Wren".to_string()),
        "the chosen name did not survive the app being closed"
    );
}

/// A keystore that fails must not read as a device that has never sealed
/// anything.
///
/// This is the one that would have cost an identity. Read as "first launch",
/// the engine generates a new one and seals it *over* the seed that was there
/// but momentarily unreadable — destroying, on a transient IO error, the only
/// key material on the device that cannot be recreated.
#[test]
fn a_failing_keystore_is_not_mistaken_for_a_first_launch() {
    use rust_lib_hoppler::identity::keystore::{Keystore, KeystoreError};
    use std::sync::Mutex;

    /// Layer-1 present and readable; Layer-2 there but refusing.
    struct HalfBroken(Mutex<Vec<u8>>);
    impl Keystore for HalfBroken {
        fn seal(&self, _: &str, _: &[u8]) -> Result<(), KeystoreError> {
            Ok(())
        }
        fn unseal(&self, label: &str) -> Result<zeroize::Zeroizing<Vec<u8>>, KeystoreError> {
            if label == Identity::LAYER1_LABEL {
                Ok(zeroize::Zeroizing::new(self.0.lock().unwrap().clone()))
            } else {
                Err(KeystoreError::Backend)
            }
        }
        fn wipe(&self, _: &str) -> Result<(), KeystoreError> {
            Ok(())
        }
    }

    let ks = HalfBroken(Mutex::new(vec![3u8; 32]));
    assert_eq!(
        Identity::unsealed(&ks, a_persona()).err(),
        Some(KeystoreError::Backend),
        "a backend failure read as a first launch, which would have sealed a \
         new identity over the one that was there"
    );
}

/// A corrupt persona record falls back to the default rather than becoming a
/// live persona. Every invariant the writer holds, checked on the way in.
#[test]
fn a_corrupt_persona_is_ignored_rather_than_used() {
    use rust_lib_hoppler::identity::{COLOUR_MASK, MAX_PERSONA_NAME_LEN};

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("too short", vec![0u8; 7]),
        ("version zero", {
            let mut v = vec![0u8; 8];
            v.extend_from_slice(b"Wren");
            v
        }),
        ("colour with a top byte", {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend_from_slice(&(COLOUR_MASK + 1).to_be_bytes());
            v.extend_from_slice(b"Wren");
            v
        }),
        ("name over the bound", {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&[b'x'; MAX_PERSONA_NAME_LEN + 1]);
            v
        }),
        // Refused on length before a byte of it is examined, so a corrupt row
        // cannot choose how much work startup does.
        ("enormous", {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&vec![b'x'; 4 * 1024 * 1024]);
            v
        }),
        ("name is not text", {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&[0xff, 0xfe]);
            v
        }),
    ];

    for (why, bytes) in cases {
        let dir = tempfile::tempdir().unwrap();
        let store = engine::open_store_for_test(dir.path().to_string_lossy().into_owned()).unwrap();
        store.settings_set("identity/persona/v1", &bytes).unwrap();
        assert_eq!(
            engine::stored_persona_for_test(&store).unwrap(),
            None,
            "a persona that is {why} was read back as usable"
        );
    }

    // And a good one still reads, so what is refused is the corruption and not
    // the format.
    let dir = tempfile::tempdir().unwrap();
    let store = engine::open_store_for_test(dir.path().to_string_lossy().into_owned()).unwrap();
    engine::store_persona_for_test(&store, &a_persona()).unwrap();
    assert_eq!(
        engine::stored_persona_for_test(&store).unwrap(),
        Some(a_persona())
    );
}

/// R0-F1's first-launch question: has anyone chosen this device's name?
///
/// Derived from the stored persona rather than a flag, so this checks the
/// derivation rather than a boolean somebody set. A device that has sealed an
/// identity but stored no persona has never been asked; one with a stored
/// persona has.
#[test]
fn a_device_asks_for_a_name_until_one_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    let store = engine::open_store_for_test(path.clone()).unwrap();
    assert!(
        engine::needs_name_for_test(&store),
        "a device with no stored persona must ask — otherwise the placeholder \
         ships as the real name, which is how every install came to be 'Me'"
    );

    engine::store_persona_for_test(&store, &a_persona()).unwrap();
    assert!(
        !engine::needs_name_for_test(&store),
        "a device whose owner chose a name must not be asked again"
    );

    // And a persona too damaged to read asks again, which is the right answer:
    // the name genuinely is not known.
    store
        .settings_set("identity/persona/v1", &[0u8; 3])
        .unwrap();
    assert!(engine::needs_name_for_test(&store));
}

/// The launch path itself, which is where the placeholder could be written
/// down by accident.
///
/// Two launches, because the second is the one that matters: the first seals an
/// identity, and if it stored the made-up persona while doing so, the device
/// would come back on the second launch already believing somebody had chosen
/// to be called "Me".
#[test]
fn sealing_an_identity_does_not_count_as_choosing_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();

    assert!(
        engine::launch_needs_name_for_test(path.clone()).unwrap(),
        "a first launch must still ask"
    );
    assert!(
        engine::launch_needs_name_for_test(path.clone()).unwrap(),
        "the second launch stopped asking, so sealing an identity was taken \
         for choosing a name"
    );

    // And once a name is chosen, no launch asks again.
    let store = engine::open_store_for_test(path.clone()).unwrap();
    engine::store_persona_for_test(&store, &a_persona()).unwrap();
    drop(store);
    assert!(!engine::launch_needs_name_for_test(path).unwrap());
}
