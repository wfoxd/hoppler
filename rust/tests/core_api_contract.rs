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

use rust_lib_hoppler::api::discovery::{
    block_device, block_thread, blocked_people, nearby_devices, set_discovery, unblock_person,
};
use rust_lib_hoppler::api::identity::{current_persona, update_persona};
use rust_lib_hoppler::api::messaging::{
    list_threads, ping, send_chat, send_chat_to_thread, thread_for_device, thread_messages,
};
use rust_lib_hoppler::api::pairing::{
    begin_pairing, confirm_pairing, pairing_invite, stop_showing_invite,
};
use rust_lib_hoppler::api::transfers::offer_drop;
use rust_lib_hoppler::api::types::{CoreEvent, MessageStateDto};
use rust_lib_hoppler::block::{Blocklist, Handle};
use rust_lib_hoppler::discovery::Discovery;
use rust_lib_hoppler::engine::fake::placeholder_pseudonym;
use rust_lib_hoppler::engine::{
    acked_on_receipt_for_test, drain_events_for_test, has_session, init_with_transport,
    layer1_public_for_test, mark_delivered_for_test, mark_sent_for_test, message_state_for_test,
    open_store_for_test, queued_for_resend_for_test, queued_on_thread_for_test,
    ratchet_can_send_for_test, ratchet_fingerprint_for_test, ratchet_size_for_test,
    receive_chat_for_test, record_events_for_test, refusal_for_test, resend_queued_for_test,
    stop_recording_for_test, thread_rows_for_test,
};
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::pairing::invite::Invite;
use rust_lib_hoppler::session::chat::{ChatEnvelope, MAX_AHEAD, MAX_BODY, MAX_UNACKED};
use rust_lib_hoppler::session::ratchet::{self, Header, Ratchet};
use rust_lib_hoppler::store::NewContact;
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
    dir: tempfile::TempDir,
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
    Harness { dir, air }
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
    let d = Discovery::new(
        transport,
        identity,
        Arc::new(Blocklist::default()),
        Instant::now(),
    );
    // What `Net::new` does for the real one. Without it the advert goes out
    // under an id the hint was not computed against, and nothing says so.
    d.set_local_id_for_tiebreak(id);
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

/// A second engine over the same directory — a restart, as far as the store is
/// concerned.
///
/// The rung id differs per boot because a loopback airspace holds one member
/// per id, and because a restarted device really does come back under a fresh
/// one (R0-F2).
fn boot(dir: &tempfile::TempDir, air: &LoopbackNet, id: &str) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    init_with_transport(dir.path().to_str().unwrap().to_string(), transport, id, rx).unwrap();
}

/// A second view of the same database, for a test to write facts the engine has
/// no API to write yet — the block *action* is T18b.
fn on_disk(dir: &tempfile::TempDir) -> rust_lib_hoppler::store::Store {
    open_store_for_test(dir.path().to_str().unwrap().to_string()).unwrap()
}

/// A block is a fact about a person, not a mood this run happens to be in.
///
/// The `blocklist` table has been in the schema since its first version and
/// nothing has ever read it back, so a block written before this slice was
/// forgotten at the next launch. What fixes that is one line in `install`,
/// which is exactly why it gets a test of its own.
#[test]
fn a_block_on_disk_is_in_force_at_the_next_launch() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    boot(&dir, &air, "core-first");

    // Somebody we paired with, so the row comes off the disk with no radio
    // involved and this test says nothing about discovery timing.
    let mallory = [7u8; 32];
    {
        let store = on_disk(&dir);
        let id = store
            .add_contact(&NewContact {
                pseudonym: mallory,
                l2_pub: [1u8; 32],
                name: "Mallory".into(),
                colour: 0x00_ff_00,
                persona_version: 1,
                first_seen: 0,
            })
            .unwrap();
        store
            .record_pairing(
                id, &[1u8; 32], "Mallory", 0x00_ff_00, 1, &[2u8; 32], &[3u8; 4], 0,
            )
            .unwrap();
    }

    // The control, and it has to come first: with nothing on the block list the
    // row is drawn. Without this the assertion below would pass just as well
    // for a row that was never going to appear.
    boot(&dir, &air, "core-second");
    assert!(
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.name == "Mallory"),
        "the paired row is not drawn at all, so hiding it proves nothing"
    );

    on_disk(&dir)
        .block(&[(mallory, Handle::Pseudonym)], None, 0)
        .unwrap();

    boot(&dir, &air, "core-third");
    assert!(
        !nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.name == "Mallory"),
        "a block written to disk was forgotten at the next launch: the \
         list the app enforces is not the list the user wrote"
    );
}

/// §12's fourth surface, on the other loop: a blocked device we can currently
/// see.
///
/// Absence is asserted against an ordering rather than a timeout. Both peers
/// join after the engine, blocked one first, and the pump processes transport
/// events on one thread in order — so by the time the *second* peer has a row,
/// the first one's advertisement has certainly been seen and refused.
#[test]
fn a_blocked_device_in_front_of_us_draws_no_row() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    boot(&dir, &air, "core-before");

    // No session has ever proved who this device is, so the row the engine
    // would draw is keyed on the placeholder derived from its rung id — and
    // that is what a block written against such a row would hold.
    let mallory = placeholder_pseudonym("mallory-device");
    {
        let store = on_disk(&dir);
        store
            .add_contact(&NewContact {
                pseudonym: mallory,
                l2_pub: [0u8; 32],
                name: "Mallory".into(),
                colour: 0x00_ff_00,
                persona_version: 1,
                first_seen: 0,
            })
            .unwrap();
        store.block(&[(mallory, Handle::Device)], None, 0).unwrap();
    }

    boot(&dir, &air, "core-after");
    let (_blocked, _rb) = advertising_peer(&air, "mallory-device");
    let (_friend, _rf) = advertising_peer(&air, "friend-device");
    set_discovery(true).unwrap();
    until("an unblocked peer to appear", || {
        nearby_devices()
            .map(|d| {
                d.iter()
                    .any(|d| d.device_id.as_deref() == Some("friend-device"))
            })
            .unwrap_or(false)
    });

    let listed: Vec<String> = nearby_devices()
        .unwrap()
        .iter()
        .filter_map(|d| d.device_id.clone())
        .collect();
    assert!(
        !listed.iter().any(|id| id == "mallory-device"),
        "a blocked device is still listed as nearby: {listed:?}"
    );
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
            .map(|d| d.iter().any(|d| d.device_id.as_deref() == Some("peer-one")))
            .unwrap_or(false)
    });
    let devices = nearby_devices().unwrap();
    assert!(
        devices
            .iter()
            .any(|d| d.device_id.as_deref() == Some("peer-one")),
        "an advertising peer was not seen; ids: {:?}",
        devices
            .iter()
            .map(|d| d.device_id.clone().unwrap_or_default())
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
            .map(|d| d.iter().any(|d| d.device_id.as_deref() == Some("nameless")))
            .unwrap_or(false)
    });
    let devices = nearby_devices().unwrap();
    let seen = devices
        .iter()
        .find(|d| d.device_id.as_deref() == Some("nameless"));
    assert!(
        seen.is_some(),
        "not listed at all; ids: {:?}",
        devices
            .iter()
            .map(|d| d.device_id.clone().unwrap_or_default())
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
            .map(|d| d.iter().any(|d| d.device_id.as_deref() == Some("peer-one")))
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
fn a_chat_to_someone_out_of_range_is_queued_rather_than_refused() {
    // R0-F5: "messages composed out of range are queued locally and delivered
    // at the next direct encounter". This answered `Err`, and a Pixel showed
    // the consequence — "Error: no session with …" over a message sitting
    // safely in the thread. The app contradicted the requirement to the one
    // person who could see both halves.
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();
    let dto = send_chat("unreachable".into(), "hi".into())
        .expect("a message to someone out of range was refused");
    assert_eq!(dto.text, "hi");

    let thread = thread_for_device("unreachable".into()).unwrap();
    assert!(thread.is_some(), "no thread was created for a failed send");
    let msgs = thread_messages(thread.unwrap()).unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].outgoing && msgs[0].text == "hi");
    // And left *queued*, which is the difference between this and a send that
    // got away, and what the next reunion acts on.
    let owed = queued_for_resend_for_test("unreachable").unwrap();
    assert_eq!(owed.len(), 1, "the message was not left for the reunion");
}

/// The defect this slice removes. A sender that never saw an ack resends the
/// same message, and the ratchet is right to accept it — cryptographically it
/// has never seen those bytes. Only the envelope's `msg_id` can say the person
/// already has it, and until this change the receiver invented its own, so the
/// resend became a second line on somebody's screen.
#[test]
fn the_same_message_arriving_twice_is_one_message() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let envelope = ChatEnvelope::new(1, b"are you there?".to_vec()).unwrap();
    let first = receive_chat_for_test("peer", &envelope.encode()).unwrap();
    assert!(first.is_some(), "the first arrival was not announced");

    let again = receive_chat_for_test("peer", &envelope.encode()).unwrap();
    assert!(again.is_none(), "a resend was announced as a new message");

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let msgs = thread_messages(thread).unwrap();
    assert_eq!(msgs.len(), 1, "the resend was stored as a second line");
    assert_eq!(msgs[0].text, "are you there?");
}

/// And the sender's numbering is what gets stored, not a count of what we
/// happen to have seen. A receiver that renumbered from 1 would agree with
/// itself and with nobody else — and every later resend, acknowledgement and
/// gap is matched on these two values.
#[test]
fn an_arriving_message_keeps_the_senders_identifiers() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let envelope = ChatEnvelope::new(7, b"seventh".to_vec()).unwrap();
    let event = receive_chat_for_test("peer", &envelope.encode())
        .unwrap()
        .unwrap();
    let CoreEvent::MessageReceived { msg_id, .. } = event else {
        panic!("expected a message event");
    };
    assert_eq!(
        msg_id,
        hex::encode(envelope.msg_id),
        "the receiver announced an id the sender never sent"
    );

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(
        thread_rows_for_test(thread).unwrap(),
        vec![(7, hex::encode(envelope.msg_id))],
        "the row was renumbered or re-identified on the way in"
    );
}

fn rows_ids(thread: i64) -> Vec<String> {
    thread_rows_for_test(thread)
        .unwrap()
        .into_iter()
        .map(|(_, id)| id)
        .collect()
}

/// R0-F5: "queued messages deliver on reunion with no user action". Nothing
/// did — a message written while the peer was away stayed `Queued` for ever,
/// because the only thing that ever sent one was the call that created it.
#[test]
fn a_reunion_sends_what_was_queued_while_the_peer_was_away() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    // No session, so each send writes its row and fails on the wire.
    for text in ["one", "two", "three"] {
        let _ = send_chat("peer".into(), text.to_string());
    }
    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(thread_rows_for_test(thread).unwrap().len(), 3);

    // What the reunion would put on the wire. Checked apart from the sending,
    // because the sending needs a transport and a peer and the *decision* is
    // where the rules are — every one of them survived a mutant while the test
    // only watched the store, which is true whether anything is sent or not.
    let owed = queued_for_resend_for_test("peer").unwrap();
    assert_eq!(
        owed.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "the backlog was renumbered or came out of order"
    );
    // Under the identifiers the rows already carry, or the far side sees three
    // new messages rather than three it may already have.
    assert_eq!(
        owed.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
        rows_ids(thread),
        "the resend drew fresh message ids"
    );

    let _ = resend_queued_for_test("peer");
    let rows = thread_rows_for_test(thread).unwrap();
    assert_eq!(rows.len(), 3, "the resend duplicated or dropped rows");
}

