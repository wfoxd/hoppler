//! Transport-layer tests (T08).
//!
//! The heart of this file is [`conformance`] — one behavioural suite run
//! against *every* rung, because the trait's whole claim is that the session
//! layer above cannot tell them apart. A rung that passes it is a drop-in for
//! the next one, and the BLE adapter (T08b) can call it directly.
//!
//! Rung-specific tests live below the harness and cover only what is genuinely
//! particular to a rung (mDNS multicast, TCP reassembly, throughput).

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_lib_hoppler::transport::lan::LanTransport;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportError, TransportEvent};

// ── harness plumbing ────────────────────────────────────────────────────────

type Events = Receiver<TransportEvent>;

fn recorder() -> (Box<dyn Fn(TransportEvent) + Send + Sync>, Events) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    (sink, rx)
}

/// Wait for the first event matching `pred`, ignoring others.
fn wait_for(rx: &Events, pred: impl Fn(&TransportEvent) -> bool) -> TransportEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
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

fn opened(rx: &Events, peer: &str) {
    wait_for(
        rx,
        |e| matches!(e, TransportEvent::PipeOpened { peer: p } if p == peer),
    );
}

fn closed(rx: &Events, peer: &str) {
    wait_for(
        rx,
        |e| matches!(e, TransportEvent::PipeClosed { peer: p } if p == peer),
    );
}

/// Collect `want` bytes from `Received` events, however the rung chose to
/// chunk them.
fn received_bytes(rx: &Events, want: usize) -> Vec<u8> {
    let mut got = Vec::new();
    while got.len() < want {
        match wait_for(rx, |e| matches!(e, TransportEvent::Received { .. })) {
            TransportEvent::Received { bytes, .. } => got.extend_from_slice(&bytes),
            _ => unreachable!(),
        }
    }
    got
}

/// A pair of nodes on one rung, plus their event streams.
struct Pair {
    a: Box<dyn Transport>,
    b: Box<dyn Transport>,
    rx_a: Events,
    rx_b: Events,
    id_a: String,
    id_b: String,
    /// Whether this rung's discovery can actually run here. LAN discovery needs
    /// multicast, which container CI lacks — the *behavioural* rules are still
    /// checked, with the address seeded instead of discovered.
    discovery_works: bool,
}

// ── the conformance suite ───────────────────────────────────────────────────

