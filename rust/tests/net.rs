//! End-to-end tests for the engine's networking half (T10 part 2b).
//!
//! This is the first test in the project where every layer runs together:
//! transport → discovery → persona fetch → Noise IK → session → frames. Each
//! layer is already tested alone, and none of that tells you they compose —
//! the wiring between them is exactly where the ordering assumptions live.
//!
//! `Net` is not a singleton for this reason. The engine is process-wide, so an
//! engine-against-engine test cannot exist; two `Net`s can talk over the
//! loopback rung and the whole stranger→session→Ping path becomes reachable
//! without a radio.

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_lib_hoppler::engine::net::{Net, NetEvent};
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportEvent};

struct Node {
    net: Net,
    transport: Arc<dyn Transport>,
    rx: Receiver<TransportEvent>,
    id: String,
    identity: Arc<Mutex<Identity>>,
}

/// Every test builds its airspace here, and every one of them fragments.
///
/// Fragmentation is the *normal* case on the rung this ships to — T08's
/// contract says a rung may split or merge freely and BLE certainly will — so a
/// suite that only exercised whole messages would be testing a transport
/// Hoppler does not have. Review caught exactly that: the handshake path had no
/// reassembly at all, and the boundary-preserving default was hiding it.
///
/// Seven bytes: small enough to split a 3-byte pipe header across chunks, which
/// is the case a length-prefix reader is most likely to get wrong.
fn air() -> LoopbackNet {
    LoopbackNet::with_max_chunk(7)
}

fn node(air: &LoopbackNet, id: &str, name: &str, now: Instant) -> Node {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let transport: Arc<dyn Transport> = Arc::new(air.join(id, sink));
    let identity = Arc::new(Mutex::new(Identity::generate(name, 0x00_88_ff)));
    Node {
        net: Net::new(transport.clone(), identity.clone(), id, now),
        transport,
        rx,
        id: id.to_string(),
        identity,
    }
}

/// Run both nodes' event loops until neither has anything left to say,
/// collecting what each surfaced. The exchange is several round trips — fetch
/// persona, handshake, reply — so this pumps until quiet rather than a fixed
/// number of times.
fn settle(a: &Node, b: &Node, now: Instant) -> (Vec<NetEvent>, Vec<NetEvent>) {
    let (mut out_a, mut out_b) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        let mut moved = false;
        while let Ok(event) = a.rx.recv_timeout(Duration::from_millis(30)) {
            out_a.extend(a.net.handle(event, now));
            moved = true;
        }
        while let Ok(event) = b.rx.recv_timeout(Duration::from_millis(30)) {
            out_b.extend(b.net.handle(event, now));
            moved = true;
        }
        if !moved {
            break;
        }
    }
    (out_a, out_b)
}

fn opened_with(events: &[NetEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        NetEvent::SessionOpened { persona_name, .. } => Some(persona_name.clone()),
        _ => None,
    })
}

// ── the whole path ──────────────────────────────────────────────────────────

/// Two strangers: discover, fetch persona, handshake, exchange a Ping.
///
/// This is T10's first acceptance line ("two unpaired devices establish a
/// session and exchange echo frames"), minus the wall-clock bound, which
/// belongs on real hardware rather than a loopback rung that would always pass.
#[test]
fn two_strangers_go_from_sighting_to_a_ping() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    let (a_events, _) = settle(&alice, &bob, now);
    assert!(
        a_events.contains(&NetEvent::PeersChanged),
        "Alice never saw Bob appear"
    );

    // Alice reaches for Bob. Everything after this — the persona fetch, the IK
    // handshake, the session — happens without another call.
    alice.net.reach(&bob.id).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);

    assert_eq!(
        opened_with(&a_events).as_deref(),
        Some("Bob"),
        "Alice has no session with Bob: {a_events:?}"
    );
    assert_eq!(
        opened_with(&b_events).as_deref(),
        Some("Alice"),
        "Bob has no session with Alice: {b_events:?}"
    );

    // And a Ping crosses, arriving as the peer it actually came from.
    alice.net.ping(&bob.id, now).unwrap();
    let (_, b_events) = settle(&alice, &bob, now);
    assert!(
        b_events.contains(&NetEvent::Pinged {
            peer: alice.id.clone(),
            persona_name: "Alice".into(),
        }),
        "the Ping did not arrive: {b_events:?}"
    );
}

#[test]
fn chat_crosses_a_real_session() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);

    alice
        .net
        .send_chat(&bob.id, "hello over noise", now)
        .unwrap();
    let (_, b_events) = settle(&alice, &bob, now);
    assert!(
        b_events.contains(&NetEvent::ChatReceived {
            peer: alice.id.clone(),
            text: "hello over noise".into(),
        }),
        "chat did not arrive: {b_events:?}"
    );
}