/// Only what never got away. Without an acknowledgement protocol a `Sent`
/// message cannot be told from a delivered one, and resending every message
/// ever sent on every reunion would be worse than the gap it covers.
#[test]
fn a_reunion_does_not_resend_what_already_went() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let _ = send_chat("peer".into(), "queued".into());
    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(queued_for_resend_for_test("peer").unwrap().len(), 1);

    mark_sent_for_test(thread).unwrap();
    assert!(
        queued_for_resend_for_test("peer").unwrap().is_empty(),
        "a message already away was queued up again"
    );
}

/// The queue is bounded. Somebody typing to a person who is not there fills it,
/// and the honest answer is to refuse: dropping the oldest would lose something
/// they wrote and tell them nothing, and holding everything makes the queue a
/// peer's absence can grow without limit.
#[test]
fn a_full_queue_refuses_rather_than_forgetting_what_was_typed() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    for i in 0..MAX_UNACKED {
        let _ = send_chat("peer".into(), format!("message {i}"));
    }
    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(thread_rows_for_test(thread).unwrap().len(), MAX_UNACKED);

    let refused = send_chat("peer".into(), "one too many".into());
    assert!(refused.is_err(), "the bound was not enforced");
    assert_eq!(
        thread_rows_for_test(thread).unwrap().len(),
        MAX_UNACKED,
        "the refused message was written anyway"
    );
}

/// The gap R0-F5 says must not be closed over silently. A peer that jumps
/// further ahead than the inbox will track is refused rather than accepted
/// with an unbounded hole behind it — the hole is memory a peer could ask for
/// by picking a number, and quietly forgetting it is the loss the requirement
/// exists to prevent.
#[test]
fn a_message_too_far_ahead_is_refused_rather_than_leaving_a_hole() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let first = ChatEnvelope::new(1, b"here".to_vec()).unwrap();
    receive_chat_for_test("peer", &first.encode())
        .unwrap()
        .unwrap();

    let far = ChatEnvelope::new(1 + MAX_AHEAD + 1, b"much later".to_vec()).unwrap();
    assert!(
        receive_chat_for_test("peer", &far.encode())
            .unwrap()
            .is_none(),
        "a message beyond the window was accepted"
    );

    // Just inside is fine, so what is refused is the bound and not the shape.
    let edge = ChatEnvelope::new(1 + MAX_AHEAD, b"at the edge".to_vec()).unwrap();
    assert!(receive_chat_for_test("peer", &edge.encode())
        .unwrap()
        .is_some());

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let rows = thread_rows_for_test(thread).unwrap();
    assert_eq!(rows.len(), 2, "the refused message was stored: {rows:?}");
}

/// Out-of-order arrival is held, and the run closes when the missing one turns
/// up — which is what a reunion looks like when a queued backlog lands.
#[test]
fn a_gap_closes_when_the_missing_message_arrives() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    for seq in [3u64, 1, 2] {
        let e = ChatEnvelope::new(seq, format!("number {seq}").into_bytes()).unwrap();
        assert!(
            receive_chat_for_test("peer", &e.encode())
                .unwrap()
                .is_some(),
            "message {seq} was refused"
        );
    }

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let rows = thread_rows_for_test(thread).unwrap();
    assert_eq!(rows.len(), 3);

    // And now that the run is contiguous, every one of them is a duplicate.
    for seq in 1..=3u64 {
        let e = ChatEnvelope::new(seq, format!("number {seq}").into_bytes()).unwrap();
        assert!(
            receive_chat_for_test("peer", &e.encode())
                .unwrap()
                .is_none(),
            "message {seq} was accepted twice"
        );
    }
}

/// The inbox dedups on `seq`, the store on `msg_id`, and the code claims
/// neither covers the other's case. This is that claim as a test: a *new*
/// `seq` carrying an id we already hold gets past the inbox and has to be
/// stopped by the store. Without the second check it would be announced as a
/// new message and then quietly not stored — on screen, and gone on the next
/// launch.
#[test]
fn an_id_we_already_hold_is_refused_even_under_a_new_seq() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let first = ChatEnvelope::new(1, b"once".to_vec()).unwrap();
    receive_chat_for_test("peer", &first.encode())
        .unwrap()
        .unwrap();

    // Same id, next number: past the inbox, into the store's UNIQUE.
    let recycled = ChatEnvelope {
        seq: 2,
        msg_id: first.msg_id,
        body: b"again".to_vec(),
    };
    assert!(
        receive_chat_for_test("peer", &recycled.encode())
            .unwrap()
            .is_none(),
        "a message id we already hold was announced under a new seq"
    );

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(thread_rows_for_test(thread).unwrap().len(), 1);
}

/// `seq` is chosen by whoever is sending, and `decode` takes any `u64`. Cast
/// into the store's `i64`, anything above `i64::MAX` lands *negative* — sorting
/// before every real message — so one frame from a stranger could reorder a
/// conversation permanently. It is refused instead, and refused quietly: an
/// unusable message is not a reason to tear down a session.
#[test]
fn a_seq_too_large_for_the_store_is_refused_rather_than_wrapped() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let good = ChatEnvelope::new(1, b"first".to_vec()).unwrap();
    receive_chat_for_test("peer", &good.encode())
        .unwrap()
        .unwrap();

    let huge = ChatEnvelope::new(u64::MAX, b"from the future".to_vec()).unwrap();
    assert!(
        receive_chat_for_test("peer", &huge.encode())
            .unwrap()
            .is_none(),
        "a seq that cannot be stored was announced anyway"
    );

    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let rows = thread_rows_for_test(thread).unwrap();
    assert_eq!(rows.len(), 1, "the unstorable message was written anyway");
    assert!(
        rows.iter().all(|(seq, _)| *seq > 0),
        "a negative seq reached the store: {rows:?}"
    );
}

/// The id the caller is handed has to be the id in the store, because that is
/// what an acknowledgement or a state change will look the row up by. Two
/// separately-drawn ids would agree on nothing and fail silently: the message
/// would send, and its delivery state would never move again.
#[test]
fn an_outgoing_message_is_stored_under_the_id_its_sender_was_given() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    // The wire send fails — there is no session — but the row is written
    // first, which is the part under test.
    let _ = send_chat("peer".into(), "hello".into());
    let thread = thread_for_device("peer".into()).unwrap().unwrap();
    let rows = thread_rows_for_test(thread).unwrap();
    let shown = thread_messages(thread).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1, shown[0].msg_id,
        "the caller was told one id and the store kept another"
    );
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

/// A transport that can be told to stop accepting bytes, with the pipes it has
/// already opened left alone.
///
/// Refusing from the start would only fail the handshake, and the handshake is
/// not what is under test — a session that exists and then will not carry a
/// frame is.
struct RefusesSends {
    inner: Arc<dyn Transport>,
    refusing: Arc<std::sync::atomic::AtomicBool>,
}

impl Transport for RefusesSends {
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        if self.refusing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TransportError::Io("the radio would not take it".into()));
        }
        self.inner.send(peer, bytes)
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
    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        self.inner.connect(peer)
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

/// The other half of "out of range is not an error".
///
/// Holding a message quietly is right for a peer we have no session with, and
/// wrong for every other refusal — those happen *with a session open*, so no
/// reunion is coming to retry them: nothing reopens a session that never
/// closed. Reported as sent, the row would wait forever beside a recipient who
/// is standing right there.
///
/// The two cases were one `String` before this, and the arm that held the first
/// held the second with it. This is the test that tells them apart.
#[test]
fn a_send_the_transport_refuses_is_reported_rather_than_held() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let refusing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let transport: Arc<dyn Transport> = Arc::new(RefusesSends {
        inner: Arc::new(air.join("core", sink)),
        refusing: Arc::clone(&refusing),
    });
    init_with_transport(
        dir.path().to_str().unwrap().to_string(),
        transport,
        "core",
        rx,
    )
    .unwrap();
    set_discovery(true).unwrap();

    let identity = Arc::new(Mutex::new(Identity::generate("Wanda", 0x00_88_ff)));
    let peer = session_peer(&air, "wanda", identity);
    until_session(&peer);

    refusing.store(true, std::sync::atomic::Ordering::SeqCst);
    let why = match send_chat("wanda".into(), "are you there?".into()) {
        Ok(_) => panic!("a refused send was reported as success"),
        Err(why) => why,
    };
    assert!(
        why.contains("could not send"),
        "the person was not told the send failed: {why}"
    );

    // Still on the queue, because the row is the only copy of what they typed.
    // Reported *and* kept is the whole answer; either alone is a bug.
    let owed = queued_for_resend_for_test("wanda").unwrap();
    assert_eq!(owed.len(), 1, "a refused message was dropped");
}

/// R0-F5 end to end, with nobody tapping anything.
#[test]
fn a_queued_message_goes_out_when_she_appears() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    set_discovery(true).unwrap();

    send_chat("wanda".into(), "hey".into()).expect("queueing refused");
    assert_eq!(queued_for_resend_for_test("wanda").unwrap().len(), 1);

    let identity = Arc::new(Mutex::new(Identity::generate("Wanda", 0x00_88_ff)));
    let peer = session_peer(&h.air, "wanda", identity);
    // Deliberately not `until_session`: that dials from her side, and the whole
    // question is whether *this* device goes and asks who has just appeared.
    until("the queued message to go out on its own", || {
        let now = Instant::now();
        while let Ok(e) = peer.rx.recv_timeout(Duration::from_millis(20)) {
            peer.net.handle(e, now);
        }
        queued_for_resend_for_test("wanda")
            .map(|q| q.is_empty())
            .unwrap_or(false)
    });
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
    let net = rust_lib_hoppler::engine::net::Net::new(
        transport.clone(),
        identity,
        Arc::new(Blocklist::default()),
        id,
        Instant::now(),
    );
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
    /// This end's own ratchet, once a ceremony has seeded one.
    ///
    /// The engine keeps its half in the store; the harness has no store, so it
    /// keeps its half here. Without it the peer is a device that pairs and then
    /// never opens anything it is sent, which is not a second end of a
    /// conversation — it is an echo the engine cannot get wrong.
    ratchet: Mutex<Option<Ratchet>>,
    /// Whether this end answers what it opens.
    ///
    /// A real device always acknowledges. This exists so a test can hold the
    /// one state the engine cannot reach on its own — a message that genuinely
    /// went out, genuinely arrived, and was never answered — which is what a
    /// refused message looks like from the sender's side.
    silent: Mutex<bool>,
    /// Every body this end has opened, in order.
    ///
    /// What makes "the engine sent it" and "the other device could read it"
    /// two different assertions. They come apart exactly where this slice is
    /// most dangerous: a sender whose advanced ratchet never reached the store
    /// rewinds its message counter on the next send, which reuses a key and a
    /// nonce — and the only place that shows is here, as a body that will not
    /// open.
    heard: Mutex<Vec<Vec<u8>>>,
}

