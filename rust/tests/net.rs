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

use rust_lib_hoppler::discovery::ROTATION_PERIOD;
use rust_lib_hoppler::engine::net::{Net, NetEvent, PING_DEADLINE};
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::session::table::IDLE_TIMEOUT;
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

    // A Ping is *accepted* and simply never arrives. It must not report an
    // error: an error is a signal, and telling Alice her Ping failed here is
    // precisely how she would learn she is blocked rather than out of range —
    // the distinction R0-F10 exists to deny her.
    alice
        .net
        .ping(&bob.id, now)
        .expect("a blocked peer's Ping reported failure, which is a signal");
    let (_, b_events) = settle(&alice, &bob, now);
    assert!(
        !b_events
            .iter()
            .any(|e| matches!(e, NetEvent::Pinged { .. })),
        "a blocked peer's Ping was delivered: {b_events:?}"
    );
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

/// The first Ping to a peer must work.
///
/// `reach` returns on acceptance, and a session needs a pipe, a persona fetch
/// and a handshake — so sending straight after it always failed the first time
/// and worked the second. On hardware that is "I have to tap Ping twice".
#[test]
fn the_first_ping_to_a_peer_arrives_without_a_second_tap() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    // No session yet — this is the tap that used to fail.
    assert!(!alice.net.sessions().is_open(&bob.id));
    alice
        .net
        .ping(&bob.id, now)
        .expect("the first ping was refused");

    let (_, b_events) = settle(&alice, &bob, now);
    assert!(
        b_events.contains(&NetEvent::Pinged {
            peer: alice.id.clone(),
            persona_name: "Alice".into(),
        }),
        "the first ping never arrived: {b_events:?}"
    );
}

/// A Ping that cannot be delivered must say so.
///
/// Queueing (added the same day) fixed "the first tap always fails" but made
/// an undeliverable Ping silent, which is worse: a wrong message is a lead, no
/// message is a blind alley.
#[test]
fn a_ping_to_an_unreachable_peer_reports_failure() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);

    // A peer that was seen and has since gone. `connect` fails, and the queued
    // Ping has to surface rather than evaporate.
    let _ = alice.net.ping("ghost", now);
    let events: Vec<NetEvent> = {
        let mut out = Vec::new();
        while let Ok(e) = alice.rx.recv_timeout(Duration::from_millis(100)) {
            out.extend(alice.net.handle(e, now));
        }
        out
    };
    assert!(
        events
            .iter()
            .any(|e| matches!(e, NetEvent::PingUndeliverable { .. })),
        "an undeliverable Ping vanished without a word: {events:?}"
    );
}

/// ...but a *blocked* peer must still produce nothing.
///
/// The distinction that makes the above safe for R0-F10: an unreachable peer
/// fails at connect, while a blocked one accepts the pipe and goes quiet during
/// the handshake. Only the first reaches the failure path.
#[test]
fn a_blocked_peer_is_indistinguishable_from_an_absent_one() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    let bob_persona = rust_lib_hoppler::identity::verify_persona_record(
        &bob.identity.lock().unwrap().persona_record(),
    )
    .unwrap();
    let pseudonym = alice
        .identity
        .lock()
        .unwrap()
        .pseudonym_toward(&bob_persona.l2_pub)
        .0;
    bob.net.block(pseudonym);

    // What an absent peer looks like, to compare against.
    let _ = alice.net.ping("ghost", now);
    let absent_why = {
        let mut found = None;
        while let Ok(e) = alice.rx.recv_timeout(Duration::from_millis(100)) {
            for out in alice.net.handle(e, now) {
                if let NetEvent::PingUndeliverable { why, .. } = out {
                    found = Some(why);
                }
            }
        }
        found.expect("an absent peer's Ping must be reported")
    };

    alice.net.ping(&bob.id, now).unwrap();
    let (a_events, _) = settle(&alice, &bob, now);
    assert!(
        !a_events
            .iter()
            .any(|e| matches!(e, NetEvent::PingUndeliverable { peer, .. } if peer == &bob.id)),
        "the pipe to a blocking peer opens normally, so nothing should be \
         concluded before the deadline: {a_events:?}"
    );

    // And at the deadline it must say the *same thing* an absent peer said.
    //
    // Note which way round this cuts. The instinct is that a blocked peer
    // should produce nothing — but silence is itself a signal, and a unique
    // one: absent peers report, so anyone who reports nothing is blocking you.
    // Indistinguishability is the requirement (R0-F10), not silence, and that
    // means a blocked peer must produce the identical message.
    let blocked_why = alice
        .net
        .expire_pings(now + PING_DEADLINE)
        .into_iter()
        .find_map(|e| match e {
            NetEvent::PingUndeliverable { peer, why } if peer == bob.id => Some(why),
            _ => None,
        })
        .expect(
            "a blocked peer's Ping was never reported — silence where an absent \
             peer reports is exactly how the sender learns it was refused",
        );

    assert_eq!(
        blocked_why, absent_why,
        "blocked and absent must be indistinguishable to the sender"
    );

    // The deadline above is computed *from* PING_DEADLINE, so on its own it
    // would pass however long that grew — a day included, which is the silence
    // this test exists to forbid wearing a different hat. Pin the policy
    // against a literal, separately from the mechanism.
    assert!(
        PING_DEADLINE <= Duration::from_secs(30),
        "a Ping that takes longer than this to report has told the person nothing"
    );
}

