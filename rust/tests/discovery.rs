//! Discovery service tests (T09) — R0-F2, and the endpoint shape R0-F10 rests
//! on.
//!
//! The load-bearing one is [`the_null_response_is_and_stays_nothing`]. The task
//! notes that indistinguishability is fragile under refactoring and asks for it
//! to be pinned with bytes rather than a comment, so it is checked as a golden
//! value and across every refusal path at once — a future branch that decided
//! to be helpful on one of them fails here rather than in a privacy report.

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_lib_hoppler::discovery::protocol::{Request, Response, PSEUDONYM_LEN, REQUEST_LEN};
use rust_lib_hoppler::discovery::{Discovery, ROTATION_PERIOD};
use rust_lib_hoppler::identity::Identity;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportEvent};

// ── harness ─────────────────────────────────────────────────────────────────

type Events = Receiver<TransportEvent>;

fn recorder() -> (Box<dyn Fn(TransportEvent) + Send + Sync>, Events) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    (
        Box::new(move |e| {
            let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
        }),
        rx,
    )
}

fn wait_for(rx: &Events, pred: impl Fn(&TransportEvent) -> bool) -> TransportEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out waiting for an event");
        match rx.recv_timeout(left) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => continue,
            Err(e) => panic!("no event: {e}"),
        }
    }
}

/// Drain everything currently queued, so a later assertion about what arrives
/// cannot pass on history.
fn drain(rx: &Events) {
    while rx.try_recv().is_ok() {}
}

/// Feed a transport's events into a Discovery until it goes quiet.
fn pump(d: &Discovery, rx: &Events, now: Instant) {
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
        d.on_event(event, now);
    }
}

struct Node {
    discovery: Discovery,
    transport: Arc<dyn Transport>,
    rx: Events,
    id: String,
}

fn node(net: &LoopbackNet, id: &str, now: Instant) -> Node {
    let (sink, rx) = recorder();
    let transport: Arc<dyn Transport> = Arc::new(net.join(id, sink));
    let identity = Arc::new(Mutex::new(Identity::generate(
        format!("{id}-persona"),
        0x00ff00,
    )));
    Node {
        discovery: Discovery::new(transport.clone(), identity, now),
        transport,
        rx,
        id: id.to_string(),
    }
}

/// Everything a requester gets back after asking, as raw bytes.
fn ask(asker: &Node, responder: &Node, request: &Request, now: Instant) -> Vec<u8> {
    drain(&asker.rx);
    // Not `.ok()`: a failed send also produces an empty reply, so swallowing it
    // would let every silence assertion pass without the responder ever having
    // been asked anything.
    asker
        .transport
        .send(&responder.id, &request.encode())
        .expect("the request never reached the responder");
    pump(&responder.discovery, &responder.rx, now);
    let mut got = Vec::new();
    while let Ok(event) = asker.rx.recv_timeout(Duration::from_millis(150)) {
        if let TransportEvent::Received { bytes, .. } = event {
            got.extend_from_slice(&bytes);
        }
    }
    got
}

fn connected(net: &LoopbackNet, a: &Node, b: &Node) {
    a.transport.connect(&b.id).unwrap();
    wait_for(&a.rx, |e| matches!(e, TransportEvent::PipeOpened { .. }));
    wait_for(&b.rx, |e| matches!(e, TransportEvent::PipeOpened { .. }));
    let _ = net;
}

// ── the null response ───────────────────────────────────────────────────────