impl FullPeer {
    /// Play this end's half of the opening, the way the engine plays its own.
    ///
    /// Which side of a pairing gets the ratchet's initiator role is decided by
    /// comparing two freshly generated Layer-1 keys, so it lands here half the
    /// time — and when it does, the engine is the responder and cannot write
    /// until this peer speaks. Without this the harness is only ever half a
    /// conversation, and every test that sends on a paired thread passes on a
    /// coin flip.
    ///
    /// Deliberately the same three steps as `engine::opening_for_thread` rather
    /// than a call into it: this is the *other device*, and a test that reached
    /// into the engine to produce what it is supposed to be receiving would
    /// prove only that one function agrees with itself.
    fn open_the_chain(&self) {
        let owed = self
            .ratchet
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|r| r.owes_an_opening());
        if owed {
            self.send_opening();
        }
    }

    /// Seal an empty body and send it, the way an initiator does.
    fn send_opening(&self) {
        let mut held = self.ratchet.lock().unwrap_or_else(|p| p.into_inner());
        let ratchet = held.as_mut().expect("a ceremony has run");
        let (header, sealed) = ratchet.encrypt(&[]).expect("a chain that can send");
        let mut body = header.to_bytes().to_vec();
        body.extend_from_slice(&sealed);
        self.net
            .send_opening("core", body, Instant::now())
            .expect("a session that just carried a whole ceremony");
    }

    /// A whole chat frame, the way this device would send a *new* message: a
    /// fresh envelope carrying the text, encoded, and sealed entire.
    ///
    /// Each call draws a new `msg_id`, as `ChatEnvelope::new` does on the send
    /// path. A resend is not this — see [`FullPeer::seal_envelope`], which is
    /// what `resend_queued` does and keeps both identifiers.
    fn seal_chat(&self, seq: u64, text: &[u8]) -> Vec<u8> {
        let envelope = ChatEnvelope::new(seq, text.to_vec()).expect("a sendable message");
        self.seal_envelope(&envelope)
    }

    /// Seal an envelope that already exists — a resend.
    ///
    /// The distinction is the whole reason a resend is hard: it carries the
    /// `seq` *and* the `msg_id` it was first sent under, so the far side can
    /// tell it from a new message, and it is freshly sealed, so the ratchet has
    /// never seen these bytes. A helper that drew a new `msg_id` would be
    /// simulating a second message rather than the same one twice.
    fn seal_envelope(&self, envelope: &ChatEnvelope) -> Vec<u8> {
        self.seal(&envelope.encode())
    }

    /// Seal a body the way this device would send it.
    ///
    /// What the engine has to open. A test that handed it plaintext on a paired
    /// thread would be testing a message no real peer can send.
    fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut held = self.ratchet.lock().unwrap_or_else(|p| p.into_inner());
        let ratchet = held.as_mut().expect("a ceremony has run");
        let (header, sealed) = ratchet.encrypt(plaintext).expect("a chain that can send");
        let mut body = header.to_bytes().to_vec();
        body.extend_from_slice(&sealed);
        body
    }

    /// Forget this end's ratchet, so a second ceremony can seed a new one.
    ///
    /// Harness bookkeeping, not a device behaviour: `absorb` claims the
    /// ceremony's ratchet only when it is holding none, so a peer that pairs
    /// twice would otherwise keep chaining off the first root while the engine
    /// moved to the second.
    fn forget_ratchet(&self) {
        *self.ratchet.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.heard.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Whether a ceremony has left this end a ratchet at all.
    ///
    /// Distinct from [`FullPeer::can_send`], which is false both before the
    /// ceremony finishes and after it finishes on the responding side — two
    /// states that need telling apart, because only one of them is worth
    /// waiting for.
    fn has_ratchet(&self) -> bool {
        self.ratchet
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Whether this end could write on the thread right now.
    fn can_send(&self) -> bool {
        self.ratchet
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|r| r.can_send())
    }

    /// Open whatever the engine just sent, so this end keeps up with its own
    /// conversation.
    ///
    /// A body that will not open is ignored rather than fatal: most tests here
    /// chat over *unpaired* threads, where nothing is ratcheted and the bytes
    /// are the message.
    fn absorb(&self, events: &[rust_lib_hoppler::engine::net::NetEvent]) {
        use rust_lib_hoppler::engine::net::NetEvent;
        let mut held = self.ratchet.lock().unwrap_or_else(|p| p.into_inner());
        // Claimed here rather than after the pump, because the engine's opening
        // can arrive in the same batch of bytes as the ceremony that seeded
        // this: `Net` hands both back from one `handle`. Claiming it later
        // meant a peer that dropped the very message it was waiting for.
        if held.is_none() {
            if let Some(state) = self.net.take_pairing_ratchet("core") {
                *held = Some(Ratchet::from_state(&state).expect("the ceremony's ratchet"));
            }
        }
        let Some(ratchet) = held.as_mut() else { return };
        for event in events {
            // A chat frame is a sealed *envelope* now, so what comes out of the
            // ratchet still has to be decoded before it is anything a person
            // wrote. An opening carries no envelope at all, which is the whole
            // difference between the two kinds.
            let (body, is_chat) = match event {
                NetEvent::ChainOpening { body, .. } => (body, false),
                NetEvent::ChatReceived { body, .. } => (body, true),
                _ => continue,
            };
            let Some(header) = body
                .get(..ratchet::HEADER_LEN)
                .and_then(|h| <[u8; ratchet::HEADER_LEN]>::try_from(h).ok())
            else {
                continue;
            };
            let Ok(plaintext) =
                ratchet.decrypt(Header::from_bytes(&header), &body[ratchet::HEADER_LEN..])
            else {
                continue;
            };
            // Decoded once. It was decoded twice — for the text and again for
            // the `msg_id` to acknowledge — which is two readings of one thing,
            // free to drift apart.
            let decoded = if is_chat {
                match ChatEnvelope::decode(&plaintext) {
                    Ok(envelope) => Some(envelope),
                    Err(_) => continue,
                }
            } else {
                None
            };
            let heard = match &decoded {
                Some(envelope) => envelope.body.clone(),
                None => plaintext.to_vec(),
            };
            // Acknowledge it, the way the engine does when it is the one
            // receiving: seal the `msg_id` and send it back. Without this the
            // harness is a device that reads everything and confirms nothing,
            // so the sender's row could never leave `Sent` and the whole of
            // T14a would be untestable from this end.
            if let Some(envelope) = decoded {
                if !*self.silent.lock().unwrap_or_else(|p| p.into_inner()) {
                    if let Ok((header, sealed)) = ratchet.encrypt(&envelope.msg_id) {
                        let mut body = header.to_bytes().to_vec();
                        body.extend_from_slice(&sealed);
                        let _ = self.net.send_ack("core", body, Instant::now());
                    }
                }
            }
            self.heard
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(heard);
        }
    }

    /// Stop answering. What a device that refuses a message looks like from
    /// the other end: it received something and said nothing back.
    fn go_silent(&self) {
        *self.silent.lock().unwrap_or_else(|p| p.into_inner()) = true;
    }

    /// Everything this end has managed to open, as text.
    fn heard(&self) -> Vec<String> {
        self.heard
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect()
    }
}

fn full_peer(air: &LoopbackNet, id: &str, name: &str) -> FullPeer {
    full_peer_with(air, id, Identity::generate(name, 0x00_44_88))
}

/// A peer whose identity puts the *engine* on the named side of the ratchet.
///
/// `speaks_first` settles the two roles by comparing Layer-1 keys, smaller half
/// first, and both are generated fresh — so a test that takes whichever role
/// falls out exercises one of the two arrangements per run and cannot say which.
/// The arrangement is the entire subject of this slice: one side can write the
/// moment it pairs and the other cannot write until it is written to.
///
/// Rejection sampling, which is a coin flip per attempt and needs no cleverness
/// about where in the keyspace this device's key happens to sit.
fn full_peer_facing(air: &LoopbackNet, id: &str, name: &str, engine_initiates: bool) -> FullPeer {
    let ours = layer1_public_for_test().expect("an engine to compare against");
    loop {
        let identity = Identity::generate(name, 0x00_44_88);
        if (ours < identity.layer1_public().0) == engine_initiates {
            return full_peer_with(air, id, identity);
        }
    }
}

fn full_peer_with(air: &LoopbackNet, id: &str, identity: Identity) -> FullPeer {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    let identity = Arc::new(Mutex::new(identity));
    let net = rust_lib_hoppler::engine::net::Net::new(
        transport.clone(),
        identity.clone(),
        Arc::new(Blocklist::default()),
        id,
        Instant::now(),
    );
    FullPeer {
        net,
        transport,
        rx,
        identity,
        id: id.to_string(),
        ratchet: Mutex::new(None),
        silent: Mutex::new(false),
        heard: Mutex::new(Vec::new()),
    }
}

/// Drain whatever the peer's transport has to say. The engine pumps itself on
/// its own thread; this is the other side's turn.
/// Run a whole ceremony against `peer` and return the device id it paired.
///
/// The steps in order, with no assertions in between — a test that wants to
/// watch the middle of a ceremony should not use this, and one that only needs
/// two people paired should not carry fifty lines to say so.
///
/// Stops the moment the pairing is written down, which is *before* either end
/// can necessarily write: one of the two has no sending chain until the other
/// opens it. [`pair_with`] is the one to reach for unless that gap is the point.
fn pair_only(peer: &FullPeer) -> String {
    let invite = Invite::fresh(
        peer.identity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .layer2_public(),
        &peer.id,
    );
    peer.net.show_invite(invite.clone());
    let device_id = begin_pairing(invite.to_uri()).unwrap();
    // Accumulated across pumps: the event arrives exactly once, so a condition
    // re-evaluated per poll would have to catch that single pump or wait
    // forever.
    let mut seen = Vec::new();
    until("both sides to reach the colours", || {
        seen.extend(pump(peer));
        seen.iter().any(|e| {
            matches!(
                e,
                rust_lib_hoppler::engine::net::NetEvent::PairingSas { .. }
            )
        })
    });
    confirm_pairing(device_id.clone()).unwrap();
    peer.net.confirm_pairing("core", Instant::now()).unwrap();
    // Waited for by the *ratchet*, not by a count of threads.
    //
    // Counting was wrong in both directions. `len() == 1` sits through a second
    // ceremony for ever, because pairing again opens another conversation. And
    // "one more than before" hangs on a first pairing that adopts a thread that
    // was already there — which is what happens whenever two people chatted
    // before they paired, and is a decision T12 made on purpose.
    //
    // A thread gains a ratchet exactly when `record_pairing` writes one, so
    // this is the pairing landing rather than a proxy for it, and it holds
    // whichever of the two shapes the ceremony took.
    until("the pairing to be written down", || {
        pump(peer);
        thread_for_device(device_id.clone())
            .ok()
            .flatten()
            .and_then(|t| ratchet_can_send_for_test(t).ok().flatten())
            .is_some()
    });
    device_id
}