/// Tapping Ping twice must not destroy the session it is building.
///
/// `PipeOpened` is not once-per-pipe: T08 rule 2 makes the rung emit it again
/// for a `connect` to an already-connected peer, so a caller waiting on the
/// event never hangs. Every extra tap therefore delivers another one.
///
/// Before the fix that meant two handshakes down one pipe. The second
/// initiator overwrote the first in `pending`, so the reply to msg1 #1 was
/// decrypted against msg1 #2's ephemeral key — `Noise("decrypt error")` — while
/// the responder, which had opened a session from msg1 #1, read msg1 #2 as
/// ciphertext and dropped it. Both ends destroyed what they had just built, and
/// tapping harder made it worse.
///
/// Found on two phones. Every layer reported success: the pipe was healthy, the
/// persona verified, both msg1s sent `Ok`.
#[test]
fn a_second_pipe_opened_does_not_wreck_the_handshake_in_flight() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    // Two taps. The rung announces the pipe each time, exactly as the contract
    // says it must.
    alice.net.reach(&bob.id).unwrap();
    alice.net.handle(
        TransportEvent::PipeOpened {
            peer: bob.id.clone(),
        },
        now,
    );
    let (a_events, b_events) = settle(&alice, &bob, now);

    assert_eq!(
        opened_with(&a_events).as_deref(),
        Some("Bob"),
        "the duplicate announcement cost Alice the session: {a_events:?}"
    );
    assert_eq!(
        opened_with(&b_events).as_deref(),
        Some("Alice"),
        "the duplicate announcement cost Bob the session: {b_events:?}"
    );

    // And the thing the person was trying to do actually happens.
    alice.net.ping(&bob.id, now).unwrap();
    let (_, b_events) = settle(&alice, &bob, now);
    assert!(
        b_events.contains(&NetEvent::Pinged {
            peer: alice.id.clone(),
            persona_name: "Alice".into(),
        }),
        "the Ping did not arrive after a duplicate announcement: {b_events:?}"
    );
}

/// Both sides must end up able to name the other, not just the one that asked.
///
/// Names on the nearby list come from `Discovery`'s sightings, and only the
/// *initiator* fetches a persona over the discovery channel. The tie-break
/// fixes the initiator as the smaller id, so before the fix the larger id
/// showed a live peer — session open, Pings arriving — as a nameless tile, for
/// the life of the session and every session after it.
///
/// Deterministic, which is why the two-phone runs looked inconsistent: whichever
/// handset drew the larger id that run was the one with the blank tile.
///
/// Both directions are asserted because only one of them was ever broken, and a
/// test that checked the responder alone would not notice the fix breaking the
/// initiator.
#[test]
fn both_sides_learn_the_others_name() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    // Both advertising and both scanning, so each has a sighting of the other
    // to carry a name. "alice" < "bob", so Alice initiates and Bob responds.
    alice.net.discovery().set_enabled(true, now).unwrap();
    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    bob.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);

    alice.net.reach(&bob.id).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);
    assert!(
        opened_with(&a_events).is_some() && opened_with(&b_events).is_some(),
        "no session, so this test would prove nothing: {a_events:?} / {b_events:?}"
    );

    let named = |node: &Node, peer: &str| -> Option<String> {
        node.net
            .discovery()
            .sightings()
            .into_iter()
            .find(|s| s.peer == peer)
            .and_then(|s| s.persona)
            .map(|p| p.name)
    };

    assert_eq!(
        named(&alice, &bob.id).as_deref(),
        Some("Bob"),
        "the initiator lost the name it fetched"
    );
    assert_eq!(
        named(&bob, &alice.id).as_deref(),
        Some("Alice"),
        "the responder never learned the name, so its tile stays blank forever"
    );
}

