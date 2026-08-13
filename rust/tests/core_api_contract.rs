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
use rust_lib_hoppler::api::pairing::{
    begin_pairing, confirm_pairing, pairing_invite, stop_showing_invite,
};
use rust_lib_hoppler::api::transfers::offer_drop;
use rust_lib_hoppler::discovery::Discovery;
use rust_lib_hoppler::engine::{has_session, init_with_transport};
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::pairing::invite::Invite;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportError, TransportEvent};

/// Serialises these tests against the process-wide `CORE`.
///
/// Recovered rather than unwrapped at every use. A test that panics while
/// holding this poisons it, and `unwrap` then fails every *other* test with
/// `PoisonError` — nine red tests and one real one, with the failure that
/// matters buried. Observed exactly that way while writing the pairing tests
/// below; the cascade is much harder to read than the fault.
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
///
/// `FnMut` because some conditions have to accumulate: an event that arrives
/// exactly once cannot be observed by a predicate that only ever looks at the
/// present moment.
fn until(what: &str, mut cond: impl FnMut() -> bool) {
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());

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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _d = fresh();
    let id = offer_drop("fake-sam".into(), "clip.mp4".into(), 5_000_000).unwrap();
    assert!(id.starts_with("xfer-"), "unexpected id {id}");
}

// ── contact identity across a rotation ────────────────────────────────────────

struct SessionPeer {
    net: rust_lib_hoppler::engine::net::Net,
    rx: Receiver<TransportEvent>,
    transport: Arc<dyn Transport>,
    /// The id the engine knows this peer by, so the wait can ask the engine's
    /// side about it and not only the peer's.
    id: String,
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
    SessionPeer {
        net,
        rx,
        transport,
        id: id.to_string(),
    }
}

/// Drive the peer's side until it holds a session with the engine.
///
/// Only this side needs pumping — the engine runs its own pump thread. Waits on
/// the session itself rather than a sleep, so a failure here means the
/// handshake did not happen, not that the machine was slow.
///
/// Wait until **both** sides hold the session.
///
/// The peer's half is not enough, and waiting only on it is a race every caller
/// here loses. The engine adopts its own side on the pump thread, so a test can
/// see the peer connected and still make its assertions before the engine has
/// reconciled the contact or become able to send. Idle, the pump wins and this
/// is invisible; under CPU load the two tests below failed 13 times in 15, and
/// on CI they failed for real.
fn until_session(p: &SessionPeer) {
    p.transport.connect("core").unwrap();
    until("the peer to hold a session with the engine", || {
        let now = Instant::now();
        while let Ok(e) = p.rx.recv_timeout(Duration::from_millis(20)) {
            p.net.handle(e, now);
        }
        p.net.sessions().is_open("core")
    });
    until("the engine to hold the session too", || has_session(&p.id));
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
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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

/// The session reconciles the contact — the next message does not have to.
///
/// A chat can be written before anyone has proved who they are, and that row
/// lands under the device id. Reconciling only on the *next* send or receive
/// would leave a window where the UI opens a conversation by device id and
/// lands on a thread that is about to be folded into another.
///
/// The rotation at the end is what makes this observable: after reconciling,
/// the row is keyed on the pseudonym, so it is findable under an id it was
/// never written with. Lazily reconciled, it would still be under the old id
/// and this lookup would come back empty.
#[test]
fn a_session_reconciles_a_contact_written_before_it() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    set_discovery(true).unwrap();

    // No session yet, so this row can only be keyed on the id.
    let _ = send_chat("wanda-before".into(), "queued before we met".into());
    let thread = thread_for_device("wanda-before".into())
        .unwrap()
        .expect("the pre-session row was not written at all");

    let wanda = Arc::new(Mutex::new(Identity::generate("Wanda", 0x00_88_ff)));
    let before = session_peer(&h.air, "wanda-before", wanda.clone());
    until_session(&before);

    // Deliberately no send here: the session alone has to have moved the row.
    let after = session_peer(&h.air, "wanda-after", wanda.clone());
    until_session(&after);

    assert_eq!(
        thread_for_device("wanda-after".into()).unwrap(),
        Some(thread),
        "the conversation was not findable under the rotated id, so the row \
         was still keyed on the id it was written with"
    );
    assert_eq!(
        list_threads().unwrap().len(),
        1,
        "reconciling left more than one conversation for one person"
    );
}

// ── pairing (R0-F4) ─────────────────────────────────────────────────────────