/// See the peer, pair with it, and come back with the device id.
///
/// The preamble every test that needs two paired devices was carrying verbatim.
fn meet_and_pair(peer: &FullPeer) -> String {
    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();
    until("the engine to see the peer", || {
        pump(peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    pair_with(peer)
}

/// Pair, and then wait until both devices can actually write.
///
/// The two are separate because the gap between them is a real state with real
/// behaviour in it — see `a_message_written_before_the_chain_opens_is_held`,
/// which is the only test that wants to stop in the middle. Everything else
/// wants two people who can talk to each other.
fn pair_with(peer: &FullPeer) -> String {
    let device_id = pair_only(peer);
    // `pair_only` waits for the *engine's* row, and the two ceremonies finish
    // independently: the engine pairs on the peer's Layer-1 proof and the peer
    // pairs on the engine's, so the engine's row can exist while this end has
    // not finished. Offering before that finds no ratchet and does nothing —
    // and in the arrangement where the peer is the one that owes an opening,
    // nothing would ever offer again and the wait below would never end.
    until("the peer's own ceremony to finish", || {
        pump(peer);
        peer.has_ratchet()
    });
    // Whichever of the two ends can speak, speaks — the engine does its half
    // from the `PairingCompleted` arm, this is the peer's. Exactly one of them
    // has anything to send, and the thread is not two-way until it lands.
    peer.open_the_chain();
    // The thread this ceremony opened, asked for by device rather than taken as
    // the only one: a device that has paired twice has two.
    let thread = thread_for_device(device_id.clone())
        .unwrap()
        .expect("a pairing with no thread");
    until("both ends of the ratchet to open", || {
        pump(peer);
        ratchet_can_send_for_test(thread).unwrap() == Some(true) && peer.can_send()
    });
    device_id
}

fn pump(peer: &FullPeer) -> Vec<rust_lib_hoppler::engine::net::NetEvent> {
    let mut out = Vec::new();
    while let Ok(event) = peer.rx.recv_timeout(Duration::from_millis(30)) {
        out.extend(peer.net.handle(event, Instant::now()));
    }
    peer.absorb(&out);
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()) && d.paired)
    });
}

/// R0-F10 end to end: blocking a paired person revokes the pairing, tears down
/// the live session, and takes the row off the list.
///
/// Every one of those is a separate way for a block to be nominal. A pairing
/// left in place keeps this device sealing to somebody it has decided not to
/// hear from; a session left open keeps delivering frames; a row left on screen
/// invites the next tap to start it all again.
#[test]
fn blocking_a_paired_person_revokes_tears_down_and_delists() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Mallory");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id.clone()).unwrap().unwrap();

    // The controls, all before the block, so each assertion below has something
    // it is known to be changing.
    assert!(has_session(&device_id), "no session to tear down");
    assert!(
        ratchet_size_for_test(thread).unwrap().is_some(),
        "no ratchet to revoke"
    );
    assert!(
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(device_id.as_str())),
        "no row to remove"
    );

    block_device(device_id.clone()).unwrap();

    assert!(
        !has_session(&device_id),
        "the session survived the block: frames from a blocked device still arrive"
    );
    assert!(
        ratchet_size_for_test(thread).unwrap().is_none(),
        "the ratchet survived the block, so this device can still seal to them"
    );
    let listed: Vec<String> = nearby_devices()
        .unwrap()
        .iter()
        .filter_map(|d| d.device_id.clone())
        .collect();
    assert!(
        !listed.iter().any(|id| id == &device_id),
        "a blocked person is still on the nearby list: {listed:?}"
    );
    // And not as a paired-but-absent row either, which is drawn off the disk by
    // a different loop and would be a second way for them to reappear.
    assert!(
        nearby_devices()
            .unwrap()
            .iter()
            .all(|d| d.name != "Mallory"),
        "the blocked person came back as a remembered pairing"
    );
}

/// Blocking somebody who is **not in the room**.
///
/// The ordinary case, not an edge one: R0-F4 makes pairing durable, so a paired
/// friend has a row and a thread whether the radio can see them or not — and
/// their `device_id` is `None` exactly then. `block_device` cannot be called at
/// all, which is why `block_thread` exists.
#[test]
fn a_paired_person_who_has_walked_away_can_still_be_blocked() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Mallory");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id.clone()).unwrap().unwrap();

    // They leave. Discovery off is the bluntest way to make the sighting go,
    // and it leaves the paired row drawn off the disk with no handle — which is
    // the state under test.
    set_discovery(false).unwrap();
    let away = nearby_devices().unwrap();
    let row = away
        .iter()
        .find(|d| d.name == "Mallory")
        .expect("a paired person must still have a row when out of range");
    assert!(
        row.device_id.is_none(),
        "this test needs them out of range, with no handle to block by"
    );

    block_thread(thread).unwrap();

    assert!(
        nearby_devices()
            .unwrap()
            .iter()
            .all(|d| d.name != "Mallory"),
        "a blocked person is still drawn as a remembered pairing"
    );
    let blocks = on_disk(&h.dir).list_blocks().unwrap();
    assert!(!blocks.is_empty(), "no block was written");
    assert!(
        blocks.iter().all(|b| b.revoked_pairing),
        "the pairing was not revoked"
    );
}

/// Unblocking has to reach the **live** list, not only the table.
///
/// A store-only unblock leaves every ingress refusing until the next launch,
/// which is a person told they unblocked somebody who is still gone. Asserted
/// through the nearby list, which consults the in-memory gate.
#[test]
fn unblocking_takes_effect_without_a_restart() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Mallory");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id.clone()).unwrap().unwrap();
    let contact = on_disk(&h.dir).contact_for_thread(thread).unwrap().unwrap();

    block_thread(thread).unwrap();
    assert!(
        blocked_people()
            .unwrap()
            .iter()
            .any(|p| p.name == "Mallory"),
        "the blocked list does not show who was just blocked"
    );
    // One row per *person*. A block is several handle rows underneath — that is
    // how T18b covers both dialling roles — so a list built straight off the
    // table would show the same person two or three times, with nothing on
    // screen to explain why.
    assert!(
        on_disk(&h.dir).list_blocks().unwrap().len() > 1,
        "this person was blocked on one handle, so the dedupe below proves nothing"
    );
    assert_eq!(
        blocked_people().unwrap().len(),
        1,
        "one person appears more than once on the blocked list"
    );

    unblock_person(contact).unwrap();

    assert!(
        blocked_people().unwrap().is_empty(),
        "the block survived being lifted"
    );
    assert!(
        on_disk(&h.dir).list_blocks().unwrap().is_empty(),
        "the rows survived being lifted"
    );
    // The live gate, not the table: this row is drawn only if `shut_out` lets
    // it through, and nothing has restarted.
    until("the unblocked person to come back", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.name == "Mallory")
    });

    // And what unblocking does *not* restore (R0-F10).
    assert!(
        on_disk(&h.dir)
            .pairing_for_contact(contact)
            .unwrap()
            .is_none(),
        "unblocking brought the revoked pairing back"
    );
}

/// T18e: a block bound to a Layer-2 persona key learns the durable pseudonym
/// the first time the blocked device dials — and is still refused.
///
/// This is R0-F10's "a freshly generated Layer-2 persona does not evade it",
/// for the half of blocks that cannot hold a pseudonym when they are written.
/// The peer's rung id sorts before the engine's, so the *peer* dials, which is
/// the only arrangement in which a pseudonym is ever proved to us.
#[test]
fn a_refused_dial_makes_a_weak_block_durable() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let air = LoopbackNet::new();
    let peer = full_peer(&air, "aaa-peer", "Mallory");

    // A block holding only their persona key — what `block_device` records for
    // somebody this device has only ever dialled. Written straight to the store
    // so the state is exactly that and not accidentally stronger.
    let their_l2 = peer
        .identity
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .layer2_public()
        .0;
    let contact = {
        let store = on_disk(&dir);
        let id = store
            .add_contact(&NewContact {
                pseudonym: [9u8; 32],
                l2_pub: their_l2,
                name: "Mallory".into(),
                colour: 1,
                persona_version: 1,
                first_seen: 0,
            })
            .unwrap();
        store
            .block(&[(their_l2, Handle::PersonaKey)], Some(id), 0)
            .unwrap();
        id
    };

    // Booted *after* the block is on disk, so the live list holds it the way a
    // launch loads it — and so the engine's rung id sorts after the peer's,
    // which is what makes the peer the one that dials.
    boot(&dir, &air, "mmm-core");
    set_discovery(true).unwrap();
    peer.net
        .discovery()
        .set_enabled(true, Instant::now())
        .unwrap();
    peer.net.discovery().start_scanning().unwrap();

    // They dial. Their offer proves a pseudonym before we have said anything.
    peer.net.reach("mmm-core").unwrap();
    until("the block to learn the durable handle", || {
        pump(&peer);
        on_disk(&dir)
            .list_blocks()
            .unwrap()
            .iter()
            .any(|b| b.kind == Handle::Pseudonym)
    });

    let learned = on_disk(&dir)
        .list_blocks()
        .unwrap()
        .into_iter()
        .find(|b| b.kind == Handle::Pseudonym)
        .expect("checked above");
    assert_eq!(
        learned.contact,
        Some(contact),
        "the learned pseudonym was filed under the wrong person"
    );
    assert_ne!(
        learned.handle, their_l2,
        "what was recorded is the persona key again, not a pseudonym"
    );

    // And the refusal was a refusal: no session, either side.
    assert!(
        !has_session("aaa-peer"),
        "a blocked device got a session out of the dial that taught us"
    );
}

/// A device this app has never seen cannot be blocked, because there is
/// nothing to write that would mean anything.
///
/// Storing a hash of a rung id we have never seen would look like a block and
/// be one for nobody: the id is somebody else's within twelve minutes, and the
/// row would then be a block on whoever inherits it.
#[test]
fn a_device_we_know_nothing_about_cannot_be_blocked() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();

    assert!(
        block_device("never-seen-this-one".into()).is_err(),
        "blocked a device the app has no record of"
    );
    assert!(
        on_disk(&h.dir).list_blocks().unwrap().is_empty(),
        "a block row was written for a device we have never seen"
    );

    // The control: a device we *have* seen blocks fine, so the refusal above is
    // about what is known and not about blocking being broken.
    let peer = full_peer(&h.air, "peer", "Mallory");
    let device_id = meet_and_pair(&peer);
    block_device(device_id).unwrap();
    assert!(!on_disk(&h.dir).list_blocks().unwrap().is_empty());
}

/// What a block binds to depends on who dialled, so both arrangements are
/// tested — and the weaker one must not be recorded as though it were durable.
///
/// `Net::we_initiate` compares the two rung ids, and the engine's is `core`. So
/// naming the peer either side of that decides the roles outright, which is the
/// only reason this can be asserted at all rather than depending on a coin flip.
#[test]
fn a_block_records_a_pseudonym_only_when_the_peer_dialled_us() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (peer_id, dialled_us, expected) in [
        ("aaa-peer", true, Handle::Pseudonym),
        ("zzz-peer", false, Handle::PersonaKey),
    ] {
        let h = fresh();
        let peer = full_peer(&h.air, peer_id, "Mallory");
        assert_eq!(
            peer_id < "core",
            dialled_us,
            "this test's premise about who dials is wrong"
        );
        let device_id = meet_and_pair(&peer);

        block_device(device_id).unwrap();

        let blocks = on_disk(&h.dir).list_blocks().unwrap();
        let strongest = blocks.iter().map(|b| b.kind).max().unwrap();
        assert_eq!(
            strongest, expected,
            "{peer_id}: wrong strongest handle recorded. A session's remote \
             static is the peer's pseudonym only when they dialled us; when we \
             dialled it is their Layer-2 key, and recording that as a pseudonym \
             credits the block with a durability it has not got"
        );
        // Whichever way round, the Layer-2 key is on the list too — that is
        // what a session we opened can offer the gate, and without it the block
        // is invisible to exactly half of them.
        assert!(
            blocks.iter().any(|b| b.kind == Handle::PersonaKey),
            "{peer_id}: no persona key recorded"
        );
        assert!(
            blocks.iter().all(|b| b.revoked_pairing),
            "{peer_id}: the pairing was not revoked"
        );
    }
}