/// The property the whole endpoint shape exists to produce: a blocked
/// requester, a requester arriving while Discovery is off, and one that has
/// asked too often all observe *exactly* the same thing, and that thing is
/// nothing.
///
/// Checked as bytes rather than as behaviour because the failure mode is a
/// well-meaning refactor — an "unavailable" frame, an error code, a distinct
/// close — that keeps every other test green.
#[test]
fn the_null_response_is_and_stays_nothing() {
    // Golden: silence is the empty encoding, now and after any refactor.
    assert_eq!(
        Response::Silence.encode(),
        Vec::<u8>::new(),
        "the null response gained bytes — a requester can now distinguish \
         'refused' from 'not there', which is the F10 property"
    );

    let net = LoopbackNet::new();
    let now = Instant::now();
    let asker = node(&net, "asker", now);
    let responder = node(&net, "responder", now);
    connected(&net, &asker, &responder);

    let blocked_pseudonym = [7u8; PSEUDONYM_LEN];
    let stranger = [9u8; PSEUDONYM_LEN];

    // 1. Discovery off.
    let off = ask(&asker, &responder, &Request::new(stranger), now);

    // 2. Discovery on, but the requester is blocked.
    responder.discovery.set_enabled(true, now).unwrap();
    responder.discovery.block(blocked_pseudonym);
    let blocked = ask(&asker, &responder, &Request::new(blocked_pseudonym), now);

    // 3. Discovery on, not blocked, but over its allowance.
    let flooder = [11u8; PSEUDONYM_LEN];
    for _ in 0..10 {
        ask(&asker, &responder, &Request::new(flooder), now);
    }
    let limited = ask(&asker, &responder, &Request::new(flooder), now);

    // 4. A frame we could not parse at all.
    drain(&asker.rx);
    asker.transport.send(&responder.id, b"junk").unwrap();
    asker
        .transport
        .send(&responder.id, &vec![0xff; REQUEST_LEN])
        .unwrap();
    pump(&responder.discovery, &responder.rx, now);
    let mut malformed = Vec::new();
    while let Ok(TransportEvent::Received { bytes, .. }) =
        asker.rx.recv_timeout(Duration::from_millis(150))
    {
        malformed.extend_from_slice(&bytes);
    }

    for (what, bytes) in [
        ("discovery off", &off),
        ("blocked", &blocked),
        ("rate limited", &limited),
        ("malformed", &malformed),
    ] {
        assert_eq!(
            bytes,
            &Vec::<u8>::new(),
            "{what}: the responder said something. Every refusal must be \
             byte-identical, or the reason leaks."
        );
    }

    // And the control: a permitted requester really does get a record, so the
    // assertions above are not all passing because nothing works at all.
    let served = ask(&asker, &responder, &Request::new(stranger), now);
    assert!(
        !served.is_empty(),
        "a permitted requester got silence too — this test proves nothing"
    );
}

#[test]
fn a_persona_is_only_disclosed_after_the_requester_identifies_itself() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let asker = node(&net, "asker", now);
    let responder = node(&net, "responder", now);
    connected(&net, &asker, &responder);
    responder.discovery.set_enabled(true, now).unwrap();

    // A partial request must not be answered: the responder speaks only after a
    // whole pseudonym has landed (tech spec §4, requester-first).
    drain(&asker.rx);
    asker
        .transport
        .send(&responder.id, &Request::first_contact().encode()[..10])
        .unwrap();
    pump(&responder.discovery, &responder.rx, now);
    assert!(
        asker.rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "the responder answered a partial request"
    );

    // The rest of it arrives and now it answers.
    asker
        .transport
        .send(&responder.id, &Request::first_contact().encode()[10..])
        .unwrap();
    pump(&responder.discovery, &responder.rx, now);
    let mut got = Vec::new();
    while let Ok(TransportEvent::Received { bytes, .. }) =
        asker.rx.recv_timeout(Duration::from_millis(150))
    {
        got.extend_from_slice(&bytes);
    }
    assert!(
        matches!(Response::decode(&got), Ok(Some(Response::Persona(_)))),
        "a reassembled request was not answered: {got:?}"
    );
}

// ── rotation ────────────────────────────────────────────────────────────────

/// A rotation must not be linkable. The sighting list is keyed by the rung's
/// id, so a rotated peer arrives as a new device — and the old entry goes.
#[test]
fn the_sighting_list_does_not_carry_a_peer_across_a_rotation() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);
    let subject = node(&net, "subject", now);

    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);

    let before: Vec<String> = watcher
        .discovery
        .sightings()
        .into_iter()
        .map(|s| s.peer)
        .collect();
    assert_eq!(before.len(), 1, "expected one sighting, got {before:?}");

    subject.discovery.rotate(now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);

    let after: Vec<String> = watcher
        .discovery
        .sightings()
        .into_iter()
        .map(|s| s.peer)
        .collect();
    assert_eq!(after.len(), 1, "expected one sighting, got {after:?}");
    assert_ne!(
        before[0], after[0],
        "the peer kept its id across a rotation — an observer can link it"
    );
}