/// A second party that runs the whole stack, so a ceremony has two real ends.
///
/// `advertising_peer` above is only a `Discovery`, which is enough to be seen
/// but not to pair: pairing needs a session, and a session needs the peer to
/// answer a handshake. This is the same shape `tests/net.rs` uses, borrowed
/// here because what is under test is the engine's *persistence*, and that
/// cannot be reached without a genuine ceremony reaching it.
struct FullPeer {
    net: rust_lib_hoppler::engine::net::Net,
    transport: Arc<dyn Transport>,
    rx: Receiver<TransportEvent>,
    identity: Arc<Mutex<Identity>>,
    id: String,
}

fn full_peer(air: &LoopbackNet, id: &str, name: &str) -> FullPeer {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    let identity = Arc::new(Mutex::new(Identity::generate(name, 0x00_44_88)));
    let net = rust_lib_hoppler::engine::net::Net::new(
        transport.clone(),
        identity.clone(),
        id,
        Instant::now(),
    );
    FullPeer {
        net,
        transport,
        rx,
        identity,
        id: id.to_string(),
    }
}

/// Drain whatever the peer's transport has to say. The engine pumps itself on
/// its own thread; this is the other side's turn.
fn pump(peer: &FullPeer) -> Vec<rust_lib_hoppler::engine::net::NetEvent> {
    let mut out = Vec::new();
    while let Ok(event) = peer.rx.recv_timeout(Duration::from_millis(30)) {
        out.extend(peer.net.handle(event, Instant::now()));
    }
    out
}

/// R0-F4 end to end through the Core API: a ceremony that both people confirm
/// leaves a contact with a real Layer-1 identity and a persistent thread.
#[test]
fn a_confirmed_ceremony_leaves_a_thread_behind() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");

    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();
    until("the engine to see the peer", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == peer.id)
    });
    assert!(
        list_threads().unwrap().is_empty(),
        "a thread existed already"
    );

    // The peer holds up a code; the engine reads it.
    let invite = Invite::fresh(
        peer.identity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .layer2_public(),
        &peer.id,
    );
    peer.net.show_invite(invite.clone());
    let device_id = begin_pairing(invite.to_uri()).unwrap();
    assert_eq!(device_id, peer.id);

    // Pump until the peer's screen actually has colours on it, which means
    // both do. Accumulated across pumps rather than checked per pump: the event
    // arrives exactly once, and a condition re-evaluated on each poll would
    // have to catch that single pump or wait forever.
    //
    // Written the lazy way first — "the peer has a ceremony and is showing a
    // code" — which is true from the moment the ceremony starts and would have
    // let the confirmation below run before the handshake had finished.
    let mut seen = Vec::new();
    until("both sides to reach the colours", || {
        seen.extend(pump(&peer));
        seen.iter().any(|e| {
            matches!(
                e,
                rust_lib_hoppler::engine::net::NetEvent::PairingSas { .. }
            )
        })
    });

    // Nothing is written on one confirmation.
    confirm_pairing(device_id.clone()).unwrap();
    for _ in 0..5 {
        pump(&peer);
    }
    assert!(
        list_threads().unwrap().is_empty(),
        "one confirmation created a thread"
    );

    // The second one completes it.
    peer.net.confirm_pairing("core", Instant::now()).unwrap();
    until("the pairing to be written down", || {
        pump(&peer);
        list_threads().unwrap().len() == 1
    });

    // And the device now reads as paired, which it could not before.
    until("the nearby list to show the pairing", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == peer.id && d.paired)
    });
}

/// A pairing outlives the session that made it.
///
/// The case that matters most and the one the first version got wrong: pair,
/// walk away, come back. There is no session until something dials, so a check
/// that asks the session reports a paired friend as not paired — precisely the
/// state pairing exists to survive.
#[test]
fn a_paired_friend_still_reads_as_paired_with_no_session() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");

    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();
    until("the engine to see the peer", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == peer.id)
    });

    let invite = Invite::fresh(
        peer.identity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .layer2_public(),
        &peer.id,
    );
    peer.net.show_invite(invite.clone());
    let device_id = begin_pairing(invite.to_uri()).unwrap();
    let mut seen = Vec::new();
    until("both sides to reach the colours", || {
        seen.extend(pump(&peer));
        seen.iter().any(|e| {
            matches!(
                e,
                rust_lib_hoppler::engine::net::NetEvent::PairingSas { .. }
            )
        })
    });
    confirm_pairing(device_id.clone()).unwrap();
    peer.net.confirm_pairing("core", Instant::now()).unwrap();
    until("the pairing to be written down", || {
        pump(&peer);
        list_threads().unwrap().len() == 1
    });

    // Now lose the session, as walking out of range would. Hanging up at the
    // peer's transport is what reaches the engine — closing the peer's own
    // session table would leave the engine's untouched, which is how this test
    // failed first: it asserted a state it had not actually produced.
    peer.transport.disconnect("core").unwrap();
    until("the engine to notice the session go", || {
        pump(&peer);
        !has_session(&device_id)
    });

    // The badge must survive it.
    assert!(
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == device_id && d.paired),
        "a paired friend read as unpaired once the session was gone"
    );
}