/// Pairing leaves a ratchet behind.
///
/// T12 calls ratchet persistence the correctness heart of Chat, and until now
/// `Ratchet::initiator` and `Ratchet::responder` had no caller outside their own
/// tests: the state machine, the `ratchets` table and `start_ratchet` all
/// existed with nothing joining them. A paired thread with no ratchet cannot
/// ever hold a ratcheted message, and the only way back is pairing again —
/// which costs two people and a code.
///
/// The size, never the state. A test helper that handed back key material would
/// put it one assertion away from a CI log.
#[test]
fn pairing_leaves_a_ratchet_on_the_thread() {
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    pair_with(&peer);

    let thread = list_threads().unwrap()[0].thread_id;
    let size = ratchet_size_for_test(thread).unwrap();
    assert!(
        size.is_some_and(|n| n > 0),
        "a paired thread has no ratchet: {size:?}"
    );
}

/// Writing on a paired thread turns its ratchet.
///
/// The slice that made the ratchet do something. Until now it was seeded,
/// stored and never touched — every test passed because unpaired threads have
/// no ratchet, so nothing about them changed.
///
/// The fingerprint moving is what says a message key was drawn and the chain
/// stepped. It is also the property the nonce depends on: `nonce_for` derives
/// from the message number, so a counter that failed to advance — or failed to
/// persist and rewound — would reuse a key *and* a nonce on different
/// plaintext, which is an AEAD break rather than a degraded ratchet.
#[test]
fn writing_on_a_paired_thread_turns_the_ratchet() {
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    let device_id = pair_with(&peer);

    let thread = list_threads().unwrap()[0].thread_id;
    let before = ratchet_fingerprint_for_test(thread)
        .unwrap()
        .expect("a paired thread with no ratchet");

    send_chat(device_id, "are you there?".into()).unwrap();

    let after = ratchet_fingerprint_for_test(thread).unwrap().unwrap();
    assert!(
        before != after,
        "the ratchet did not move when a message was sealed"
    );

    // And the row still holds what the person typed, not what went on the wire.
    // The screen reads this, and the store is already encrypted at rest.
    let msgs = thread_messages(thread).unwrap();
    assert_eq!(msgs.last().unwrap().text, "are you there?");
}

/// An opening turns the ratchet and leaves nothing behind it.
///
/// The cost of the design, made into a test rather than a comment. An opening
/// is a message that must not be stored and must not be shown, and that rule
/// lives in two places — the sender's decision to send one and the receiver's
/// decision to keep nothing — which is exactly the shape of rule this project
/// keeps finding one copy of rotted. The receiver's copy is the one worth
/// pinning: a sender that stops sending openings breaks a conversation
/// visibly, where a receiver that starts storing them puts an empty line in
/// somebody's thread.
///
/// Both halves are asserted, because either alone is passable by accident. A
/// ratchet that did not move would leave no message either, and a thread that
/// was never written to would have nothing to count.
#[test]
fn an_opening_leaves_no_message_behind() {
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    pair_with(&peer);

    let thread = list_threads().unwrap()[0].thread_id;
    let before = ratchet_fingerprint_for_test(thread)
        .unwrap()
        .expect("a paired thread with no ratchet");

    // Sent from the peer, not the engine, and after `pair_with` has already
    // settled which of them opened which. Both chains are two-way by now, so
    // this one lands whichever role the ceremony handed out — which is what
    // makes the test say the same thing on every run.
    peer.send_opening();
    until("the engine to take the opening", || {
        pump(&peer);
        ratchet_fingerprint_for_test(thread).unwrap() != Some(before)
    });

    assert!(
        thread_messages(thread).unwrap().is_empty(),
        "an opening put a message in the thread"
    );
}

/// Both ends of a pairing can write when the engine is the one that speaks
/// first.
///
/// Half of the requirement this slice exists for, and the easy half: this end
/// takes the ratchet's initiator role, so it could already write the moment it
/// paired. What is under test is that its opening reached the other device and
/// left that one able to write too.
#[test]
fn both_ends_can_write_with_the_engine_speaking_first() {
    both_ends_can_write(true);
}

/// Both ends of a pairing can write when the *peer* is the one that speaks
/// first.
///
/// The half the slice was written for. A `Ratchet::responder` has no sending
/// chain of its own, so before there was an opening frame this end paired and
/// then could not send a single message until the other person happened to
/// write — and nothing on screen said why. Without `full_peer_facing` this case
/// ran on a coin flip.
#[test]
fn both_ends_can_write_with_the_peer_speaking_first() {
    both_ends_can_write(false);
}

/// Pair with a peer placed on the chosen side of the ratchet, then check that
/// both devices can actually write.
///
/// The flag alone is not the property. A message the engine seals has to be one
/// the other device can open, and the two come apart exactly where this slice is
/// most dangerous: an engine that sealed its opening and never wrote the turn
/// down rewinds its message counter on the next send, reusing a key *and* a
/// nonce, and the message lands looking perfectly ordinary and refuses to open.
fn both_ends_can_write(engine_initiates: bool) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer_facing(&h.air, "peer", "Ada", engine_initiates);

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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    let device_id = pair_with(&peer);

    let thread = list_threads().unwrap()[0].thread_id;
    assert_eq!(ratchet_can_send_for_test(thread).unwrap(), Some(true));
    assert!(peer.can_send(), "the other end cannot write");
    if engine_initiates {
        // What the engine sent to open the chain, seen from the other device.
        // An opening is defined to carry nothing, and this is the only place
        // that can say whether it does — the receiver drops the plaintext, so a
        // sender that started putting something in one would go unnoticed on
        // the side that has to be trusted not to store it.
        assert_eq!(
            peer.heard().first().map(String::as_str),
            Some(""),
            "the engine's opening carried a body"
        );
    }

    let text = "so, which of us was the initiator?";
    send_chat(device_id, text.into()).unwrap();
    until("the peer to read it", || {
        pump(&peer);
        peer.heard().iter().any(|t| t == text)
    });
}

/// A message typed before the other end opens the chain is held, not lost.
///
/// The window this design pays for. The two devices finish the same ceremony
/// milliseconds apart, and for those milliseconds the responder is paired,
/// looking at a thread, and holds no sending chain. Somebody can type into that.
///
/// Sealing is the first thing `write_then_send` does, so a refusal there would
/// mean no row at all: what they wrote would be gone, and what they would see is
/// "message could not be decrypted" — frightening, and about something else.
/// R0-F5 already has the right answer for a message that cannot leave yet, and
/// this is one more reason it cannot.
#[test]
fn a_message_written_before_the_chain_opens_is_held() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    // The engine on the responding side, so it is the one that cannot write.
    let peer = full_peer_facing(&h.air, "peer", "Ada", false);

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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    let device_id = pair_only(&peer);

    let thread = list_threads().unwrap()[0].thread_id;
    assert_eq!(
        ratchet_can_send_for_test(thread).unwrap(),
        Some(false),
        "the engine was given a sending chain it should not have yet"
    );

    // The limit still applies on this path, and this is the only place it can
    // be seen: `ChatEnvelope::new` bounds the wire body, and the wire body here
    // is empty because nothing could be sealed. Without a check on the
    // plaintext an oversized message would be taken down, held, and refused
    // later by the frame layer for a different reason.
    assert!(
        send_chat(device_id.clone(), "x".repeat(MAX_BODY + 1)).is_err(),
        "an oversized message was held rather than refused"
    );

    // Not an error, and not a lost message.
    send_chat(device_id, "typed too soon".into()).expect("a send that had to be held");
    assert_eq!(
        thread_messages(thread).unwrap().len(),
        1,
        "what was typed is not in the thread"
    );
    assert_eq!(
        queued_on_thread_for_test(thread).unwrap(),
        1,
        "a message that cannot have left was marked as sent"
    );

    // And it goes out on its own the moment the other end opens the chain —
    // no second tap, which is the same promise R0-F5 makes about a reunion.
    peer.open_the_chain();
    until("the held message to go out", || {
        pump(&peer);
        peer.heard().iter().any(|t| t == "typed too soon")
    });
}

/// A message that arrives on a paired thread is announced as what was written.
///
/// The event and the row are two different pieces of code reading two different
/// values, and only one of them was ratcheted. The row held the opened body and
/// the event held `envelope.body` — so a thread already on screen drew a ratchet
/// header and a block of ciphertext, and only leaving the thread and coming
/// back put the real line there. Every test passed: they all read the row.
#[test]
fn an_arriving_message_is_announced_as_what_was_written() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);

    let event = receive_chat_for_test(&device_id, &peer.seal_chat(1, b"can you read this?"))
        .unwrap()
        .expect("a message the engine kept");
    let CoreEvent::MessageReceived { text, .. } = event else {
        panic!("a chat message was announced as something else");
    };
    assert_eq!(text, "can you read this?");
}

/// A message this end declines still turns the ratchet.
///
/// The row keeps plaintext, so `resend_queued` seals again — which means a
/// duplicate `seq` carries a message number this end has never stepped to.
/// Dropped before opening, the sender advances and the receiver does not, and
/// the gap is permanent: `walk` refuses more than `MAX_SKIP` at once, so enough
/// resends and the next genuinely new message is undecryptable with nothing to
/// be done about it.
///
/// The message itself is still dropped — that is what the inbox is for, and
/// `the_same_message_arriving_twice_is_one_message` says so.
#[test]
fn a_declined_message_still_turns_the_ratchet() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = list_threads().unwrap()[0].thread_id;

    let first = ChatEnvelope::new(1, b"hello".to_vec()).unwrap();
    receive_chat_for_test(&device_id, &peer.seal_envelope(&first))
        .unwrap()
        .expect("the first message");
    let before = ratchet_fingerprint_for_test(thread).unwrap().unwrap();

    // The same envelope — same `seq`, same `msg_id` — sealed again. That is
    // exactly what `resend_queued` puts on the wire, and the reason it is not
    // simply a replay: the ciphertext is new every time, so the ratchet has
    // never seen these bytes even though the message is one this end already
    // has.
    assert!(
        receive_chat_for_test(&device_id, &peer.seal_envelope(&first))
            .unwrap()
            .is_none(),
        "a resend was shown twice"
    );
    assert!(
        ratchet_fingerprint_for_test(thread).unwrap() != Some(before),
        "the ratchet did not step over a message it declined to keep"
    );

    // The same for a number this store cannot hold. The sender stepped its
    // chain to send it whatever we think of the number on the outside, and a
    // peer only has to be declined 257 times — for any reason — before the next
    // genuinely new message cannot be opened at all.
    let before = ratchet_fingerprint_for_test(thread).unwrap().unwrap();
    assert!(
        receive_chat_for_test(&device_id, &peer.seal_chat(u64::MAX, b"from the future"))
            .unwrap()
            .is_none(),
        "a seq that cannot be stored was announced anyway"
    );
    assert!(
        ratchet_fingerprint_for_test(thread).unwrap() != Some(before),
        "the ratchet did not step over a message it declined to keep"
    );

    // And a sealed body that is not an envelope at all. This is the case the
    // seal was moved outward for: it opened, so the sender stepped its chain,
    // and until the whole envelope travelled inside the seal there was no way
    // to know that — `Net` rejected the frame before the engine could open it,
    // and the turn went with it.
    let before = ratchet_fingerprint_for_test(thread).unwrap().unwrap();
    assert!(
        receive_chat_for_test(&device_id, &peer.seal(&[0xffu8; 8]))
            .unwrap()
            .is_none(),
        "noise surfaced as a message"
    );
    assert!(
        ratchet_fingerprint_for_test(thread).unwrap() != Some(before),
        "the ratchet did not step over a message it declined to keep"
    );

    // And the conversation carries on from where the resends left it.
    let event = receive_chat_for_test(&device_id, &peer.seal_chat(2, b"still there?"))
        .unwrap()
        .expect("the message after the resend");
    let CoreEvent::MessageReceived { text, .. } = event else {
        panic!("a chat message was announced as something else");
    };
    assert_eq!(text, "still there?");
}