/// Every rule from the module-level contract, checked against one rung.
fn conformance(p: Pair) {
    let Pair {
        a,
        b,
        rx_a,
        rx_b,
        id_a,
        id_b,
        discovery_works,
    } = p;

    // Rule: limits are reported and sane.
    assert!(a.limits().max_advertising_payload > 0);
    assert!(a.limits().preferred_write_size > 0);
    assert!(a.is_available());

    // Rule: an advertiser is discovered, with its payload.
    b.start_advertising(b"beacon".to_vec()).unwrap();
    a.start_scanning().unwrap();
    if discovery_works {
        let found = wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == &id_b),
        );
        let TransportEvent::PeerFound { payload, .. } = found else {
            unreachable!()
        };
        assert_eq!(
            payload,
            b"beacon",
            "{}: payload must survive discovery",
            a.name()
        );
        assert!(
            a.peers().contains(&id_b),
            "{}: peers() must list the sighting",
            a.name()
        );
    }

    // Rule: an oversized advertisement is refused with the right error.
    let too_big = vec![0u8; a.limits().max_advertising_payload + 1];
    assert!(
        matches!(
            a.start_advertising(too_big),
            Err(TransportError::PayloadTooLarge { .. })
        ),
        "{}: oversized payload must be PayloadTooLarge",
        a.name()
    );

    // Rule: connect is accepted, and the pipe is usable only after PipeOpened —
    // which both ends receive.
    a.connect(&id_b).unwrap();
    opened(&rx_a, &id_b);
    opened(&rx_b, &id_a);
    assert!(
        a.pipes().contains(&id_b),
        "{}: pipes() must list the open pipe",
        a.name()
    );

    // Rule: bytes flow both ways, reassembling byte-exact regardless of how the
    // rung chunked them.
    let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    a.send(&id_b, &payload).unwrap();
    assert_eq!(
        received_bytes(&rx_b, payload.len()),
        payload,
        "{}: a→b",
        a.name()
    );
    b.send(&id_a, b"pong").unwrap();
    assert_eq!(received_bytes(&rx_a, 4), b"pong", "{}: b→a", a.name());

    // Rule: a peer that dialled us is dialable back without re-discovery.
    b.connect(&id_a).unwrap();
    opened(&rx_b, &id_a);

    // Rule: connecting an already-open pipe still announces it, so a caller
    // awaiting PipeOpened never hangs.
    a.connect(&id_b).unwrap();
    opened(&rx_a, &id_b);

    // Rule: the local id cannot rotate while a pipe is open — a connected peer
    // knows us by the id it dialled (rule 4).
    assert!(
        a.set_local_id("rotated-while-open").is_err(),
        "{}: rotation must be refused while a pipe is open",
        a.name()
    );

    // Rule: disconnect closes both ends and the pipe stops working.
    a.disconnect(&id_b).unwrap();
    closed(&rx_a, &id_b);
    closed(&rx_b, &id_a);
    assert!(!a.pipes().contains(&id_b));
    assert!(
        matches!(a.send(&id_b, b"x"), Err(TransportError::NoSuchPeer(_))),
        "{}: send after close must fail",
        a.name()
    );

    // Rule: dialling an unknown peer fails, and says so on the event stream.
    assert!(a.connect("ghost").is_err());
    wait_for(
        &rx_a,
        |e| matches!(e, TransportEvent::PipeFailed { peer, .. } if peer == "ghost"),
    );

    // Rule: stop_advertising makes us undiscoverable, and scanners hear about it.
    b.stop_advertising().unwrap();
    if discovery_works {
        wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerLost { peer } if peer == &id_b),
        );
    }

    // Rule: after shutdown the rung is silent and inert; shutdown is idempotent.
    a.shutdown();
    a.shutdown();
    if let Ok(e) = rx_a.recv_timeout(Duration::from_millis(300)) {
        panic!("{}: event after shutdown: {e:?}", a.name());
    }
    assert!(a.send(&id_b, b"x").is_err());
}

#[test]
fn loopback_meets_the_contract() {
    let net = LoopbackNet::new();
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    conformance(Pair {
        a: Box::new(net.join("lb-a", sink_a)),
        b: Box::new(net.join("lb-b", sink_b)),
        rx_a,
        rx_b,
        id_a: "lb-a".into(),
        id_b: "lb-b".into(),
        discovery_works: true,
    });
}

#[test]
fn loopback_meets_the_contract_when_fragmenting() {
    // A rung may split sends; callers must not depend on message boundaries.
    let net = LoopbackNet::with_max_chunk(17);
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    conformance(Pair {
        a: Box::new(net.join("fr-a", sink_a)),
        b: Box::new(net.join("fr-b", sink_b)),
        rx_a,
        rx_b,
        id_a: "fr-a".into(),
        id_b: "fr-b".into(),
        discovery_works: true,
    });
}

#[test]
fn lan_meets_the_contract() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("ct-a", sink_a).unwrap();
    let b = LanTransport::new("ct-b", sink_b).unwrap();
    // Seed the address instead of discovering it: mDNS needs multicast and is
    // exercised separately, but every *behavioural* rule is checked here
    // through the same `connect` the other rungs use.
    a.add_peer_addr("ct-b", ([127, 0, 0, 1], b.port()).into());
    conformance(Pair {
        a: Box::new(a),
        b: Box::new(b),
        rx_a,
        rx_b,
        id_a: "ct-a".into(),
        id_b: "ct-b".into(),
        // mDNS needs multicast; the dedicated test below covers it locally.
        discovery_works: false,
    });
}

// ── rung-specific ───────────────────────────────────────────────────────────