#[test]
fn rotation_is_due_on_the_specified_cadence_and_not_before() {
    let net = LoopbackNet::new();
    let start = Instant::now();
    let subject = node(&net, "subject", start);
    let watcher = node(&net, "watcher", start);
    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, start).unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    let first = watcher.discovery.sightings()[0].peer.clone();

    // Just short of the period: nothing moves.
    subject
        .discovery
        .tick(start + ROTATION_PERIOD - Duration::from_secs(1))
        .unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    assert_eq!(
        watcher.discovery.sightings()[0].peer,
        first,
        "rotated early — the cadence is what aligns us with the radio's own \
         address rotation, so drifting off it re-links the two"
    );

    subject.discovery.tick(start + ROTATION_PERIOD).unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    assert_ne!(
        watcher.discovery.sightings()[0].peer,
        first,
        "did not rotate when due"
    );
}

/// A rotation is refused while a pipe is open (T08 rule 4). That is a normal
/// state, not a failure, and it must not stall the schedule for ever.
#[test]
fn a_rotation_blocked_by_an_open_pipe_is_retried_not_abandoned() {
    let net = LoopbackNet::new();
    let start = Instant::now();
    let subject = node(&net, "subject", start);
    let peer = node(&net, "peer", start);
    let watcher = node(&net, "watcher", start);

    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, start).unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    let first = watcher.discovery.sightings()[0].peer.clone();

    connected(&net, &subject, &peer);
    subject.discovery.tick(start + ROTATION_PERIOD).unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    assert_eq!(
        watcher.discovery.sightings()[0].peer,
        first,
        "rotated while a pipe was open, which renames a connected peer"
    );

    // Hang up, tick again: the rotation that was deferred now happens.
    subject.transport.disconnect(&peer.id).unwrap();
    subject
        .discovery
        .tick(start + ROTATION_PERIOD + Duration::from_secs(1))
        .unwrap();
    pump(&watcher.discovery, &watcher.rx, start);
    assert_ne!(
        watcher.discovery.sightings()[0].peer,
        first,
        "a deferred rotation never happened — the id is now pinned for as long \
         as the device keeps talking to anyone"
    );
}

// ── the toggle ──────────────────────────────────────────────────────────────

#[test]
fn turning_discovery_off_stops_advertising_and_the_endpoint_together() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);
    let subject = node(&net, "subject", now);
    let asker = node(&net, "asker", now);

    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);
    assert_eq!(watcher.discovery.sightings().len(), 1);

    connected(&net, &asker, &subject);
    assert!(
        !ask(&asker, &subject, &Request::new([3u8; PSEUDONYM_LEN]), now).is_empty(),
        "the endpoint was not answering while Discovery was on"
    );

    subject.discovery.set_enabled(false, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);

    assert!(
        watcher.discovery.sightings().is_empty(),
        "still advertising after the toggle went off"
    );
    assert!(
        ask(&asker, &subject, &Request::new([3u8; PSEUDONYM_LEN]), now).is_empty(),
        "the endpoint still answered after the toggle went off — both halves \
         must stop, or 'invisible' only covers the advertisement"
    );
}

// ── persona verification ────────────────────────────────────────────────────

#[test]
fn a_persona_that_fails_its_signature_never_reaches_a_sighting() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);
    let subject = node(&net, "subject", now);
    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);
    let peer = watcher.discovery.sightings()[0].peer.clone();

    let genuine = Identity::generate("real", 1).persona_record();

    // Every way a record can be wrong, not a sample: the UI's guarantee is that
    // nothing unverified is displayed, so one unchecked path is the whole
    // guarantee gone.
    let mut flipped = genuine.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    for (what, bad) in [
        ("empty", Vec::new()),
        ("junk", b"not a record at all".to_vec()),
        ("truncated", genuine[..genuine.len() / 2].to_vec()),
        ("flipped signature bit", flipped),
    ] {
        assert!(
            watcher.discovery.accept_persona(&peer, &bad).is_err(),
            "{what}: accepted"
        );
        assert!(
            watcher.discovery.sightings()[0].persona.is_none(),
            "{what}: reached the sighting anyway"
        );
    }

    watcher.discovery.accept_persona(&peer, &genuine).unwrap();
    assert!(
        watcher.discovery.sightings()[0].persona.is_some(),
        "a genuine record was rejected — the checks above prove nothing"
    );
}