/// A message longer than a message may be is refused, and the longest allowed
/// one is not.
///
/// `MAX_BODY` bounds what a person types, and it is `ChatEnvelope::new` that
/// enforces it — which works only because the envelope is now built from the
/// plaintext on every path out, including the one where nothing can be sealed
/// yet. While the ratchet sealed the *body* rather than the envelope, this
/// limit was measured against the sealed bytes: fifty-six characters short
/// where it was checked, and absent where the sealed body was empty.
#[test]
fn a_message_longer_than_a_message_may_be_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = list_threads().unwrap()[0].thread_id;

    assert!(
        send_chat(device_id.clone(), "x".repeat(MAX_BODY + 1)).is_err(),
        "an oversized message was taken down"
    );
    assert!(
        thread_messages(thread).unwrap().is_empty(),
        "a refused message left a row behind"
    );
    // The bound is a bound, and not an off-by-one that quietly costs the last
    // characters of a long message.
    send_chat(device_id, "x".repeat(MAX_BODY)).expect("a message at the limit");
    assert_eq!(thread_messages(thread).unwrap().len(), 1);
}

/// A chat frame that is not an envelope is dropped, and leaves no row.
///
/// The decision `Net` used to make, moved to where the ratchet is. It had to
/// move: a chat frame is a sealed envelope now, so the only layer that can tell
/// a message from noise is the one that can open it — and judging it earlier
/// meant discarding a message that would have opened, along with the chain step
/// its sender had already taken.
///
/// Landing here does not make it milder. One bad message is one bad message: no
/// row, no event, and nothing on anybody's screen.
#[test]
fn an_unreadable_chat_message_is_dropped_and_leaves_no_row() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    assert!(
        receive_chat_for_test("a-stranger", &[0xffu8; 8])
            .unwrap()
            .is_none(),
        "noise surfaced as a message"
    );
    // And nothing was written under it. `ensure_thread` opens a thread for
    // anyone who speaks — R0-F5 lets strangers chat — so the thread existing is
    // expected; a row in it is not.
    let thread = thread_for_device("a-stranger".into()).unwrap().unwrap();
    assert!(
        thread_messages(thread).unwrap().is_empty(),
        "noise was written down as something somebody said"
    );

    // And a real message straight after still arrives, so one bad frame has not
    // poisoned the thread.
    let good = ChatEnvelope::new(1, b"still here".to_vec()).unwrap();
    assert!(receive_chat_for_test("a-stranger", &good.encode())
        .unwrap()
        .is_some());
}

/// A resend goes out as the bytes it was first sealed as, and costs no key.
///
/// The bound this slice exists to remove. Re-sealing at every reunion drew a
/// fresh message key each time, so a frame the transport accepted and then lost
/// left the receiver's chain one behind — permanently, because `walk` will not
/// close a gap past `MAX_SKIP` and a reply does not heal it either: the turn
/// walks the old chain up to `previous_chain_len` first, and that walk is what
/// trips the bound.
///
/// The fingerprint standing still is the whole assertion. It says no message
/// key was drawn, which is what makes a lost resend cost nothing at all rather
/// than one step of a budget that cannot be refilled.
#[test]
fn a_resend_costs_no_message_key() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = list_threads().unwrap()[0].thread_id;

    // A message that cannot leave: with Discovery off the peer is not
    // addressable, so the row is written and sealed and stays `Queued`. That is
    // R0-F5's ordinary case, and the only one where a resend exists to be
    // watched.
    set_discovery(false).unwrap();
    send_chat_to_thread(thread, "for later".into()).expect("a message that had to wait");
    assert_eq!(
        queued_on_thread_for_test(thread).unwrap(),
        1,
        "the message did not wait"
    );
    let after_sealing = ratchet_fingerprint_for_test(thread).unwrap().unwrap();

    // The reunion.
    set_discovery(true).unwrap();
    until("the peer to be reachable again", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    resend_queued_for_test(&device_id).expect("a reunion that could not send");

    assert!(
        ratchet_fingerprint_for_test(thread).unwrap() == Some(after_sealing),
        "the resend drew a fresh message key"
    );
    // And the bytes it sent are ones the other device can open — a seal kept
    // from an earlier chain position is no use if it no longer decrypts.
    until("the peer to read it", || {
        pump(&peer);
        peer.heard().iter().any(|t| t == "for later")
    });
}

/// Pairing somebody you were already chatting with keeps the conversation.
///
/// T12 decided this on purpose: people chat and Ping before they pair (R0-F3,
/// R0-F5), and pairing is not a reason to put what you already said behind a
/// divider. Their numbering runs straight on through the ceremony — the same
/// outbox on their side, the next `seq` after the last — so there is nothing
/// here for a new thread to fix.
///
/// Which makes it the exception to the rule beside it, and the reason that rule
/// is "pairing *again*" rather than "pairing". Only covered at the store level
/// before; a first pairing that adopts an existing thread is also the shape
/// that used to hang the harness, because the thread count does not move.
#[test]
fn pairing_someone_you_already_chat_with_keeps_the_conversation() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");

    // A stranger, said before any ceremony.
    receive_chat_for_test(
        "peer",
        &ChatEnvelope::new(1, b"we have not met".to_vec())
            .unwrap()
            .encode(),
    )
    .unwrap()
    .expect("a stranger's message");
    let before = thread_for_device("peer".into()).unwrap().unwrap();
    assert_eq!(thread_messages(before).unwrap().len(), 1);

    let device_id = meet_and_pair(&peer);

    assert_eq!(
        thread_for_device(device_id).unwrap(),
        Some(before),
        "pairing put what was already said behind a divider"
    );
    assert_eq!(
        thread_messages(before).unwrap().len(),
        1,
        "the conversation lost what was said before the ceremony"
    );
    assert_eq!(
        list_threads().unwrap().len(),
        1,
        "a first pairing opened a second conversation"
    );
}

/// Pairing again opens a new conversation and closes the old one.
///
/// The case this exists for is narrower than it first looks, and worth stating
/// exactly. A peer that crypto-erases (R0-F9) comes back with a *new* Layer-1
/// identity, which is a new contact and a new thread already — nothing to fix.
/// The one that bites is a peer whose **identity survives while its store does
/// not**, which on Android is an ordinary thing rather than a corner: the
/// keystore-backed key lives outside the app's data, so clearing that data
/// leaves the same person with an empty outbox. Their numbering restarts at
/// `seq = 1` into an inbox that has already seen a hundred, and their first
/// message home is dropped as a duplicate of something said before it knew
/// this device.
///
/// Both halves matter and they pull against each other: the new conversation
/// has to work from nothing, and the old one has to still be there, because
/// what somebody said does not stop having been said.
#[test]
fn pairing_again_opens_a_second_conversation() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();

    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let first = thread_for_device(device_id.clone()).unwrap().unwrap();
    send_chat(device_id.clone(), "before".into()).expect("a message on the first thread");

    // The same identity, a second ceremony. This is the peer that kept its keys
    // and lost its messages.
    peer.forget_ratchet();
    let device_again = pair_with(&peer);
    let second = thread_for_device(device_again.clone()).unwrap().unwrap();
    assert_ne!(second, first, "the second pairing reused the first thread");

    // The new conversation counts from nothing, which is the whole point: their
    // `seq = 1` has to land here rather than be refused by an inbox that
    // remembers a conversation this one never had.
    assert!(
        thread_messages(second).unwrap().is_empty(),
        "the new conversation started with the old one's messages in it"
    );
    receive_chat_for_test(&device_again, &peer.seal_chat(1, b"after"))
        .unwrap()
        .expect("their first message home was refused");
    assert_eq!(thread_messages(second).unwrap().len(), 1);

    // And the old one is intact, and closed.
    assert_eq!(
        thread_messages(first).unwrap().len(),
        1,
        "the first conversation lost what was said in it"
    );
    assert!(
        send_chat_to_thread(first, "still there?".into()).is_err(),
        "a finished conversation took a message it can never send"
    );
    assert_eq!(
        thread_messages(first).unwrap().len(),
        1,
        "the refusal left a row behind"
    );
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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });

    let device_id = pair_with(&peer);

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
            .any(|d| d.device_id.as_deref() == Some(device_id.as_str()) && d.paired),
        "a paired friend read as unpaired once the session was gone"
    );
}