/// Concurrent sends to one peer must not interleave their bytes — the framer
/// above us would desync and the failure would look like a crypto bug.
#[test]
fn send_is_atomic_under_concurrency() {
    let net = LoopbackNet::new();
    let (sink_a, _rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = Arc::new(net.join("at-a", sink_a));
    let _b = net.join("at-b", sink_b);
    a.connect("at-b").unwrap();
    opened(&rx_b, "at-a");

    // Two writers, distinct byte values, large enough to interleave if unlocked.
    const N: usize = 8 * 1024;
    let handles: Vec<_> = [1u8, 2u8]
        .into_iter()
        .map(|v| {
            let a = Arc::clone(&a);
            std::thread::spawn(move || a.send("at-b", &vec![v; N]).unwrap())
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let got = received_bytes(&rx_b, 2 * N);
    assert_eq!(got.len(), 2 * N);
    assert_eq!(
        got.iter().filter(|b| **b == 1).count(),
        N,
        "bytes lost or duplicated"
    );

    // The real assertion: each send must appear as ONE unbroken run. Counting
    // bytes alone would happily accept 1,2,1,2,… — precisely the interleaving
    // this test exists to catch — so count transitions instead. Two atomic
    // sends in either order give exactly one transition.
    let transitions = got.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(
        transitions, 1,
        "sends interleaved: {transitions} transitions, expected 1 (bytes were cut apart)"
    );
}

/// No Transport method may invoke the sink before it returns: the core calls
/// `connect` while holding its own locks, and a re-entrant event would deadlock.
#[test]
fn events_never_fire_on_the_callers_thread() {
    let net = LoopbackNet::new();
    let caller = std::thread::current().id();
    let seen_on_caller = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&seen_on_caller);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |_| {
        if std::thread::current().id() == caller {
            *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        }
    });
    let (sink_b, rx_b) = recorder();
    let a = net.join("re-a", sink);
    let b = net.join("re-b", sink_b);

    b.start_advertising(b"x".to_vec()).unwrap();
    a.start_scanning().unwrap();
    a.connect("re-b").unwrap();
    opened(&rx_b, "re-a");
    a.send("re-b", b"hello").unwrap();
    a.disconnect("re-b").unwrap();
    std::thread::sleep(Duration::from_millis(100));

    assert!(
        !*seen_on_caller.lock().unwrap_or_else(|p| p.into_inner()),
        "a sink ran on the caller's thread — the core would deadlock"
    );
}

/// Rotating the local id is how a rung delivers unlinkability (tech spec §4):
/// rotating only the payload while a stable name rides alongside would let an
/// observer link across rotations.
#[test]
fn rotating_the_local_id_is_visible_as_a_new_peer() {
    let net = LoopbackNet::new();
    let (sink_a, rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = net.join("watch", sink_a);
    let b = net.join("before", sink_b);

    a.start_scanning().unwrap();
    b.start_advertising(b"p".to_vec()).unwrap();
    wait_for(
        &rx_a,
        |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == "before"),
    );

    b.set_local_id("after").unwrap();
    wait_for(
        &rx_a,
        |e| matches!(e, TransportEvent::PeerLost { peer } if peer == "before"),
    );
    wait_for(
        &rx_a,
        |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == "after"),
    );
}