// ── rate limiting ───────────────────────────────────────────────────────────

#[test]
fn the_allowance_is_per_pseudonym_and_recovers() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let asker = node(&net, "asker", now);
    let responder = node(&net, "responder", now);
    connected(&net, &asker, &responder);
    responder.discovery.set_enabled(true, now).unwrap();

    let noisy = [1u8; PSEUDONYM_LEN];
    let quiet = [2u8; PSEUDONYM_LEN];

    let mut served = 0;
    for _ in 0..20 {
        if !ask(&asker, &responder, &Request::new(noisy), now).is_empty() {
            served += 1;
        }
    }
    assert!(
        (1..20).contains(&served),
        "expected the allowance to cut in, served {served}/20"
    );

    // One pseudonym exhausting itself must not silence another — otherwise a
    // single noisy stranger takes Discovery down for the room.
    assert!(
        !ask(&asker, &responder, &Request::new(quiet), now).is_empty(),
        "a different pseudonym was refused because of someone else's traffic"
    );

    // And the window reopens rather than banning for the session.
    let later = now + Duration::from_secs(120);
    assert!(
        !ask(&asker, &responder, &Request::new(noisy), later).is_empty(),
        "the allowance never recovered"
    );
}

/// `PeerFound` is re-sent whenever the advertised payload changes, so it fires
/// repeatedly for a peer we already know. A verified persona must survive that
/// — otherwise names blink out of the UI for a reason no user could explain.
#[test]
fn a_repeat_sighting_does_not_discard_a_verified_persona() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);
    let subject = node(&net, "subject", now);
    watcher.discovery.start_scanning().unwrap();
    subject.discovery.set_enabled(true, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);

    let peer = watcher.discovery.sightings()[0].peer.clone();
    let record = Identity::generate("real", 1).persona_record();
    watcher.discovery.accept_persona(&peer, &record).unwrap();
    assert!(watcher.discovery.sightings()[0].persona.is_some());

    // The same peer advertises again — a new payload, the same id.
    watcher.discovery.on_event(
        TransportEvent::PeerFound {
            peer: peer.clone(),
            payload: b"changed".to_vec(),
        },
        now,
    );
    assert!(
        watcher.discovery.sightings()[0].persona.is_some(),
        "a repeat sighting cleared a persona that had already been verified"
    );
}

/// A device that is answering nobody must not spend anybody's allowance.
/// Otherwise requests arriving while Discovery is off suppress a requester's
/// first legitimate ask moments after it comes back on — and the bucket map
/// grows for a device that is not talking to anyone.
#[test]
fn requests_while_discovery_is_off_do_not_consume_the_allowance() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let asker = node(&net, "asker", now);
    let responder = node(&net, "responder", now);
    connected(&net, &asker, &responder);

    // Well past the burst, all while off.
    let who = [21u8; PSEUDONYM_LEN];
    for _ in 0..50 {
        assert!(ask(&asker, &responder, &Request::new(who), now).is_empty());
    }

    // Now open up: the very first ask must be served.
    responder.discovery.set_enabled(true, now).unwrap();
    assert!(
        !ask(&asker, &responder, &Request::new(who), now).is_empty(),
        "an allowance spent while Discovery was off suppressed the first real \
         request after it came back on"
    );
}

/// `Response` is public, so a record too large for the 2-byte length prefix is
/// reachable from outside the module. It must refuse rather than emit a frame
/// whose length field has silently wrapped.
#[test]
fn an_oversized_record_is_refused_rather_than_truncated_on_the_wire() {
    let huge = Response::Persona(vec![0u8; 70_000]);
    assert_eq!(
        huge.encode(),
        Vec::<u8>::new(),
        "an oversized record was encoded — its length field wrapped, and the \
         peer would parse a frame that means something else entirely"
    );
    // The bound is the protocol's, not u16's: anything above it is refused.
    let over = Response::Persona(vec![0u8; 4097]);
    assert!(over.encode().is_empty());
    let ok = Response::Persona(vec![0u8; 4096]);
    assert!(!ok.encode().is_empty(), "a legal record was refused");
}