/// A person you paired with does not disappear because the radio stopped
/// looking.
///
/// R0-F2 rotates transport ids and R0-F4 makes pairing durable, so a list built
/// only from what the radio can see loses a paired friend the moment they stop
/// advertising — the app forgetting somebody the user deliberately introduced
/// it to. Discovery off is the sharpest version of the same thing: nothing is
/// being scanned for at all, and the friend is still on disk.
///
/// What the row must *not* carry is a transport handle. An earlier attempt put
/// a durable pseudonym in that field to keep the row addressable, which made
/// every transport-routed action on it unroutable — Ping and Drop had nothing
/// to dial. Absent is the honest answer, and the thread is what stays.
#[test]
fn a_paired_friend_stays_on_the_list_with_discovery_off() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let stranger = advertising_peer(&h.air, "passer-by");

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
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });
    let _device_id = pair_with(&peer);

    // While she is still in front of us, the row already carries the thread the
    // away row will be addressed by — same handle, present or absent, so the
    // UI is not holding two different ideas of the same person.
    let listed = nearby_devices().unwrap();
    assert!(
        listed
            .iter()
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()) && d.thread_id.is_some()),
        "a sighted paired row had no conversation to open"
    );
    // Once, not twice. She is both a sighting and someone we have paired with,
    // and the list is built from both — a friend standing in front of you must
    // not appear a second time as away.
    assert_eq!(
        listed.iter().filter(|d| d.name == "Ada").count(),
        1,
        "a paired friend in front of us was listed twice"
    );

    // Somebody we have written to but never paired with. A contact and a thread
    // exist for them, which is exactly what makes them a good test: the rule is
    // *paired*, not *known*.
    send_chat("a-stranger".into(), "hello?".into()).unwrap();

    // The radio stops. Nothing is advertised, nothing is scanned for.
    set_discovery(false).unwrap();
    let devices = nearby_devices().unwrap();

    // The stranger goes, because seeing a stranger *is* the radio.
    assert!(
        !devices
            .iter()
            .any(|d| d.device_id.as_deref() == Some("passer-by")),
        "a stranger survived Discovery going off: {:?}",
        devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>()
    );
    drop(stranger);

    // And so does someone we merely chatted with: R0-F4 is what makes a row
    // outlive the radio, and a stranger has not done it. Counted rather than
    // asked whether every row is paired — an away row sets that flag itself, so
    // the flag cannot report a row that should not be there at all.
    assert_eq!(
        devices.len(),
        1,
        "exactly Ada was expected, got {:?}",
        devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>()
    );

    let ada = devices
        .iter()
        .find(|d| d.name == "Ada")
        .expect("a paired friend vanished when Discovery went off");
    assert!(
        ada.paired,
        "she was listed but not as someone we paired with"
    );
    assert!(
        ada.device_id.is_none(),
        "an away row carried a transport handle, which nothing can dial"
    );
    let thread = ada
        .thread_id
        .expect("no conversation to open on an away row");

    // And the thread still takes what she is told, with no radio at all.
    let dto = send_chat_to_thread(thread, "back in a bit".into())
        .expect("writing to an away friend was refused");
    assert_eq!(dto.text, "back in a bit");
    assert_eq!(dto.thread_id, thread);
    let msgs = thread_messages(thread).unwrap();
    assert_eq!(msgs.len(), 1, "the message was not kept");
    assert!(msgs[0].outgoing);
    // Kept, not sent. The session with her may well still be open — turning
    // Discovery off does not hang up — but F3 says a closed Discovery produces
    // no traffic, and `ping` has always enforced that. Writing to a thread must
    // not be the one way round it.
    assert_eq!(
        queued_on_thread_for_test(thread).unwrap(),
        1,
        "a message went out over the radio with Discovery off"
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
            .any(|d| d.device_id.as_deref() == Some("peer"))
    });
    ping("peer".into()).unwrap();
    until("the peer's persona to be known", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some("peer") && !d.name.is_empty())
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
            .any(|d| d.device_id.as_deref() == Some("peer"))
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
            .any(|d| d.device_id.as_deref() == Some("peer") && !d.name.is_empty())
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

/// The two-row bug, as it appeared on a phone (T09a).
///
/// With Discovery on, the nearby list showed the same Samsung twice: an unknown
/// device, and separately the paired contact as away. Both rows were one phone
/// on one desk, and nothing resolved it — the list is drawn from sightings and
/// from disk, and a friend who is merely advertising matched neither route.
/// R0-F2 rotates their id every twelve minutes precisely so that an advert
/// cannot be attributed to a person, so the app genuinely held nothing that
/// said this unknown device was Ada.
///
/// The rotation is what makes this the real thing rather than a re-run of
/// `a_paired_friend_still_reads_as_paired_with_no_session`. Losing the session
/// alone leaves the device id we paired under, and that still matches. Rotating
/// takes the last route away, which is exactly the state the phones were in:
/// nothing had dialled, so no session had ever been established, and the
/// duplicate stood indefinitely.
#[test]
fn a_paired_friend_who_only_advertises_is_listed_once() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let paired_under = meet_and_pair(&peer);

    // Walk out of range: the session goes, and the id we knew them by rotates
    // out from under us.
    peer.transport.disconnect("core").unwrap();
    until("the session to go", || {
        pump(&peer);
        !has_session(&paired_under)
    });
    peer.net.discovery().rotate(Instant::now()).unwrap();
    let now_called = peer.net.discovery().local_id();
    assert_ne!(now_called, paired_under, "the peer should have rotated");

    until("the engine to see the new advertisement", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(now_called.as_str()))
    });

    let listed = nearby_devices().unwrap();
    let hers: Vec<_> = listed
        .iter()
        .filter(|d| d.name == "Ada" || d.device_id.as_deref() == Some(now_called.as_str()))
        .collect();
    assert_eq!(
        hers.len(),
        1,
        "one phone, listed {} times: {:?}",
        hers.len(),
        listed
            .iter()
            .map(|d| (d.device_id.clone(), d.name.clone(), d.paired))
            .collect::<Vec<_>>()
    );
    let row = hers[0];
    assert_eq!(row.name, "Ada", "the row should have carried her name");
    assert!(row.paired, "the row should have carried the paired badge");
    assert_eq!(
        row.device_id.as_deref(),
        Some(now_called.as_str()),
        "the row should be addressable at the id she is advertising under now"
    );
    assert!(row.thread_id.is_some(), "the row should open her thread");
}

/// The other half of it: recognising a friend must not turn a stranger into
/// one. Eight bytes that nobody's Layer-1 key generated resolve to nobody, and
/// the row stays the unknown device it is.
#[test]
fn a_stranger_advertising_noise_is_still_a_stranger() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    meet_and_pair(&peer);

    // Not derived from any key we hold — the overwhelmingly likely case for any
    // eight bytes at all.
    let (_noise, _rx) = advertising_peer(&h.air, "passer-by");

    until("the engine to see the passer-by", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some("passer-by"))
    });

    let listed = nearby_devices().unwrap();
    let row = listed
        .iter()
        .find(|d| d.device_id.as_deref() == Some("passer-by"))
        .unwrap();
    assert!(!row.paired, "a stranger's advert read as a pairing");
    assert!(row.name.is_empty(), "a stranger's advert borrowed a name");
    assert!(
        row.thread_id.is_none(),
        "a stranger's advert opened somebody's conversation"
    );
}

/// Writing to a paired friend must not mint a stranger (T09a follow-up).
///
/// Found on two phones. The nearby list had recognised Ada from her advert and
/// drawn her by name — and tapping Chat on that very row started a second
/// conversation with "Unknown". The list resolved her by hint;
/// `contact_id_for_device`, which every send and receive goes through, knew
/// only the proved pseudonym and the rotating device id, so it fell through and
/// created a stranger.
///
/// The lasting part is worse than the duplicate. The new row holds the
/// placeholder key for that device id, so it wins the lookup from then on and
/// the paired friend is displaced for as long as the id lives — one tap on a
/// screen that had correctly recognised her.
#[test]
fn writing_to_a_friend_recognised_by_her_advert_uses_her_thread() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    meet_and_pair(&peer);

    // Walk out of range and come back under a fresh id, so the only thing that
    // can identify her is the advert.
    peer.transport.disconnect("core").unwrap();
    peer.net.discovery().rotate(Instant::now()).unwrap();
    let now_called = peer.net.discovery().local_id();
    until("the engine to see her new advertisement", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(now_called.as_str()))
    });

    let threads_before = list_threads().unwrap().len();
    send_chat(now_called.clone(), "hello again".into()).unwrap();

    assert_eq!(
        list_threads().unwrap().len(),
        threads_before,
        "writing to a recognised friend opened a second conversation: {:?}",
        list_threads()
            .unwrap()
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !list_threads().unwrap().iter().any(|t| t.name == "Unknown"),
        "a paired friend was written to as a stranger"
    );
}

/// A message written into a paired conversation goes when she is standing
/// there, even if the only thing identifying her is her advert.
///
/// This is what the phone actually did: `holding a message on thread 1 until we
/// meet again`, logged while Ada was on screen, named, with Discovery open.
/// Nothing was wrong with the thread — the sighting had resolved to a stranger
/// row minted earlier by the same gap, so no visible device belonged to the
/// thread's owner and the send had nowhere to go.
///
/// `device_for_thread` now asks who owns the thread and looks for *them*. It
/// used to compare against `thread_for_contact`, the contact's newest thread,
/// which is a second one-to-one assumption of the kind T12 already broke once.
#[test]
fn a_paired_conversation_reaches_a_friend_known_only_by_her_advert() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();

    // Out of range, back under a fresh id: no session, no known device id, and
    // the advert hint is the only route left.
    peer.transport.disconnect("core").unwrap();
    peer.net.discovery().rotate(Instant::now()).unwrap();
    let now_called = peer.net.discovery().local_id();
    until("the engine to see her new advertisement", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(now_called.as_str()))
    });

    // The property the fix is about: her conversation is addressable at the id
    // she is advertising under now. Delivery beyond this point is the reunion
    // flush, which has its own tests and its own timing — what broke on the
    // phone was earlier than that, and was that nothing on screen belonged to
    // the thread's owner at all.
    let row = nearby_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.device_id.as_deref() == Some(now_called.as_str()))
        .expect("she is not on the list");
    assert_eq!(row.name, "Ada", "her row lost her name after a rotation");
    assert!(row.paired, "her row lost its paired badge after a rotation");
    assert_eq!(
        row.thread_id,
        Some(thread),
        "her row points at a different conversation than the one she is paired on"
    );
    assert_eq!(
        with_device_for_thread(thread),
        Some(now_called),
        "the paired conversation could not find her, so a message would wait \
         for a meeting already happening"
    );
}

/// `device_for_thread` through the public surface: which device a thread would
/// be sent to right now.
fn with_device_for_thread(thread: i64) -> Option<String> {
    nearby_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.thread_id == Some(thread))
        .and_then(|d| d.device_id)
}

/// A stranger you have written to is still recognised by the id you wrote to.
///
/// The weakest of the four routes, and the last one tried — a device id rotates
/// and anybody may present one — but it is what keeps an unpaired conversation
/// attached to the person in front of you between rotations. Demoting it below
/// the advert hint (so a paired friend outranks a stray row minted under her
/// old id) must not delete it.
#[test]
fn a_stranger_written_to_is_still_recognised_by_that_id() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let (_peer, _rx) = advertising_peer(&h.air, "a-stranger");
    set_discovery(true).unwrap();
    until("the stranger to appear", || {
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some("a-stranger"))
    });

    // Writing to them creates the contact and the conversation.
    send_chat("a-stranger".into(), "hello?".into()).unwrap();
    let thread = thread_for_device("a-stranger".into()).unwrap().unwrap();

    // And the row for that id now carries it, so the next line goes to the same
    // conversation rather than opening another.
    let row = nearby_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.device_id.as_deref() == Some("a-stranger"))
        .expect("the stranger left the list after being written to");
    assert_eq!(
        row.thread_id,
        Some(thread),
        "a stranger's own conversation was not attached to their row"
    );
    assert!(!row.paired, "an unpaired stranger read as paired");
}

/// A row stops claiming what it cannot know (T14a).
///
/// `Sent` used to be terminal, so a message that left this device looked the
/// same as one the other person is reading — including one they refused, which
/// is what T12a found. Nothing on the wire came back, so nothing could tell
/// them apart.
///
/// Now the receiver seals the `msg_id` it stored and sends it back, and only
/// that moves a row to `Delivered`. A message that was refused, lost, or never
/// arrived is not acknowledged, so its row stays `Sent` — the row says the
/// bytes left, which is all this end ever knew.
#[test]
fn a_message_reaches_delivered_only_when_the_far_side_says_so() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();

    let sent = send_chat_to_thread(thread, "did you get this?".into()).unwrap();
    let msg_id = hex::decode(&sent.msg_id).unwrap();

    // It left, and that is all this end knows until she answers.
    until("her to read it", || {
        pump(&peer);
        peer.heard().iter().any(|t| t == "did you get this?")
    });

    until("her acknowledgement to come back", || {
        pump(&peer);
        message_state_for_test(&msg_id).unwrap().as_deref() == Some("Delivered")
    });
}