/// A persona that arrives before the advertisement must not be thrown away.
///
/// The two orders both happen. A peer that dials *us* is known through the pipe
/// — hello, persona fetch, handshake — before its mDNS advertisement
/// necessarily reaches us. Storing the persona only onto an existing sighting
/// dropped it silently in that case, and the sighting created afterwards
/// started nameless with nothing to refill it.
///
/// Seen on hardware immediately after the first naming fix: the phone that
/// *accepted* the pipe showed a blank tile above a live session with Pings
/// arriving on it, while the phone that dialled showed the name correctly. The
/// first fix was right and incomplete — it inherited this hole from
/// `accept_persona`.
#[test]
fn a_persona_learned_before_the_sighting_survives() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    alice.net.discovery().start_scanning().unwrap();
    bob.net.discovery().set_enabled(true, now).unwrap();
    bob.net.reach(&alice.id).unwrap();

    // Pump both sides, but withhold Alice's sighting of Bob. Left to itself the
    // loopback rung delivers PeerFound first and the sighting exists before the
    // session does — which is the ordering that already worked, and a test that
    // allowed it passed with the fix removed.
    let mut a_events = Vec::new();
    for _ in 0..40 {
        let mut moved = false;
        while let Ok(e) = alice.rx.recv_timeout(Duration::from_millis(30)) {
            moved = true;
            if matches!(&e, TransportEvent::PeerFound { peer, .. } if peer == &bob.id) {
                continue;
            }
            a_events.extend(alice.net.handle(e, now));
        }
        while let Ok(e) = bob.rx.recv_timeout(Duration::from_millis(30)) {
            moved = true;
            bob.net.handle(e, now);
        }
        if !moved {
            break;
        }
    }

    assert!(
        opened_with(&a_events).is_some(),
        "no session, so this proves nothing: {a_events:?}"
    );
    assert!(
        !alice
            .net
            .discovery()
            .sightings()
            .iter()
            .any(|s| s.peer == bob.id),
        "Bob must still be unsighted here, or the ordering under test never happened"
    );

    // Only now does the advertisement land.
    alice.net.handle(
        TransportEvent::PeerFound {
            peer: bob.id.clone(),
            payload: Vec::new(),
        },
        now,
    );

    let named = alice
        .net
        .discovery()
        .sightings()
        .into_iter()
        .find(|s| s.peer == bob.id)
        .and_then(|s| s.persona)
        .map(|p| p.name);
    assert_eq!(
        named.as_deref(),
        Some("Bob"),
        "the persona was learned over the pipe and then dropped when the \
         sighting appeared, leaving a nameless tile above a live session"
    );
}

/// The radio's reason has to survive the trip, because nothing downstream can
/// reconstruct it.
///
/// `Availability` used to fold into a bare `PeersChanged`, which threw both
/// fields away — and `TransportEvent::Availability`'s own doc says it exists so
/// the UI can say "Bluetooth is off" instead of showing an empty list that
/// reads as "nobody is nearby" (R0-F2). The type promised it and the pipeline
/// dropped it, which is a thing only an end-to-end assertion catches: every
/// layer in between compiled perfectly happily.
#[test]
fn an_unusable_radio_says_why_and_does_not_just_empty_the_list() {
    let now = Instant::now();
    let air = air();
    let alice = node(&air, "alice", "Alice", now);

    let events = alice.net.handle(
        TransportEvent::Availability {
            available: false,
            reason: Some("Bluetooth is off".into()),
        },
        now,
    );

    let told = events.iter().find_map(|e| match e {
        NetEvent::RadioChanged { available, reason } => Some((*available, reason.clone())),
        _ => None,
    });
    assert_eq!(
        told,
        Some((false, Some("Bluetooth is off".into()))),
        "the reason the radio is unusable never reached the engine, so the \
         screen can only show an empty list — which is what being out of \
         range looks like too"
    );

    // The list still has to be redrawn: a radio going down takes every peer
    // with it. Losing this while adding the reason would trade one bug for
    // another.
    assert!(
        events.contains(&NetEvent::PeersChanged),
        "the nearby list was left stale after the radio went away"
    );
}

