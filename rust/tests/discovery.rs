//! Discovery service tests (T09) — R0-F2, and the endpoint shape R0-F10 rests
//! on.
//!
//! The load-bearing one is [`the_null_response_is_and_stays_nothing`]. The task
//! notes that indistinguishability is fragile under refactoring and asks for it
//! to be pinned with bytes rather than a comment, so it is checked as a golden
//! value and across every refusal path at once — a future branch that decided
//! to be helpful on one of them fails here rather than in a privacy report.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_lib_hoppler::block::Blocklist;
use rust_lib_hoppler::discovery::protocol::{Request, Response, PSEUDONYM_LEN, REQUEST_LEN};
use rust_lib_hoppler::discovery::{hint, Discovery, ROTATION_PERIOD};
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

/// Feed a transport's events into a Discovery until it goes quiet.
///
/// Also the reason there is no separate `drain`: this leaves the queue empty,
/// so an assertion after it cannot pass on events from before it.
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
    /// Kept so the hint tests can ask what a friend who had paired with this
    /// node would know — the Layer-1 public key is the whole of it.
    identity: Arc<Mutex<Identity>>,
    /// The wall clock this node's hints are computed against, in ms. Shared
    /// with the `Discovery`, so a test moves time by writing to it.
    clock: Arc<Mutex<i64>>,
    /// The same list the `Discovery` enforces — one object, not a copy. A test
    /// blocks somebody by writing here, which is what `engine::install` does.
    blocked: Arc<Blocklist>,
}

impl Node {
    fn l1(&self) -> [u8; 32] {
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layer1_public()
            .0
    }

    fn set_clock(&self, ms: i64) {
        *self.clock.lock().unwrap_or_else(|e| e.into_inner()) = ms;
    }
}

fn node(net: &LoopbackNet, id: &str, now: Instant) -> Node {
    let (sink, rx) = recorder();
    let transport: Arc<dyn Transport> = Arc::new(net.join(id, sink));
    let identity = Arc::new(Mutex::new(Identity::generate(
        format!("{id}-persona"),
        0x00ff00,
    )));
    let clock = Arc::new(Mutex::new(EPOCH_ZERO));
    let reads = clock.clone();
    let blocked = Arc::new(Blocklist::default());
    let discovery = Discovery::with_clock(
        transport.clone(),
        identity.clone(),
        blocked.clone(),
        now,
        Box::new(move || *reads.lock().unwrap_or_else(|e| e.into_inner())),
    );
    // What `Net::new` does in production: discovery is built after the
    // transport, so it has to be told the id the transport already has.
    discovery.set_local_id_for_tiebreak(id);
    Node {
        discovery,
        transport,
        rx,
        id: id.to_string(),
        identity,
        clock,
        blocked,
    }
}

/// A wall-clock instant far enough from zero that a test can step either side
/// of an epoch boundary without the arithmetic going negative by accident.
const EPOCH_ZERO: i64 = 1_000 * PERIOD_MS;
/// Derived from the real constant rather than restated. A hardcoded twelve
/// minutes would keep passing against a changed `ROTATION_PERIOD` while quietly
/// no longer testing an epoch boundary at all.
const PERIOD_MS: i64 = ROTATION_PERIOD.as_millis() as i64;

/// Everything a requester gets back after asking, as raw bytes.
///
/// Calls the responder's endpoint directly. Discovery no longer reads a raw
/// stream — the pipe carries two protocols and telling them apart by content
/// cannot be made reliable (see `engine::pipe`), so demultiplexing moved up and
/// this is now the whole entry point.
fn ask(_asker: &Node, responder: &Node, request: &Request, now: Instant) -> Vec<u8> {
    responder
        .discovery
        .answer("asker", &request.encode(), now)
        .unwrap_or_default()
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
    responder.blocked.block(blocked_pseudonym);
    let blocked = ask(&asker, &responder, &Request::new(blocked_pseudonym), now);

    // 3. Discovery on, not blocked, but over its allowance.
    let flooder = [11u8; PSEUDONYM_LEN];
    for _ in 0..10 {
        ask(&asker, &responder, &Request::new(flooder), now);
    }
    let limited = ask(&asker, &responder, &Request::new(flooder), now);

    // 4. Frames we could not parse at all.
    let mut malformed = responder
        .discovery
        .answer("asker", b"junk", now)
        .unwrap_or_default();
    malformed.extend(
        responder
            .discovery
            .answer("asker", &[0xff; REQUEST_LEN], now)
            .unwrap_or_default(),
    );

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

/// Requester-first, at the only layer that can still express it: a request that
/// is not a whole request earns silence.
///
/// Reassembly moved to `engine::pipe`, so "half a request" now means "33 bytes
/// that are not a valid request" rather than "a short read".
#[test]
fn a_partial_or_malformed_request_is_not_answered() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let responder = node(&net, "responder", now);
    responder.discovery.set_enabled(true, now).unwrap();

    for bad in [
        Vec::new(),
        b"short".to_vec(),
        Request::first_contact().encode()[..10].to_vec(),
        vec![0u8; REQUEST_LEN], // wrong version byte
    ] {
        assert!(
            responder.discovery.answer("asker", &bad, now).is_none(),
            "answered a request that was not one: {bad:?}"
        );
    }
    assert!(responder
        .discovery
        .answer("asker", &Request::first_contact().encode(), now)
        .is_some());
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

// ── the advert hint (T09a) ──────────────────────────────────────────────────

/// Every advertisement a scanner saw, in order, as (id, payload).
fn adverts(rx: &Events) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(50)) {
        if let TransportEvent::PeerFound { peer, payload } = event {
            out.push((peer, payload));
        }
    }
    out
}