/// A code names a device. If that device is nowhere to be seen, say so rather
/// than starting a ceremony into the dark.
///
/// The *message* is what this asserts, not merely that it fails. Handing the
/// unresolved hint to the transport fails too — it cannot dial a peer it has
/// never heard of — so a bare `is_err()` passes with the check deleted, which
/// is exactly what mutation testing showed. What the check buys is a sentence
/// a person can act on instead of a dialling error.
#[test]
fn a_code_for_nobody_nearby_says_so() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();
    set_discovery(true).unwrap();

    let stranger = Identity::generate("Nobody", 0);
    let invite = Invite::fresh(stranger.layer2_public(), "not-here");
    let err = begin_pairing(invite.to_uri()).unwrap_err();
    assert!(err.contains("not nearby"), "unhelpful error: {err}");
}

/// Something that is not a Hoppler code at all — the common case, since a
/// camera decodes every QR in front of it.
#[test]
fn a_foreign_code_is_refused_before_anything_is_reached() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();
    set_discovery(true).unwrap();
    assert!(begin_pairing("https://example.com/".into()).is_err());
}

/// The fallback matches on *identity*, not on "someone is nearby".
///
/// This is the one that would be dangerous to get wrong: a code names a
/// particular Layer-2 key, and a fallback that returned any visible device
/// would start a ceremony with whoever happened to be in the room. The
/// ceremony's own check would catch it a moment later — the persona would not
/// match the code — but the failure would be baffling, and relying on a later
/// layer to catch a lookup that is simply wrong is not a design.
///
/// Here because mutation testing showed the identity comparison could be
/// replaced with `true` and every test still passed.
#[test]
fn a_code_does_not_resolve_to_whoever_happens_to_be_nearby() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");

    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();
    until("the engine to see the peer", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "peer")
    });
    ping("peer".into()).unwrap();
    until("the peer's persona to be known", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "peer" && !d.name.is_empty())
    });

    // Ada is right there with a persona the engine knows. The code is not hers.
    let someone_else = Identity::generate("Not Ada", 0);
    let invite = Invite::from_parts(someone_else.layer2_public(), "stale-id", [4u8; 32]);
    let err = begin_pairing(invite.to_uri()).unwrap_err();
    assert!(
        err.contains("not nearby"),
        "resolved to the wrong device: {err}"
    );
}

/// A code outlives the id printed inside it.
///
/// The hint rotates every twelve minutes (R0-F2), so a code that has been on
/// screen across a rotation carries an address that no longer resolves. Falling
/// back to the Layer-2 key Discovery already knows is what keeps that code
/// working — without it a perfectly good code fails after twelve minutes for a
/// reason nobody holding the phone could guess.
#[test]
fn a_code_whose_hint_has_gone_stale_still_finds_its_device() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");

    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();
    until("the engine to see the peer", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "peer")
    });

    // A Ping to force the reach, because the persona is fetched by whoever
    // dials and nothing has dialled yet. Without it there is no persona for the
    // fallback to match on, and the test would be asserting the hint path
    // again under a different name.
    ping("peer".into()).unwrap();
    until("the peer's persona to be known", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "peer" && !d.name.is_empty())
    });

    let peers_key = peer
        .identity
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .layer2_public();
    // The right identity, an address that has since rotated away.
    let invite = Invite::from_parts(peers_key, "an-id-from-twelve-minutes-ago", [3u8; 32]);
    peer.net.show_invite(invite.clone());

    assert_eq!(begin_pairing(invite.to_uri()).unwrap(), "peer");
}

/// Every code is new. A dismissed one must not be usable from a photograph,
/// which is only true if the nonce changes.
#[test]
fn every_code_shown_is_a_different_code() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let first = pairing_invite().unwrap();
    let second = pairing_invite().unwrap();
    assert_ne!(first, second, "two codes were identical");

    // And what comes out is something a scanner can actually read.
    let parsed = Invite::parse(&second).unwrap();
    assert_eq!(
        parsed.l2_pub,
        Invite::parse(&first).unwrap().l2_pub,
        "the two codes named different identities"
    );
    stop_showing_invite().unwrap();
}