/// A radio that comes back must clear the sentence, not add to it.
#[test]
fn a_radio_that_returns_reports_itself_available_with_no_reason() {
    let now = Instant::now();
    let air = air();
    let alice = node(&air, "alice", "Alice", now);

    alice.net.handle(
        TransportEvent::Availability {
            available: false,
            reason: Some("Bluetooth is off".into()),
        },
        now,
    );
    let events = alice.net.handle(
        TransportEvent::Availability {
            available: true,
            reason: None,
        },
        now,
    );

    assert!(
        events.contains(&NetEvent::RadioChanged {
            available: true,
            reason: None
        }),
        "recovery went unreported, so a screen showing \"Bluetooth is off\" \
         has nothing to tell it otherwise"
    );
}

/// A ping comes back answered, without the other person doing anything.
///
/// Before there was a Pong, `Pinged` served as both the nudge and the answer —
/// so a tap looked acknowledged only when the peer happened to nudge back, and
/// an ordinary ping simply timed out. Worse, an unrelated incoming ping was
/// indistinguishable from the answer to one's own.
#[test]
fn a_ping_is_answered_by_the_peer_that_received_it() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);

    alice.net.ping(&bob.id, now).unwrap();
    let (a_events, b_events) = settle(&alice, &bob, now);

    assert!(
        b_events.contains(&NetEvent::Pinged {
            peer: alice.id.clone(),
            persona_name: "Alice".into(),
        }),
        "Bob never saw the nudge: {b_events:?}"
    );
    assert!(
        a_events.contains(&NetEvent::PingAcked {
            peer: bob.id.clone()
        }),
        "Alice's ping was never answered, so a tap can only ever time out: \
         {a_events:?}"
    );
}

/// The answer is not itself answered.
///
/// Two devices that each replied to the other's reply would nudge each other
/// forever, with no idle state and a session that never goes quiet. That is the
/// whole reason Pong is a separate kind rather than a Ping sent back.
#[test]
fn an_answered_ping_does_not_start_a_volley() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);

    alice.net.ping(&bob.id, now).unwrap();
    settle(&alice, &bob, now);

    // `settle` pumps until neither side has anything left to say. A volley
    // would never reach that point, so arriving here at all is half the
    // assertion; the counts are the other half.
    let (a_again, b_again) = settle(&alice, &bob, now);
    assert!(
        a_again.is_empty() && b_again.is_empty(),
        "the exchange did not go quiet: alice={a_again:?} bob={b_again:?}"
    );
}

// ── the clock ───────────────────────────────────────────────────────────────

// The two things that come due during silence, and so had no caller.
//
// `Discovery::tick` and `SessionTable::sweep` were both written, documented and
// unit-tested, and neither had ever run outside a test: nothing in the engine
// consulted a clock. On hardware that meant the advertised id never rotated —
// R0-F2 undelivered — and a session idle for nine minutes against a five-minute
// timeout was still open. These hold the caller in place.

/// A session nobody has used is dropped, and the pipe is hung up on.
#[test]
fn the_clock_drops_a_session_that_has_gone_quiet() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);
    assert!(
        alice.net.sessions().is_open(&bob.id),
        "no session to let go idle"
    );

    let idle = now + IDLE_TIMEOUT;
    assert_eq!(
        alice.net.tick(idle),
        vec![NetEvent::SessionClosed {
            peer: bob.id.clone()
        }],
        "the idle session was not dropped"
    );
    assert!(
        !alice.net.sessions().is_open(&bob.id),
        "the session was reported closed but is still open"
    );
    // The hang-up is the point: an idle pipe holds an LE connection slot from a
    // pool shared by every app on the phone, so dropping the keys while keeping
    // the pipe would leave the scarce half of the resource held.
    assert!(
        !alice.transport.pipes().contains(&bob.id),
        "the session went but the pipe stayed open"
    );
}