/// The point of the whole feature: somebody who has paired with us can pick our
/// advertisement out of the air, with nothing dialled and no session open.
#[test]
fn a_friend_can_recognise_the_advertisement() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let watcher = node(&net, "watcher", now);
    watcher.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();

    let seen = adverts(&watcher.rx);
    let (id, payload) = seen
        .iter()
        .find(|(id, _)| id == "subject")
        .expect("the subject should have advertised");
    let carried = hint::read(payload).expect("the advert should have carried a hint");
    assert!(
        hint::written_by(&subject.l1(), id, &carried, EPOCH_ZERO),
        "a paired friend should have recognised it"
    );
}

/// And nobody else can. A scanner holding some other Layer-1 key — which is
/// every scanner that has not stood next to us and pressed confirm — gets
/// eight bytes it cannot tell from noise.
#[test]
fn a_stranger_cannot() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let stranger = node(&net, "stranger", now);
    stranger.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();

    let seen = adverts(&stranger.rx);
    let (id, payload) = seen.iter().find(|(id, _)| id == "subject").unwrap();
    let carried = hint::read(payload).unwrap();
    assert!(!hint::written_by(&stranger.l1(), id, &carried, EPOCH_ZERO));
}

/// The one that guards R0-F2, and the reason `rotate` goes quiet before it
/// renames itself.
///
/// The rung re-advertises whatever payload it was last given when the id
/// changes, so the naive order publishes the *old* hint under the *new* id —
/// eight identical bytes under two ids, which any scanner in range can link
/// with no key and no pairing at all. That is exactly the linkage the rotation
/// was performed to break, so it would have made the advert worse than empty.
///
/// Asserted over everything the scanner saw rather than the end state: the leak
/// is a moment, not a resting position, and a test that only looked at the
/// final advertisement would sail straight past it.
#[test]
fn no_hint_is_ever_seen_under_two_ids() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let watcher = node(&net, "watcher", now);
    watcher.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();
    subject.discovery.rotate(now).unwrap();

    let mut ids_per_payload: std::collections::HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    for (id, payload) in adverts(&watcher.rx) {
        if payload.is_empty() {
            continue; // carries nothing, so it links nothing
        }
        let ids = ids_per_payload.entry(payload).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    assert!(
        !ids_per_payload.is_empty(),
        "the subject should have advertised a hint at all"
    );
    for (payload, ids) in &ids_per_payload {
        assert_eq!(
            ids.len(),
            1,
            "{} was advertised under {:?} — two ids linked by one hint",
            hex::encode(payload),
            ids
        );
    }
}

/// A rotation that the rung refuses — which is what happens whenever a pipe is
/// open — must leave us advertising, not silent. `rotate` goes quiet before it
/// asks, so the refusal path has to put the hint back.
#[test]
fn a_refused_rotation_leaves_the_hint_on_the_air() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let peer = node(&net, "peer", now);
    peer.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();
    connected(&net, &subject, &peer);
    let _ = adverts(&peer.rx);

    // A pipe is open, so the rung refuses to rename (T08 rule 4).
    subject.discovery.rotate(now).unwrap();

    let last = adverts(&peer.rx)
        .into_iter()
        .rfind(|(id, _)| id == "subject")
        .expect("the subject should have re-advertised");
    let carried = hint::read(&last.1).expect("the hint should have gone back up");
    assert!(hint::written_by(
        &subject.l1(),
        &last.0,
        &carried,
        EPOCH_ZERO
    ));
}

/// The epoch grid is shared between devices; our rotation timer is our own. So
/// a hint falls due on a schedule the id knows nothing about, and a device
/// holding an open pipe — which cannot rotate at all — would otherwise go on
/// advertising a hint from epochs ago until its friends stopped recognising it.
#[test]
fn the_hint_turns_with_the_epoch_without_waiting_for_a_rotation() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let watcher = node(&net, "watcher", now);
    watcher.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();
    let first = adverts(&watcher.rx).pop().expect("an advert").1;

    // Time enough for the epoch to turn, and nowhere near enough for a
    // rotation to fall due.
    subject.set_clock(EPOCH_ZERO + PERIOD_MS);
    subject
        .discovery
        .tick(now + Duration::from_secs(1))
        .unwrap();

    let second = adverts(&watcher.rx).pop().expect("a second advert");
    assert_eq!(second.0, "subject", "the id should not have moved");
    assert_ne!(first, second.1, "the hint should have");
    let carried = hint::read(&second.1).unwrap();
    assert!(hint::written_by(
        &subject.l1(),
        &second.0,
        &carried,
        EPOCH_ZERO + PERIOD_MS
    ));
}

