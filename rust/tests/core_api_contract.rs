//! Contract tests for Core API v0. They drive the real `crate::api` surface
//! against the real store and a **real transport** — the loopback rung, so the
//! engine's own networking runs without opening sockets.
//!
//! `CORE` is process-wide, so these serialize on `LOCK` and cannot stand two
//! engines against each other. `engine::init_with_transport` exists for exactly
//! that reason; a second peer is built from `Discovery` directly.
//!
//! Fakes are gone. What changed observably, and is asserted below: the nearby
//! list is now whatever the radio sees, `ping` is acceptance rather than
//! delivery (a real peer acks when it answers, so `Pinged` is inbound), and
//! `send_chat` needs a session — though it still stores the outgoing row first,
//! so a failed send loses nothing.

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_lib_hoppler::api::discovery::{nearby_devices, set_discovery};
use rust_lib_hoppler::api::identity::{current_persona, update_persona};
use rust_lib_hoppler::api::messaging::{
    list_threads, ping, send_chat, thread_for_device, thread_messages,
};
use rust_lib_hoppler::api::transfers::offer_drop;
use rust_lib_hoppler::discovery::Discovery;
use rust_lib_hoppler::engine::init_with_transport;
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportEvent};

static LOCK: Mutex<()> = Mutex::new(());

struct Harness {
    _dir: tempfile::TempDir,
    air: LoopbackNet,
}

/// A fresh engine on its own loopback airspace.
fn fresh() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join("core", sink));
    init_with_transport(
        dir.path().to_str().unwrap().to_string(),
        transport,
        "core",
        rx,
    )
    .unwrap();
    Harness { _dir: dir, air }
}

/// A peer that advertises, so the engine has something to see.
fn advertising_peer(air: &LoopbackNet, id: &str) -> (Discovery, Receiver<TransportEvent>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    let identity = Arc::new(Mutex::new(Identity::generate(id, 0x00_ff_00)));
    let d = Discovery::new(transport, identity, Instant::now());
    d.set_enabled(true, Instant::now()).unwrap();
    (d, rx)
}

/// The engine's pump runs on its own thread; give it a moment to catch up.
fn settle() {
    std::thread::sleep(Duration::from_millis(200));
}

#[test]
fn discovery_toggle_controls_the_nearby_list() {
    let _g = LOCK.lock().unwrap();
    let h = fresh();
    let (_peer, _rx) = advertising_peer(&h.air, "peer-one");

    assert!(nearby_devices().unwrap().is_empty(), "visible while off");

    set_discovery(true).unwrap();
    settle();
    let devices = nearby_devices().unwrap();
    assert!(
        devices.iter().any(|d| d.device_id == "peer-one"),
        "an advertising peer was not seen; ids: {:?}",
        devices
            .iter()
            .map(|d| d.device_id.clone())
            .collect::<Vec<_>>()
    );

    // Off hides the list even though the rung still remembers the sighting —
    // that suppression is the core's decision, not the transport's.
    set_discovery(false).unwrap();
    assert!(nearby_devices().unwrap().is_empty());
}

#[test]
fn a_peer_with_no_persona_yet_is_still_listed() {
    // Real discovery reports a sighting before the persona round trip
    // finishes. Hiding it until then would make the list lag the radio, which
    // is the opposite of what "who's nearby" is asking.
    let _g = LOCK.lock().unwrap();
    let h = fresh();
    let (_peer, _rx) = advertising_peer(&h.air, "nameless");
    set_discovery(true).unwrap();
    settle();
    let devices = nearby_devices().unwrap();
    let seen = devices.iter().find(|d| d.device_id == "nameless");
    assert!(
        seen.is_some(),
        "not listed at all; ids: {:?}",
        devices
            .iter()
            .map(|d| d.device_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        seen.unwrap().name,
        "",
        "a name appeared before it was fetched"
    );
}

#[test]
fn ping_requires_discovery_and_a_reachable_peer() {
    let _g = LOCK.lock().unwrap();
    let h = fresh();
    let (_peer, _rx) = advertising_peer(&h.air, "peer-one");

    assert!(
        ping("peer-one".into()).is_err(),
        "pinged with discovery off"
    );

    set_discovery(true).unwrap();
    settle();
    // A device that was never seen is not reachable, whatever its id.
    assert!(ping("never-seen".into()).is_err());
}

#[test]
fn a_chat_with_no_session_still_stores_the_outgoing_row() {
    // The row is written before the send is attempted, so a failure leaves the
    // message in the thread rather than losing what the person typed.
    let _g = LOCK.lock().unwrap();
    let _h = fresh();
    assert!(send_chat("unreachable".into(), "hi".into()).is_err());

    let thread = thread_for_device("unreachable".into()).unwrap();
    assert!(thread.is_some(), "no thread was created for a failed send");
    let msgs = thread_messages(thread.unwrap()).unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].outgoing && msgs[0].text == "hi");
}

#[test]
fn threads_are_reused_and_messages_stay_in_order() {
    let _g = LOCK.lock().unwrap();
    let _h = fresh();
    // No session, so each send fails on the wire — but the rows are written
    // first, which is what this asserts. Ordering is by insertion, not by the
    // per-sender `seq` (which cannot be a display key) or the coarse clock.
    for text in ["one", "two", "three"] {
        let _ = send_chat("peer".into(), text.to_string());
    }

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let msgs = thread_messages(thread).unwrap();
    assert_eq!(
        msgs.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );
    assert_eq!(list_threads().unwrap().len(), 1, "a thread per send");
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