/// And a message nobody answered stays `Sent`.
///
/// The half that makes the other half mean anything: if a row reached
/// `Delivered` on its own, the state would be a restatement of "we tried" and
/// T12a's silent loss would look exactly the same as success.
#[test]
fn a_message_nobody_acknowledges_stays_sent() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();

    // She receives it and says nothing — what a refused message looks like from
    // this side. The bytes really do leave, which is the part that makes the
    // assertion below mean anything: a message that never went out would sit at
    // `Queued`, which is a different failure and the one this test used to be
    // quietly passing for.
    peer.go_silent();
    let sent = send_chat_to_thread(thread, "into the dark".into()).unwrap();
    let msg_id = hex::decode(&sent.msg_id).unwrap();

    until("her to have read it", || {
        pump(&peer);
        peer.heard().iter().any(|t| t == "into the dark")
    });
    for _ in 0..10 {
        pump(&peer);
    }
    // `Sent`, not merely "not Delivered". Review caught the weaker assertion:
    // a message that never left at all would satisfy it, so the test would
    // have passed for a regression where the bytes stopped going out — the
    // opposite failure from the one it is about.
    assert_eq!(
        message_state_for_test(&msg_id).unwrap().as_deref(),
        Some("Sent"),
        "a row that got no acknowledgement should say the bytes left, and only that"
    );
}

/// An unpaired thread says nothing back, and that is a rule rather than an
/// omission.
///
/// A stranger's thread has no ratchet, so any acknowledgement it sent would be
/// unsealed — bytes anybody in range could write. A forged ack is worse than a
/// forged failure: it makes a person believe a message arrived that did not.
/// So those rows stay `Sent`, saying exactly what this end knows.
#[test]
fn an_unpaired_thread_does_not_acknowledge() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    let envelope = ChatEnvelope::new(1, b"hello from a stranger".to_vec()).unwrap();
    assert!(
        !acked_on_receipt_for_test("a-stranger", &envelope.encode()).unwrap(),
        "an unpaired thread sent an unsealed delivery claim"
    );
}

/// A message nobody acknowledged is marked, and left alone (T14a).
///
/// The decision, and it is a decision rather than a consequence: an unacked
/// message stays visible as unacked and is **not** resent by itself. Today that
/// falls out of `queued_for_resend` selecting `Queued` rows, which is an
/// accident of which state the query names — so it is pinned here before
/// somebody widens that query and turns every lost acknowledgement into a
/// duplicate delivery arriving days later.
///
/// The alternative was rejected on what it would cost the person rather than
/// the protocol. A receiver dedupes on `msg_id`, so an automatic resend is safe
/// — it is just never *finished*: a message that cannot be delivered is retried
/// at every reunion for ever, and the one thing nobody can do about it is know
/// that is what is happening. Marked and still costs a decision; resent for
/// ever costs attention and never asks.
#[test]
fn an_unacknowledged_message_is_not_resent_by_itself() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id.clone()).unwrap().unwrap();

    // Written and away, with nothing coming back — the state this decision is
    // about.
    set_discovery(false).unwrap();
    let sent = send_chat_to_thread(thread, "did that land?".into()).unwrap();
    let msg_id = hex::decode(&sent.msg_id).unwrap();
    mark_sent_for_test(thread).unwrap();
    assert_eq!(
        message_state_for_test(&msg_id).unwrap().as_deref(),
        Some("Sent"),
        "the row is not in the state this test is about"
    );

    // A reunion. It has plenty to do and nothing to do with this message.
    set_discovery(true).unwrap();
    until("the peer to be reachable again", || {
        pump(&peer);
        nearby_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id.as_deref() == Some(peer.id.as_str()))
    });

    let owed = queued_for_resend_for_test(&device_id).unwrap();
    assert!(
        !owed.iter().any(|(_, id)| *id == sent.msg_id),
        "a reunion resent a message that had already gone, with no request from anyone"
    );
    // And it is still marked, so the person can see it never arrived and decide
    // for themselves.
    assert_eq!(
        message_state_for_test(&msg_id).unwrap().as_deref(),
        Some("Sent"),
        "the mark was cleared by a reunion that did nothing"
    );
}

/// An unsealed acknowledgement is not an acknowledgement (T14a).
///
/// The whole shape rests on an ack being unforgeable: it rides the ratchet, so
/// producing one means holding a chain only a completed ceremony discloses. The
/// *sending* half was stated and tested. The receiving half was neither, and
/// `open_for_thread` hands back the raw bytes when a thread has no ratchet — so
/// sixteen bytes of anything, from anybody in range, promoted a message to
/// `Delivered` on an unpaired thread.
///
/// That is the exact lie the design refuses to permit: a forged ack makes a
/// person believe a message arrived that did not.
#[test]
fn a_forged_acknowledgement_on_an_unpaired_thread_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _h = fresh();

    // A stranger's conversation: no ceremony, so no ratchet, so nothing that
    // could seal anything.
    let sent = send_chat("a-stranger".into(), "did this land?".into()).unwrap();
    let msg_id = hex::decode(&sent.msg_id).unwrap();
    let thread = thread_for_device("a-stranger".into()).unwrap().unwrap();
    mark_sent_for_test(thread).unwrap();
    assert_eq!(
        message_state_for_test(&msg_id).unwrap().as_deref(),
        Some("Sent")
    );

    // Sixteen bytes anybody could write, naming the message they want to look
    // delivered.
    mark_delivered_for_test("a-stranger", &msg_id).unwrap();

    assert_eq!(
        message_state_for_test(&msg_id).unwrap().as_deref(),
        Some("Sent"),
        "an unsealed acknowledgement was believed"
    );
}

/// What a send hands back agrees with what it wrote down.
///
/// The row was promoted to `Sent` while the returned DTO still said `Queued` —
/// one fact in two places, disagreeing. Every caller re-reads the thread after
/// sending, which is precisely why nothing caught it: the wrong value was
/// never the one drawn. It would have bitten the first caller that trusted
/// what it was given.
#[test]
fn what_a_send_returns_says_what_the_row_says() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();

    // She is here, so this one really goes.
    let sent = send_chat_to_thread(thread, "away it goes".into()).unwrap();
    let msg_id = hex::decode(&sent.msg_id).unwrap();
    assert_eq!(
        format!("{:?}", sent.state),
        message_state_for_test(&msg_id).unwrap().unwrap(),
        "the value handed back disagreed with the row it wrote"
    );

    // And with nobody to hand it to, both say it is still waiting.
    set_discovery(false).unwrap();
    let held = send_chat_to_thread(thread, "and this one waits".into()).unwrap();
    let held_id = hex::decode(&held.msg_id).unwrap();
    assert_eq!(
        format!("{:?}", held.state),
        message_state_for_test(&held_id).unwrap().unwrap(),
        "a message with nowhere to go disagreed with its own row"
    );
}

/// A refused message says which kind of refusal it was (T12a slice 2).
///
/// Every unopenable body used to produce one line — *dropping a chat message
/// that would not open* — so a message written before a pairing, a corrupt
/// frame and a forgery were indistinguishable in a log and invisible on a
/// screen. The bytes are refused either way; what changes is what can be said
/// about them.
///
/// The reading is a guess about a shape and never a reason to keep anything,
/// which is what makes it safe to look at bytes that just failed a check.
#[test]
fn a_refused_message_says_which_kind_of_refusal_it_was() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    meet_and_pair(&peer);

    // The case this slice exists for: a whole envelope in the clear, arriving
    // on a thread that now opens only sealed ones.
    let bare = ChatEnvelope::new(1, b"from before we paired".to_vec()).unwrap();
    assert_eq!(
        refusal_for_test(&peer.id, &bare.encode())
            .unwrap()
            .as_deref(),
        Some("it looked like a message from before you paired"),
    );

    // Too short to have been a sealed anything.
    assert_eq!(
        refusal_for_test(&peer.id, &[1, 2, 3]).unwrap().as_deref(),
        Some("it was too short to be a message"),
    );

    // Long enough to carry a ratchet header, and not one this end can follow.
    assert_eq!(
        refusal_for_test(&peer.id, &[0xab; 128]).unwrap().as_deref(),
        Some("it could not be opened on this conversation"),
    );
}

/// A row moving forward tells the screen (T14a follow-up).
///
/// Found on two phones. The conversation redrew when a message *arrived* and at
/// no other time, so a line of your own sat at "not confirmed" while the store
/// already said `Delivered` — and only leaving the screen and coming back put
/// it right. Watching your own message go is exactly when somebody is looking
/// at it.
///
/// Asserted on what reaches the stream, not on the row. The row was already
/// correct; what was missing was anybody being told.
#[test]
fn a_row_moving_forward_is_announced() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();
    record_events_for_test();
    let _ = drain_events_for_test();

    let sent = send_chat_to_thread(thread, "watch this".into()).unwrap();

    // It left: the row is `Sent`, and the screen was told so.
    let announced: Vec<_> = drain_events_for_test()
        .into_iter()
        .filter_map(|e| match e {
            CoreEvent::MessageStateChanged { msg_id, state, .. } => Some((msg_id, state)),
            _ => None,
        })
        .collect();
    assert!(
        announced
            .iter()
            .any(|(id, s)| *id == sent.msg_id && matches!(s, MessageStateDto::Sent)),
        "nothing told the screen the message had gone; saw {announced:?}"
    );

    // And again when she acknowledges it.
    until("her acknowledgement", || {
        pump(&peer);
        message_state_for_test(&hex::decode(&sent.msg_id).unwrap())
            .unwrap()
            .as_deref()
            == Some("Delivered")
    });
    let announced: Vec<_> = drain_events_for_test()
        .into_iter()
        .filter_map(|e| match e {
            CoreEvent::MessageStateChanged { msg_id, state, .. } => Some((msg_id, state)),
            _ => None,
        })
        .collect();
    assert!(
        announced
            .iter()
            .any(|(id, s)| *id == sent.msg_id && matches!(s, MessageStateDto::Delivered)),
        "the row reached Delivered with nothing told; saw {announced:?}"
    );
}

/// With nobody listening, nothing is kept.
///
/// The shape a phone runs in for the moment between `core_init` and Dart
/// subscribing: no sink attached, and events being emitted. Review caught this
/// buffering them — up to 256 held for ever and never delivered, which is a
/// leak and a silence at once. Keeping is now something a test asks for, not
/// something the absence of a listener causes.
#[test]
fn events_with_nobody_listening_are_not_kept() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let h = fresh();
    let peer = full_peer(&h.air, "peer", "Ada");
    let device_id = meet_and_pair(&peer);
    let thread = thread_for_device(device_id).unwrap().unwrap();

    // Explicitly off: the flag is process-wide and another test may have set it.
    stop_recording_for_test();
    send_chat_to_thread(thread, "into the void".into()).unwrap();

    assert!(
        drain_events_for_test().is_empty(),
        "events piled up with no sink attached and nobody having asked for them"
    );
}
