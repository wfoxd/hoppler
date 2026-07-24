//! Contract tests for Core API v0 (T07). They drive the real `crate::api`
//! surface against the fake network + real store, asserting the observable
//! behaviour every UI/module will rely on. The engine is a process-wide
//! singleton, so these serialize on `LOCK`.

use std::sync::Mutex;

use rust_lib_hoppler::api::core::core_init;
use rust_lib_hoppler::api::discovery::{nearby_devices, set_discovery};
use rust_lib_hoppler::api::identity::{current_persona, update_persona};
use rust_lib_hoppler::api::messaging::{ping, send_chat, thread_messages};
use rust_lib_hoppler::api::transfers::offer_drop;

static LOCK: Mutex<()> = Mutex::new(());

fn fresh() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    core_init(dir.path().to_str().unwrap().to_string()).unwrap();
    dir
}

#[test]
fn discovery_toggle_controls_nearby_list() {
    let _g = LOCK.lock().unwrap();
    let _d = fresh();
    assert!(nearby_devices().unwrap().is_empty());
    set_discovery(true).unwrap();
    let devices = nearby_devices().unwrap();
    assert!(!devices.is_empty());
    assert!(devices.iter().any(|d| d.name == "Sam"));
    set_discovery(false).unwrap();
    assert!(nearby_devices().unwrap().is_empty());
}

#[test]
fn chat_round_trips_through_the_store() {
    let _g = LOCK.lock().unwrap();
    let _d = fresh();
    let out = send_chat("fake-sam".into(), "hi".into()).unwrap();
    assert!(out.outgoing);
    assert_eq!(out.text, "hi");

    // The thread now holds the outgoing message and the fake reply.
    let msgs = thread_messages(out.thread_id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(msgs.iter().any(|m| m.outgoing && m.text == "hi"));
    assert!(msgs.iter().any(|m| !m.outgoing && m.text.contains("hi")));
}

#[test]
fn persona_update_bumps_version_and_persists() {
    let _g = LOCK.lock().unwrap();
    let _d = fresh();
    let before = current_persona().unwrap();
    let after = update_persona("Alice".into(), 0x0011_2233).unwrap();
    assert_eq!(after.name, "Alice");
    assert_eq!(after.colour, 0x0011_2233);
    assert_eq!(after.version, before.version + 1);
    assert_eq!(current_persona().unwrap().name, "Alice");
}

#[test]
fn offer_drop_returns_a_transfer_id() {
    let _g = LOCK.lock().unwrap();
    let _d = fresh();
    let id = offer_drop("fake-sam".into(), "clip.mp4".into(), 5_000_000).unwrap();
    assert!(id.starts_with("xfer-"), "unexpected id {id}");
}

#[test]
fn ping_names_a_known_peer() {
    let _g = LOCK.lock().unwrap();
    let _d = fresh();
    // ping succeeds for a known device (the payload goes to the event stream,
    // which a Rust test has no sink for — the Dart integration test covers it).
    ping("fake-sam".into()).unwrap();
}