#[test]
fn lan_reassembles_a_large_payload_in_order() {
    let (sink_a, _rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("big-a", sink_a).unwrap();
    let b = LanTransport::new("big-b", sink_b).unwrap();
    a.connect_addr("big-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    opened(&rx_b, "big-a");

    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    a.send("big-b", &payload).unwrap();
    assert_eq!(received_bytes(&rx_b, payload.len()), payload);
}

/// Throughput smoke (T08 asks for ≥ 10 kB/s). Loopback TCP is far above that,
/// so this guards against a pathological regression rather than measuring a
/// radio.
#[test]
fn lan_pipe_throughput_is_sane() {
    let (sink_a, _rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("tp-a", sink_a).unwrap();
    let b = LanTransport::new("tp-b", sink_b).unwrap();
    a.connect_addr("tp-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    opened(&rx_b, "tp-a");

    let payload = vec![7u8; 1024 * 1024];
    let start = Instant::now();
    a.send("tp-b", &payload).unwrap();
    let got = received_bytes(&rx_b, payload.len());
    let rate = got.len() as f64 / start.elapsed().as_secs_f64();
    assert_eq!(got.len(), payload.len());
    assert!(
        rate > 10_000.0,
        "throughput {rate:.0} B/s below the 10 kB/s floor"
    );
}

/// A peer that connects and never identifies itself must not wedge the accept
/// loop for everyone else.
#[test]
fn lan_accept_loop_survives_a_silent_connection() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = LanTransport::new("sl-a", sink_a).unwrap();
    let b = LanTransport::new("sl-b", sink_b).unwrap();

    // Connect without sending a hello, and hold it open.
    let _silent = std::net::TcpStream::connect(("127.0.0.1", a.port())).unwrap();
    // A well-behaved peer still gets through immediately.
    b.connect_addr("sl-a", ([127, 0, 0, 1], a.port()).into())
        .unwrap();
    opened(&rx_a, "sl-b");
}

/// Real mDNS discovery. Needs working multicast on a real interface, which
/// container CI generally lacks — run locally with:
/// `cargo test --test transport -- --ignored`
#[test]
#[ignore = "needs multicast; run locally"]
fn lan_discovers_a_peer_over_mdns() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = LanTransport::new("mdns-a", sink_a).unwrap();
    let b = LanTransport::new("mdns-b", sink_b).unwrap();

    b.start_advertising(b"payload-b".to_vec()).unwrap();
    a.start_scanning().unwrap();

    let found = wait_for(
        &rx_a,
        |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == "mdns-b"),
    );
    let TransportEvent::PeerFound { payload, .. } = found else {
        unreachable!()
    };
    assert_eq!(payload, b"payload-b");

    // Discovery gave us an address: the pipe opens without a manual dial.
    a.connect("mdns-b").unwrap();
    opened(&rx_a, "mdns-b");
}

/// A peer that dialled us must be reachable afterwards — which means recording
/// the *listening* port it advertised, not the ephemeral source port the
/// kernel assigned to its outbound connection.
#[test]
fn lan_records_a_dialable_address_for_an_inbound_peer() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("db-a", sink_a).unwrap();
    let b = LanTransport::new("db-b", sink_b).unwrap();

    // b dials a; a learns about b only from the inbound connection.
    b.add_peer_addr("db-a", ([127, 0, 0, 1], a.port()).into());
    b.connect("db-a").unwrap();
    opened(&rx_a, "db-b");
    opened(&rx_b, "db-a");

    // The address a recorded must be b's listener, not b's source port.
    assert!(
        a.peer_addrs("db-b").iter().any(|s| s.port() == b.port()),
        "recorded {:?}, expected b's listening port {}",
        a.peer_addrs("db-b"),
        b.port()
    );

    // And it must actually work after the original connection is gone.
    b.disconnect("db-a").unwrap();
    closed(&rx_a, "db-b");
    a.connect("db-b").unwrap();
    opened(&rx_a, "db-b");
}

/// A handshake still in flight when shutdown begins must not leave a live pipe
/// behind — the rung is inert afterwards, not merely quiet.
#[test]
fn lan_shutdown_beats_an_in_flight_handshake() {
    let (sink_a, _rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = LanTransport::new("race-a", sink_a).unwrap();
    let b = LanTransport::new("race-b", sink_b).unwrap();
    b.add_peer_addr("race-a", ([127, 0, 0, 1], a.port()).into());

    // Dial and shut down concurrently; whichever order the two take, `a` must
    // end with no pipes.
    let dialer = std::thread::spawn(move || {
        let _ = b.connect("race-a");
        b
    });
    a.shutdown();
    let _b = dialer.join().unwrap();

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        a.pipes().is_empty(),
        "a pipe survived shutdown: {:?}",
        a.pipes()
    );
}