/// A session in use is not.
///
/// The bug this guards is an off-by-one in the other direction: a sweep that
/// fired a moment early would tear down live conversations, which is worse than
/// the leak it replaced.
#[test]
fn the_clock_leaves_a_live_session_alone() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);

    assert!(
        alice
            .net
            .tick(now + IDLE_TIMEOUT - Duration::from_secs(1))
            .is_empty(),
        "a session one second short of the timeout was dropped"
    );
    assert!(alice.net.sessions().is_open(&bob.id));
}

/// The advertised id rotates on the clock, which is the whole of R0-F2.
#[test]
fn the_clock_rotates_the_advertised_id() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);

    alice.net.discovery().set_enabled(true, now).unwrap();
    let before = alice.net.discovery().local_id();

    alice.net.tick(now + ROTATION_PERIOD);

    assert_ne!(
        alice.net.discovery().local_id(),
        before,
        "the id did not rotate — an observer can link across the whole session"
    );
}

/// …but not out from under a conversation still in progress (T08 rule 4).
///
/// Rotating under an open pipe would break the tie-break both sides already
/// agreed on. The engine must not treat that refusal as an error.
///
/// The session has to be *fresh* for this to test anything: a rotation comes
/// due at twelve minutes and a sweep at five, so a pair that has sat still
/// since the start gets hung up on first and then rotates quite correctly.
/// Pinging a minute before the rotation is what makes the pipe genuinely busy
/// at the moment it comes due — and the first draft of this test, without that,
/// asserted the opposite of what it meant and failed.
#[test]
fn the_clock_does_not_rotate_out_from_under_a_busy_pipe() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);
    let bob = node(&air, "bob", "Bob", now);

    bob.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().set_enabled(true, now).unwrap();
    alice.net.discovery().start_scanning().unwrap();
    settle(&alice, &bob, now);
    alice.net.reach(&bob.id).unwrap();
    settle(&alice, &bob, now);
    let before = alice.net.discovery().local_id();

    let busy = now + ROTATION_PERIOD - Duration::from_secs(60);
    alice.net.ping(&bob.id, busy).unwrap();
    settle(&alice, &bob, busy);

    // Due, but refused — and the refusal must not surface as a failure.
    let due = now + ROTATION_PERIOD;
    assert!(
        alice.net.tick(due).is_empty(),
        "a session used a minute ago was swept"
    );
    assert!(
        alice.transport.pipes().contains(&bob.id),
        "the pipe this test needs open was closed"
    );
    assert_eq!(
        alice.net.discovery().local_id(),
        before,
        "the id rotated while a pipe was open"
    );
}

/// A rung that reports the same peer over and over must not rebuild the screen
/// each time.
///
/// mDNS resolves a service once per interface and per address, which on this
/// machine meant sixteen `PeerFound` events for one peer inside a single second
/// (measured in `transport::lan_re_resolves_a_peer_that_never_moved`), and then
/// another every hundred seconds for as long as it stays put. Each one used to
/// send a fresh device list over the bridge and rebuild the list, every time
/// with identical contents.
///
/// The first sighting is news. The fifteen behind it are not.
#[test]
fn a_repeated_sighting_does_not_keep_telling_the_ui() {
    let air = air();
    let now = Instant::now();
    let alice = node(&air, "alice", "Alice", now);

    let sighting = || TransportEvent::PeerFound {
        peer: "bob".to_string(),
        payload: Vec::new(),
    };
    let lost = || TransportEvent::PeerLost {
        peer: "bob".to_string(),
    };

    assert_eq!(
        alice.net.handle(sighting(), now),
        vec![NetEvent::PeersChanged],
        "the first sighting of a peer has to reach the UI"
    );
    for i in 0..15 {
        assert!(
            alice.net.handle(sighting(), now).is_empty(),
            "re-sighting {i} of a peer already listed rebuilt the list again"
        );
    }

    // Going away is news again — the guard must not swallow real changes,
    // which would leave someone on screen after they left.
    assert_eq!(
        alice.net.handle(lost(), now),
        vec![NetEvent::PeersChanged],
        "a peer going away did not reach the UI"
    );
    assert!(
        alice.net.handle(lost(), now).is_empty(),
        "losing a peer that had already gone reported a change that did not happen"
    );

    // And it can come back.
    assert_eq!(
        alice.net.handle(sighting(), now),
        vec![NetEvent::PeersChanged],
        "a peer returning after being lost did not reach the UI"
    );
}