/// A tick that changes nothing must not touch the radio. This runs on every
/// turn of the engine loop, and re-advertising is not free — on BLE it stops
/// and restarts an advertising set.
#[test]
fn a_tick_that_changes_nothing_does_not_re_advertise() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let watcher = node(&net, "watcher", now);
    watcher.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();
    let _ = adverts(&watcher.rx);

    for second in 1..5 {
        subject
            .discovery
            .tick(now + Duration::from_secs(second))
            .unwrap();
    }

    assert!(
        adverts(&watcher.rx).is_empty(),
        "nothing had changed, so nothing should have been published"
    );
}

/// The sighting has to keep the hint, because the only thing that can resolve
/// it is the store — discovery holds no Layer-1 keys but its own.
#[test]
fn a_sighting_keeps_the_hint_it_arrived_with() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let subject = node(&net, "subject", now);
    let watcher = node(&net, "watcher", now);
    watcher.transport.start_scanning().unwrap();

    subject.discovery.set_enabled(true, now).unwrap();
    pump(&watcher.discovery, &watcher.rx, now);

    let seen = watcher
        .discovery
        .sightings()
        .into_iter()
        .find(|s| s.peer == "subject")
        .expect("the subject should have been sighted");
    let carried = seen.hint.expect("the sighting should have kept the hint");
    assert!(hint::written_by(
        &subject.l1(),
        &seen.peer,
        &carried,
        EPOCH_ZERO
    ));
}

/// mDNS resolves a peer once per interface and address — sixteen `PeerFound`
/// events inside a second, measured — so "the list changed" has to stay false
/// for a repeat, or the screen rebuilds sixteen times for one arrival. The hint
/// is the one part that legitimately moves under a settled id, and it moving is
/// news: a peer that was advertising nothing and now carries a hint stops being
/// an unknown device and becomes somebody with a name.
#[test]
fn only_a_hint_that_actually_moved_counts_as_news() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);
    let bare = TransportEvent::PeerFound {
        peer: "stranger".into(),
        payload: Vec::new(),
    };
    let with_hint = TransportEvent::PeerFound {
        peer: "stranger".into(),
        payload: vec![7u8; 8],
    };

    assert!(watcher.discovery.on_event(bare.clone(), now), "first sight");
    assert!(
        !watcher.discovery.on_event(bare.clone(), now),
        "a re-resolve"
    );
    assert!(
        watcher.discovery.on_event(with_hint.clone(), now),
        "a hint where there was none changes who the row is"
    );
    assert!(
        !watcher.discovery.on_event(with_hint, now),
        "the same hint again does not"
    );
    assert!(
        !watcher.discovery.on_event(bare, now),
        "an advertisement carrying no hint takes nothing away, so it is not news"
    );
}

/// A hint already learned survives an advertisement that carries none.
///
/// Our own rotation produces exactly that event. `rotate` goes quiet before it
/// renames itself — an empty payload under the *old* id — so every peer
/// watching sees that id lose its hint. If the sighting takes that literally,
/// the peer becomes unresolvable the moment it rotates, and stays that way for
/// as long as the rung keeps the old service around: on mDNS a withdrawal is
/// not prompt, so "for as long as" can be minutes. What the user sees is the
/// duplicate row T09a exists to prevent, back again, some time after it
/// worked.
///
/// Keeping the last hint is also the more honest reading. A hint identifies its
/// peer for its epoch window whether or not the next advertisement repeats it,
/// and nobody who cannot compute it can put one there — so there is nothing to
/// be gained by forgetting one we have already been given.
#[test]
fn a_hint_survives_an_advertisement_that_carries_none() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);

    let carrying = TransportEvent::PeerFound {
        peer: "friend".into(),
        payload: vec![7u8; 8],
    };
    let gone_quiet = TransportEvent::PeerFound {
        peer: "friend".into(),
        payload: Vec::new(),
    };

    watcher.discovery.on_event(carrying, now);
    watcher.discovery.on_event(gone_quiet, now);

    let seen = watcher
        .discovery
        .sightings()
        .into_iter()
        .find(|s| s.peer == "friend")
        .expect("the friend should still be sighted");
    assert_eq!(
        seen.hint,
        Some([7u8; 8]),
        "going quiet before a rotation made a paired friend unrecognisable"
    );
}

/// A *different* hint still replaces the old one — the epoch turns, and the
/// sighting has to follow. Only the empty case is remembered.
#[test]
fn a_new_hint_still_replaces_the_one_before_it() {
    let net = LoopbackNet::new();
    let now = Instant::now();
    let watcher = node(&net, "watcher", now);

    for payload in [vec![1u8; 8], vec![2u8; 8]] {
        watcher.discovery.on_event(
            TransportEvent::PeerFound {
                peer: "friend".into(),
                payload,
            },
            now,
        );
    }
    let seen = watcher
        .discovery
        .sightings()
        .into_iter()
        .find(|s| s.peer == "friend")
        .unwrap();
    assert_eq!(seen.hint, Some([2u8; 8]));
}
