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
use rust_lib_hoppler::transport::{Transport, TransportError, TransportEvent};

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

/// Wait until the pump thread has produced what the caller is waiting for.
///
/// A fixed sleep would be a flake on a loaded machine and a wasted second on a
/// fast one — and when it did fail it would say nothing about why. This polls
/// to a deadline and the caller says what it is waiting *for*, so a failure
/// names the condition that never came true.
fn until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn discovery_toggle_controls_the_nearby_list() {
    let _g = LOCK.lock().unwrap();
    let h = fresh();
    let (_peer, _rx) = advertising_peer(&h.air, "peer-one");

    assert!(nearby_devices().unwrap().is_empty(), "visible while off");

    set_discovery(true).unwrap();
    until("an advertising peer to appear", || {
        nearby_devices()
            .map(|d| d.iter().any(|d| d.device_id == "peer-one"))
            .unwrap_or(false)
    });
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
    until("the sighting to arrive", || {
        nearby_devices()
            .map(|d| d.iter().any(|d| d.device_id == "nameless"))
            .unwrap_or(false)
    });
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
    // A device that was never seen is not reachable, whatever its id — no wait
    // needed, since nothing is expected to arrive.
    assert!(ping("never-seen".into()).is_err());
}

/// A transport that records every dial, and otherwise gets out of the way.
struct CountingDials {
    inner: Arc<dyn Transport>,
    dials: Arc<Mutex<Vec<String>>>,
}

impl Transport for CountingDials {
    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        self.dials
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(peer.to_string());
        self.inner.connect(peer)
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn limits(&self) -> rust_lib_hoppler::transport::TransportLimits {
        self.inner.limits()
    }
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }
    fn set_local_id(&self, id: &str) -> Result<(), TransportError> {
        self.inner.set_local_id(id)
    }
    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        self.inner.start_advertising(payload)
    }
    fn stop_advertising(&self) -> Result<(), TransportError> {
        self.inner.stop_advertising()
    }
    fn start_scanning(&self) -> Result<(), TransportError> {
        self.inner.start_scanning()
    }
    fn stop_scanning(&self) -> Result<(), TransportError> {
        self.inner.stop_scanning()
    }
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        self.inner.send(peer, bytes)
    }
    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        self.inner.disconnect(peer)
    }
    fn peers(&self) -> Vec<rust_lib_hoppler::transport::PeerId> {
        self.inner.peers()
    }
    fn pipes(&self) -> Vec<rust_lib_hoppler::transport::PeerId> {
        self.inner.pipes()
    }
    fn shutdown(&self) {
        self.inner.shutdown()
    }
}