/// A rung that remembers what it was asked to check on.
struct RecordsVerifies {
    inner: Arc<dyn Transport>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl RecordsVerifies {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn forget(&self) {
        self.asked.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

impl Transport for RecordsVerifies {
    fn verify_peer(&self, peer: &str) {
        self.asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(peer.to_string());
        self.inner.verify_peer(peer);
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
    fn set_local_id(&self, id: &str) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.set_local_id(id)
    }
    fn start_advertising(
        &self,
        payload: Vec<u8>,
    ) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.start_advertising(payload)
    }
    fn stop_advertising(&self) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.stop_advertising()
    }
    fn start_scanning(&self) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.start_scanning()
    }
    fn stop_scanning(&self) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.stop_scanning()
    }
    fn connect(&self, peer: &str) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.connect(peer)
    }
    fn send(
        &self,
        peer: &str,
        bytes: &[u8],
    ) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.send(peer, bytes)
    }
    fn disconnect(&self, peer: &str) -> Result<(), rust_lib_hoppler::transport::TransportError> {
        self.inner.disconnect(peer)
    }
    fn peers(&self) -> Vec<String> {
        self.inner.peers()
    }
    fn pipes(&self) -> Vec<String> {
        self.inner.pipes()
    }
    fn shutdown(&self) {
        self.inner.shutdown()
    }
}

/// The clock asks the rung about peers it has stopped hearing about.
///
/// A peer that leaves without a goodbye sends nothing, so the rung learns by
/// asking or by waiting — and waiting is what left someone on screen for two
/// minutes on LAN against fifteen seconds on BLE (§5.0.21). The engine cannot
/// decide the peer has gone; it can only ask, and let the rung answer with
/// `PeerLost` the way it always does.
#[test]
fn the_clock_asks_about_a_peer_that_has_gone_quiet() {
    let air = air();
    let now = Instant::now();
    let (tx, _rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rung = Arc::new(RecordsVerifies {
        inner: Arc::new(air.join("alice", sink)),
        asked: asked.clone(),
    });
    let identity = Arc::new(Mutex::new(Identity::generate("Alice", 0x00_88_ff)));
    let net = Net::new(rung.clone(), identity, "alice", now);
    net.discovery().set_enabled(true, now).unwrap();

    let sighted = |t| {
        net.handle(
            TransportEvent::PeerFound {
                peer: "bob".to_string(),
                payload: Vec::new(),
            },
            t,
        )
    };
    sighted(now);

    // Recently seen: there is nothing to ask about.
    net.tick(now + Duration::from_secs(10));
    assert!(
        rung.asked().is_empty(),
        "asked about a peer the rung had just reported: {:?}",
        rung.asked()
    );

    // Gone quiet: worth a question.
    net.tick(now + Duration::from_secs(45));
    assert_eq!(
        rung.asked(),
        vec!["bob".to_string()],
        "a peer unheard-from for 45s was never asked about"
    );

    // A sighting is an answer. This is what stops the asking being endless:
    // on a rung that re-resolves rarely, every peer would otherwise be quiet
    // most of the time and be probed on every single tick.
    rung.forget();
    sighted(now + Duration::from_secs(50));
    net.tick(now + Duration::from_secs(60));
    assert!(
        rung.asked().is_empty(),
        "kept asking about a peer that had just answered: {:?}",
        rung.asked()
    );

    // Discovery off: no list, nobody looking, no reason to be on the air.
    rung.forget();
    net.discovery()
        .set_enabled(false, now + Duration::from_secs(61))
        .unwrap();
    net.tick(now + Duration::from_secs(200));
    assert!(
        rung.asked().is_empty(),
        "probed the neighbourhood with Discovery off: {:?}",
        rung.asked()
    );
}