/// R0-F10, end to end: a blocked device gets nothing, and cannot tell that from
/// us being out of range.
#[test]
fn a_blocked_peer_gets_no_session_and_no_signal() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    // Bob blocks Alice. He needs her pseudonym toward him, which is what a
    // previous session would have given him.
    let alice_pseudonym = {
        let a = alice.identity.lock().unwrap();
        let bob_persona = rust_lib_hoppler::identity::verify_persona_record(
            &bob.identity.lock().unwrap().persona_record(),
        )
        .unwrap();
        a.pseudonym_toward(&bob_persona.l2_pub).0
    };
    bob.net.block(alice_pseudonym);

    alice.net.reach(&bob.id).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);

    assert!(
        opened_with(&a_events).is_none(),
        "a blocked initiator established a session: {a_events:?}"
    );
    assert!(
        opened_with(&b_events).is_none(),
        "the responder opened a session with a blocked initiator: {b_events:?}"
    );
    // And nothing came back that would tell Alice she was refused rather than
    // simply unheard.
    assert!(
        !a_events
            .iter()
            .any(|e| matches!(e, NetEvent::SessionClosed { .. })),
        "the refusal was observable as a close: {a_events:?}"
    );

    // Sending is refused locally rather than silently dropped.
    assert!(alice.net.ping(&bob.id, now).is_err());
}

#[test]
fn a_severed_pipe_closes_the_session_and_it_can_be_rebuilt() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);
    assert!(alice.net.sessions().is_open(&bob.id));

    alice.transport.disconnect(&bob.id).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);
    assert!(a_events.contains(&NetEvent::SessionClosed {
        peer: bob.id.clone()
    }));
    assert!(b_events.contains(&NetEvent::SessionClosed {
        peer: alice.id.clone()
    }));
    assert!(!alice.net.sessions().is_open(&bob.id));

    // Re-establishment: the persona is already known, so this is a straight
    // handshake with no fetch.
    alice.net.reach(&bob.id).unwrap();
    let (a_events, _) = settle(&alice, &bob, now);
    assert_eq!(
        opened_with(&a_events).as_deref(),
        Some("Bob"),
        "the session did not come back: {a_events:?}"
    );
    alice.net.ping(&bob.id, now).unwrap();
}

/// The pseudonym a session proves is the value a block binds to — checked here
/// rather than only at the handshake, because this is the path the UI's Block
/// button will actually take.
#[test]
fn a_live_session_exposes_the_pseudonym_a_block_would_use() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);

    let seen_by_bob = bob
        .net
        .pseudonym(&alice.id)
        .expect("no session on Bob's side");
    let bob_persona = rust_lib_hoppler::identity::verify_persona_record(
        &bob.identity.lock().unwrap().persona_record(),
    )
    .unwrap();
    let expected = alice
        .identity
        .lock()
        .unwrap()
        .pseudonym_toward(&bob_persona.l2_pub);
    assert_eq!(
        seen_by_bob.0, expected.0,
        "the session's pseudonym is not the one a block would be recorded \
         against, so blocking from the UI would never match"
    );
}

/// Discovery off means unreachable, not merely invisible.
#[test]
fn a_peer_with_discovery_off_yields_no_session() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    // Bob never enables discovery, so his persona endpoint answers nothing and
    // Alice can never learn the static an IK handshake needs.
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    alice.transport.connect(&bob.id).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);
    assert!(opened_with(&a_events).is_none(), "{a_events:?}");
    assert!(opened_with(&b_events).is_none(), "{b_events:?}");
}

/// The tie-break must keep working after the id rotates.
///
/// Review caught that `Net` cached its own id while `Discovery` owned rotation:
/// twelve minutes later the two would disagree, both sides could believe they
/// were the smaller id — or neither would — and the double-initiator deadlock
/// the tie-break exists to prevent would return, now on a timer. The id is read
/// from `Discovery` every time instead, and this holds that in place.
#[test]
fn a_session_still_forms_after_the_local_id_rotates() {
    let air = air();
    let now = Instant::now();
    // "zz-…" so Alice starts as the *larger* id and rotation flips her side of
    // the comparison — a cached id would give the wrong answer afterwards.
    let alice = node(&air, "zz-alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    alice.net.discovery().set_enabled(true, now).unwrap();
    // Rotate until the new id sorts *below* Bob's, so the comparison genuinely
    // flips: Alice was the larger id and is now the smaller. Left to chance,
    // roughly a quarter of rotations land the other way and the test passes
    // whether the id is read live or cached — which is no test at all.
    let mut rotated = String::new();
    for _ in 0..40 {
        alice.net.discovery().rotate(now).unwrap();
        rotated = alice.net.discovery().local_id();
        if rotated.as_str() < "bob" {
            break;
        }
    }
    assert!(
        !rotated.is_empty() && rotated.as_str() < "bob",
        "could not rotate into an id below Bob's: {rotated}"
    );

    // Bob reaches for Alice under her new id; whichever side initiates, exactly
    // one must, and a session must appear.
    bob.net.reach(&rotated).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);
    assert!(
        opened_with(&a_events).is_some() && opened_with(&b_events).is_some(),
        "no session after rotation — the tie-break disagreed with itself\n\
         alice: {a_events:?}\nbob: {b_events:?}"
    );
}