/// One tap is one dial.
///
/// `engine::ping` used to call `reach` and then `Net::ping`, which reaches for
/// itself — two dials for one tap. Every rung but one absorbs that: a duplicate
/// TCP connect to a peer already being connected is harmless, which is why ~190
/// tests and a full LAN acceptance never noticed.
///
/// BLE does not absorb it. Two L2CAP channels opened to one remote in the same
/// millisecond do not both succeed, and on two phones neither did — five taps,
/// ten dials, no session. The adapter's logging showed two `dialling` lines per
/// tap with timestamps a microsecond apart, which is the only reason this was
/// found at all.
///
/// Counted rather than asserted as "at least one", because at-least-one is what
/// the broken version satisfied.
#[test]
fn one_tap_is_one_dial() {
    let _g = LOCK.lock().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let dials = Arc::new(Mutex::new(Vec::new()));
    let transport: Arc<dyn Transport> = Arc::new(CountingDials {
        inner: Arc::new(air.join("core", sink)),
        dials: Arc::clone(&dials),
    });
    init_with_transport(
        dir.path().to_str().unwrap().to_string(),
        transport,
        "core",
        rx,
    )
    .unwrap();

    let (_peer, _rx) = advertising_peer(&air, "peer-one");
    set_discovery(true).unwrap();
    until("an advertising peer to appear", || {
        nearby_devices()
            .map(|d| d.iter().any(|d| d.device_id == "peer-one"))
            .unwrap_or(false)
    });

    // Dials from discovery itself are not what is being counted; only what the
    // tap adds.
    dials.lock().unwrap().clear();
    ping("peer-one".into()).unwrap();

    let dialled: Vec<String> = dials
        .lock()
        .unwrap()
        .iter()
        .filter(|p| *p == "peer-one")
        .cloned()
        .collect();
    assert_eq!(
        dialled.len(),
        1,
        "one tap must be one dial, got {}: {dialled:?}",
        dialled.len()
    );
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
    // Not asserted here: that the row is still `Queued` rather than `Sent`.
    // `state` is not on the DTO and no API surface exposes it, and inventing
    // one for a test would be growing the bridge to suit the suite. The
    // transition is covered where it belongs — `store::records` already tests
    // `messages_by_state` — and a real assertion arrives with the resend queue
    // in T12/T16, which is the first caller that needs to read it.
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

// ── contact identity across a rotation ────────────────────────────────────────

struct SessionPeer {
    net: rust_lib_hoppler::engine::net::Net,
    rx: Receiver<TransportEvent>,
    transport: Arc<dyn Transport>,
}

/// A peer that will complete a handshake with the engine, so the core holds a
/// pseudonym for it and not merely a device id.
///
/// Takes the identity rather than making one, because the whole point is to
/// bring the *same person* back under a different device id.
fn session_peer(air: &LoopbackNet, id: &str, identity: Arc<Mutex<Identity>>) -> SessionPeer {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    let net =
        rust_lib_hoppler::engine::net::Net::new(transport.clone(), identity, id, Instant::now());
    // Discovery on, or the peer never answers the persona request — and an IK
    // handshake cannot start without the responder's static key in advance, so
    // the session would never form and the rotation would go untested.
    net.discovery().set_enabled(true, Instant::now()).unwrap();
    SessionPeer { net, rx, transport }
}

/// Drive the peer's side until it holds a session with the engine.
///
/// Only this side needs pumping — the engine runs its own pump thread. Waits on
/// the session itself rather than a sleep, so a failure here means the
/// handshake did not happen, not that the machine was slow.
fn until_session(p: &SessionPeer) {
    p.transport.connect("core").unwrap();
    until("the peer to hold a session with the engine", || {
        let now = Instant::now();
        while let Ok(e) = p.rx.recv_timeout(Duration::from_millis(20)) {
            p.net.handle(e, now);
        }
        p.net.sessions().is_open("core")
    });
}

/// One person, two device ids, one conversation.
///
/// R0-F2 rotates the device id every twelve minutes. Keying a contact on it
/// meant the same person became a new contact, a new thread and a new
/// conversation four or five times an hour — the history still on disk, just
/// scattered across rows that no longer looked like anybody.
///
/// The session pseudonym is the peer's static DH public, which the Noise IK
/// handshake authenticates, and it does not rotate. Both sends below succeed on
/// the wire, which is what proves a session existed for each id: without one the
/// engine would fall back to the id and this would pass for the wrong reason.
#[test]
fn a_thread_survives_the_peers_device_id_rotating() {
    let _g = LOCK.lock().unwrap();
    let h = fresh();
    set_discovery(true).unwrap();

    let wanda = Arc::new(Mutex::new(Identity::generate("Wanda", 0x00_88_ff)));

    let before = session_peer(&h.air, "wanda-before", wanda.clone());
    until_session(&before);
    send_chat("wanda-before".into(), "before the rotation".into())
        .expect("no session under the first id, so the rotation is not what is under test");

    // The rotation: same identity, same static key, new device id.
    let after = session_peer(&h.air, "wanda-after", wanda.clone());
    until_session(&after);
    send_chat("wanda-after".into(), "after the rotation".into())
        .expect("no session under the second id, so the rotation is not what is under test");

    let threads = list_threads().unwrap();
    assert_eq!(
        threads.len(),
        1,
        "the rotation split one person into {} conversations",
        threads.len()
    );
    let thread = thread_for_device("wanda-after".into()).unwrap().unwrap();
    let texts: Vec<String> = thread_messages(thread)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert!(
        texts.iter().any(|t| t == "before the rotation")
            && texts.iter().any(|t| t == "after the rotation"),
        "history did not follow the person across the rotation: {texts:?}"
    );
}
