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

    // Rungs must agree on what `peers()` means: a peer discovered and not since
    // lost stays known after scanning stops, because rule 5 depends on that
    // record surviving. (Hiding peers while discovery is off is a core-layer
    // decision, not a transport one.)
    if discovery_works {
        a.stop_scanning().unwrap();
        assert!(
            a.peers().contains(&id_b),
            "{}: peers() must survive stop_scanning",
            a.name()
        );
        a.start_scanning().unwrap();
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
    // Ungated on `discovery_works`: b never scanned, so this is the only check
    // that a peer which *dialled* us is known to us — the half of `peers()`
    // that rule 5's dial-back depends on.
    assert!(
        b.peers().contains(&id_a),
        "{}: a peer that dialled us must appear in peers()",
        b.name()
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

    // Rule 5: a peer that dialled us is dialable back without re-discovery.
    // b must NOT already hold the pipe here — otherwise connect takes the
    // already-open fast path and never consults its address book, and this
    // passes on a rung that records no address at all.
    b.disconnect(&id_a).unwrap();
    closed(&rx_b, &id_a);
    closed(&rx_a, &id_b);
    b.connect(&id_a).unwrap();
    opened(&rx_b, &id_a);
    opened(&rx_a, &id_b);

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

    // Rule 3: concurrent sends to one peer never interleave. Checked on every
    // rung — the LAN write mutex had no coverage while this lived in a
    // loopback-only test.
    a.connect(&id_b).unwrap();
    opened(&rx_a, &id_b);
    const CHUNK: usize = 8 * 1024;
    std::thread::scope(|scope| {
        for v in [1u8, 2u8] {
            let a = &a;
            let id_b = &id_b;
            scope.spawn(move || a.send(id_b, &vec![v; CHUNK]).unwrap());
        }
    });
    let got = received_bytes(&rx_b, 2 * CHUNK);
    let transitions = got.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(
        transitions,
        1,
        "{}: sends interleaved ({transitions} transitions, expected 1)",
        a.name()
    );
    a.disconnect(&id_b).unwrap();
    closed(&rx_a, &id_b);
    closed(&rx_b, &id_a);

    // Rule 4: with no pipe open, rotating the local id is a real rotation —
    // the old name goes and the new one arrives.
    if discovery_works {
        b.start_advertising(b"rot".to_vec()).unwrap();
        wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == &id_b),
        );
        let rotated = format!("{id_b}-rot");
        b.set_local_id(&rotated).unwrap();
        wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerLost { peer } if peer == &id_b),
        );
        wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerFound { peer, .. } if peer == &rotated),
        );
    }

    // Rule: stop_advertising makes us undiscoverable, and scanners hear about it.
    b.stop_advertising().unwrap();
    if discovery_works {
        wait_for(
            &rx_a,
            |e| matches!(e, TransportEvent::PeerLost { peer } if peer.starts_with(&id_b)),
        );
    }

    // Rule 6: after shutdown the rung is silent AND inert; shutdown is
    // idempotent, and every mutating call refuses rather than half-working.
    //
    // Drain first: anything delivered *before* shutdown is legitimate, and
    // leaving it queued would make the silence check below assert on history
    // rather than on what shutdown actually guarantees.
    while rx_a.try_recv().is_ok() {}
    a.shutdown();
    a.shutdown();
    if let Ok(e) = rx_a.recv_timeout(Duration::from_millis(300)) {
        panic!("{}: event after shutdown: {e:?}", a.name());
    }
    // Every mutating method, not a sample: this class of gap was otherwise
    // being found one method at a time.
    for (what, result) in [
        ("send", a.send(&id_b, b"x")),
        ("connect", a.connect(&id_b)),
        ("disconnect", a.disconnect(&id_b)),
        ("start_advertising", a.start_advertising(b"zombie".to_vec())),
        ("stop_advertising", a.stop_advertising()),
        ("start_scanning", a.start_scanning()),
        ("stop_scanning", a.stop_scanning()),
        ("set_local_id", a.set_local_id("after-shutdown")),
    ] {
        assert!(
            result.is_err(),
            "{}: {what} succeeded after shutdown — the rung must be inert, not merely quiet",
            a.name()
        );
    }
    assert!(
        !a.is_available(),
        "{}: is_available after shutdown",
        a.name()
    );
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
    // Seed both families: whether the listener ends up v4, v6 or dual-stack is
    // an OS-configuration detail, and the rung races candidates in production
    // for exactly this reason.
    a.add_peer_addr("ct-b", ([127, 0, 0, 1], b.port()).into());
    a.add_peer_addr("ct-b", (std::net::Ipv6Addr::LOCALHOST, b.port()).into());
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
    // Also count deliveries: asserting only "never on the caller's thread"
    // passes vacuously if the sink is never invoked at all.
    let delivered = Arc::new(Mutex::new(0usize));
    let flag = Arc::clone(&seen_on_caller);
    let count = Arc::clone(&delivered);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |_| {
        *count.lock().unwrap_or_else(|p| p.into_inner()) += 1;
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

    // Wait until events actually arrive, so the assertion below is meaningful
    // rather than passing because nothing was delivered.
    let deadline = Instant::now() + Duration::from_secs(5);
    while *delivered.lock().unwrap_or_else(|p| p.into_inner()) < 3 {
        assert!(
            Instant::now() < deadline,
            "sink received too few events to judge"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

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

/// Rotation and advertising both touch the id *and* the advertised record, so
/// they must hold both locks together and in one order. Two distinct failures
/// live here and this test has to catch both: taking the locks in opposite
/// orders deadlocks (which hangs a suite rather than failing it, hence the
/// bounded wait), and holding neither across the publish silently leaks the
/// pre-rotation name.
#[test]
fn rotating_while_advertising_does_not_deadlock_or_leak_a_name() {
    let (sink, _rx) = recorder();
    let t = std::sync::Arc::new(LanTransport::new("lock-order", sink).unwrap());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    // Several threads per job, and time-boxed rather than iteration-capped: the
    // leak needs a rotation to land entirely inside another thread's
    // read-id-then-publish window, which a two-thread lockstep rarely hits.
    const WORKERS: usize = 3;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut threads = Vec::new();
    for i in 0..WORKERS * 2 {
        let (t, stop, done_tx) = (t.clone(), stop.clone(), done_tx.clone());
        threads.push(std::thread::spawn(move || {
            let mut n = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) && Instant::now() < deadline {
                if i % 2 == 0 {
                    let _ = t.set_local_id(&format!("rot-{i}-{n}"));
                } else {
                    let _ = t.start_advertising(b"x".to_vec());
                    let _ = t.stop_advertising();
                }
                n += 1;
            }
            let _ = done_tx.send(());
        }));
    }
    drop(done_tx);

    // Every worker must report in; a missing report means one is wedged.
    for _ in 0..WORKERS * 2 {
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("rotation deadlocked against advertising");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for h in threads {
        h.join().unwrap();
    }

    // Liveness is only half of it. The race can also *succeed* wrongly: a
    // rotation that interleaves with a publish can leave the pre-rotation name
    // registered alongside the new one, which is an unlinkability failure
    // (tech spec §4) that no deadlock check would notice. Quiescent now, so
    // this is deterministic — stop advertising and nothing may remain.
    // `mdns-sd` rejects commands with `Again` when its channel backs up, which
    // this loop provokes by design; the rung correctly refuses to report a
    // withdrawal it could not perform, so retry rather than treat it as failure.
    let mut stopped = Err(TransportError::Unavailable("not attempted".into()));
    for _ in 0..50 {
        stopped = t.stop_advertising();
        if stopped.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stopped.expect("stop_advertising never drained");
    assert_eq!(
        t.registered_names(),
        Vec::<String>::new(),
        "a rotation leaked an mDNS registration: the old name still answers"
    );
    t.shutdown();
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

// ── the BLE rung ────────────────────────────────────────────────────────────
//
// BLE needs two radios, so the rung is built as a thin adapter over a seam
// (`BlePlatform`) with every contract rule decided in Rust above it. That makes
// the Rust half testable here against the same suite as every other rung: what
// `fake_radio` replaces is the radio, not the logic under test.
//
// What this cannot cover is the adapter itself — advertising actually stopping,
// L2CAP throughput, OEM quirks. Those are the two-device acceptance in
// `docs/ring0/T08-ble-adapters.md`, and nothing here should be read as
// standing in for them.

mod fake_radio {
    use super::*;
    use rust_lib_hoppler::transport::ble::{BleIngress, BlePlatform, PlatformEvent};

    #[derive(Default)]
    struct Node {
        id: String,
        advertising: Option<Vec<u8>>,
        scanning: bool,
        ingress: Option<BleIngress>,
    }

    #[derive(Default)]
    struct Air {
        nodes: Vec<Node>,
    }

    impl Air {
        fn index_of(&self, id: &str) -> Option<usize> {
            self.nodes.iter().position(|n| n.id == id)
        }
    }

    /// A shared in-process airspace standing in for the radio.
    #[derive(Clone, Default)]
    pub struct FakeRadio {
        air: Arc<Mutex<Air>>,
    }

    /// One node's view of the airspace — what the Android adapter implements.
    pub struct FakePlatform {
        air: Arc<Mutex<Air>>,
        handle: usize,
    }

    impl FakeRadio {
        pub fn new() -> Self {
            Self::default()
        }

        /// Register a node and hand back its platform. The ingress is attached
        /// afterwards, because the transport does not exist yet — the same
        /// order the real adapter is wired in.
        pub fn attach(&self, id: &str) -> Arc<FakePlatform> {
            let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
            air.nodes.push(Node {
                id: id.to_string(),
                ..Default::default()
            });
            Arc::new(FakePlatform {
                air: self.air.clone(),
                handle: air.nodes.len() - 1,
            })
        }
    }

    impl FakePlatform {
        pub fn set_ingress(&self, ingress: BleIngress) {
            let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
            air.nodes[self.handle].ingress = Some(ingress);
        }

        /// Deliver after releasing the airspace lock. Calling into a transport
        /// under the lock would put the adapter's lock beneath the core's,
        /// which is the inversion the LAN rung already paid for once.
        fn dispatch(air: &Arc<Mutex<Air>>, deliveries: Vec<(usize, PlatformEvent)>) {
            let sinks: Vec<_> = {
                let air = air.lock().unwrap_or_else(|e| e.into_inner());
                deliveries
                    .into_iter()
                    .filter_map(|(i, e)| air.nodes.get(i)?.ingress.clone().map(|g| (g, e)))
                    .collect()
            };
            for (ingress, event) in sinks {
                ingress.on_platform_event(event);
            }
        }

        fn my_id(&self) -> String {
            self.air.lock().unwrap_or_else(|e| e.into_inner()).nodes[self.handle]
                .id
                .clone()
        }
    }

    impl BlePlatform for FakePlatform {
        fn set_local_id(&self, local_id: &str) -> Result<(), TransportError> {
            self.air.lock().unwrap_or_else(|e| e.into_inner()).nodes[self.handle].id =
                local_id.to_string();
            Ok(())
        }

        fn start_advertising(&self, payload: &[u8]) -> Result<(), TransportError> {
            let deliveries = {
                let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                let local_id = air.nodes[self.handle].id.clone();
                air.nodes[self.handle].advertising = Some(payload.to_vec());
                air.nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, n)| *i != self.handle && n.scanning)
                    .map(|(i, _)| {
                        (
                            i,
                            PlatformEvent::PeerFound {
                                peer: local_id.clone(),
                                payload: payload.to_vec(),
                            },
                        )
                    })
                    .collect()
            };
            Self::dispatch(&self.air, deliveries);
            Ok(())
        }

        fn stop_advertising(&self) -> Result<(), TransportError> {
            let deliveries = {
                let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                let id = air.nodes[self.handle].id.clone();
                if air.nodes[self.handle].advertising.take().is_none() {
                    Vec::new()
                } else {
                    air.nodes
                        .iter()
                        .enumerate()
                        .filter(|(i, n)| *i != self.handle && n.scanning)
                        .map(|(i, _)| (i, PlatformEvent::PeerLost { peer: id.clone() }))
                        .collect()
                }
            };
            Self::dispatch(&self.air, deliveries);
            Ok(())
        }

        fn start_scanning(&self) -> Result<(), TransportError> {
            let deliveries = {
                let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                air.nodes[self.handle].scanning = true;
                air.nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != self.handle)
                    .filter_map(|(_, n)| {
                        n.advertising.as_ref().map(|p| {
                            (
                                self.handle,
                                PlatformEvent::PeerFound {
                                    peer: n.id.clone(),
                                    payload: p.clone(),
                                },
                            )
                        })
                    })
                    .collect()
            };
            Self::dispatch(&self.air, deliveries);
            Ok(())
        }

        fn stop_scanning(&self) -> Result<(), TransportError> {
            self.air.lock().unwrap_or_else(|e| e.into_inner()).nodes[self.handle].scanning = false;
            Ok(())
        }

        fn connect(&self, peer: &str) -> Result<(), TransportError> {
            let me = self.my_id();
            let deliveries = {
                let air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                match air.index_of(peer) {
                    Some(target) => vec![
                        (
                            self.handle,
                            PlatformEvent::PipeOpened {
                                peer: peer.to_string(),
                            },
                        ),
                        (target, PlatformEvent::PipeOpened { peer: me }),
                    ],
                    None => vec![(
                        self.handle,
                        PlatformEvent::PipeFailed {
                            peer: peer.to_string(),
                            why: "not in range".into(),
                        },
                    )],
                }
            };
            Self::dispatch(&self.air, deliveries);
            Ok(())
        }

        fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
            let me = self.my_id();
            let target = {
                let air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                air.index_of(peer)
            };
            let Some(target) = target else {
                return Err(TransportError::NoSuchPeer(peer.to_string()));
            };
            Self::dispatch(
                &self.air,
                vec![(
                    target,
                    PlatformEvent::Received {
                        peer: me,
                        bytes: bytes.to_vec(),
                    },
                )],
            );
            // A real adapter acknowledges once the radio has the bytes; doing it
            // here keeps the send window from closing during the suite.
            let ingress = {
                let air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                air.nodes[self.handle].ingress.clone()
            };
            if let Some(ingress) = ingress {
                ingress.on_write_complete(peer, bytes.len());
            }
            Ok(())
        }

        fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
            let me = self.my_id();
            let target = {
                let air = self.air.lock().unwrap_or_else(|e| e.into_inner());
                air.index_of(peer)
            };
            if let Some(target) = target {
                Self::dispatch(
                    &self.air,
                    vec![(target, PlatformEvent::PipeClosed { peer: me })],
                );
            }
            Ok(())
        }

        fn shutdown(&self) {
            let mut air = self.air.lock().unwrap_or_else(|e| e.into_inner());
            air.nodes[self.handle].advertising = None;
            air.nodes[self.handle].scanning = false;
        }
    }
}

#[test]
fn ble_meets_the_contract() {
    use fake_radio::FakeRadio;
    use rust_lib_hoppler::transport::ble::BleTransport;

    let radio = FakeRadio::new();
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();

    let pa = radio.attach("ble-a");
    let a = BleTransport::new("ble-a", pa.clone(), sink_a).unwrap();
    pa.set_ingress(a.ingress());

    let pb = radio.attach("ble-b");
    let b = BleTransport::new("ble-b", pb.clone(), sink_b).unwrap();
    pb.set_ingress(b.ingress());

    conformance(Pair {
        a: Box::new(a),
        b: Box::new(b),
        rx_a,
        rx_b,
        id_a: "ble-a".into(),
        id_b: "ble-b".into(),
        // The fake airspace has no multicast problem, so discovery is exercised.
        discovery_works: true,
    });
}

// ── BLE: what the adapter is deliberately not trusted with ──────────────────
//
// The conformance suite proves the rung honours the contract when the radio
// behaves. These cover the opposite case — the module's claim that a
// misbehaving adapter still cannot break the core — because that claim is the
// entire reason the split exists, and per-OEM BLE misbehaviour is the norm.

/// A platform whose behaviour the test dictates, standing in for an OEM stack
/// with its own ideas.
struct Probe {
    ingress: Mutex<Option<rust_lib_hoppler::transport::ble::BleIngress>>,
    sent: Mutex<Vec<(String, usize)>>,
    ids: Mutex<Vec<String>>,
    /// Whether to acknowledge writes from inside `send`, as a fast inline
    /// write does.
    ack_inline: std::sync::atomic::AtomicBool,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Probe {
            ingress: Mutex::new(None),
            sent: Mutex::new(Vec::new()),
            ids: Mutex::new(Vec::new()),
            ack_inline: std::sync::atomic::AtomicBool::new(true),
        })
    }
    fn attach(&self, ingress: rust_lib_hoppler::transport::ble::BleIngress) {
        *self.ingress.lock().unwrap() = Some(ingress);
    }
    fn radio(&self) -> rust_lib_hoppler::transport::ble::BleIngress {
        self.ingress.lock().unwrap().clone().expect("attached")
    }
    fn sent_bytes(&self) -> usize {
        self.sent.lock().unwrap().iter().map(|(_, n)| n).sum()
    }
}

impl rust_lib_hoppler::transport::ble::BlePlatform for Probe {
    fn set_local_id(&self, id: &str) -> Result<(), TransportError> {
        self.ids.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn start_advertising(&self, _: &[u8]) -> Result<(), TransportError> {
        Ok(())
    }
    fn stop_advertising(&self) -> Result<(), TransportError> {
        Ok(())
    }
    fn start_scanning(&self) -> Result<(), TransportError> {
        Ok(())
    }
    fn stop_scanning(&self) -> Result<(), TransportError> {
        Ok(())
    }
    fn connect(&self, _: &str) -> Result<(), TransportError> {
        Ok(())
    }
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        self.sent
            .lock()
            .unwrap()
            .push((peer.to_string(), bytes.len()));
        if self.ack_inline.load(std::sync::atomic::Ordering::SeqCst) {
            // Re-entering the core from inside `send` — the case that
            // deadlocked before the pipes map stopped being held across it.
            self.radio().on_write_complete(peer, bytes.len());
        }
        Ok(())
    }
    fn disconnect(&self, _: &str) -> Result<(), TransportError> {
        Ok(())
    }
    fn shutdown(&self) {}
}

/// Build a transport wired to a probe, with one pipe already open.
fn probed(
    peer: &str,
) -> (
    Arc<Probe>,
    rust_lib_hoppler::transport::ble::BleTransport,
    Events,
) {
    use rust_lib_hoppler::transport::ble::{BleTransport, PlatformEvent};
    let probe = Probe::new();
    let (sink, rx) = recorder();
    let t = BleTransport::new("probe", probe.clone(), sink).unwrap();
    probe.attach(t.ingress());
    probe.radio().on_platform_event(PlatformEvent::PipeOpened {
        peer: peer.to_string(),
    });
    opened(&rx, peer);
    (probe, t, rx)
}

#[test]
fn an_adapter_that_acknowledges_inline_does_not_deadlock() {
    let (probe, t, _rx) = probed("p1");
    // Would hang, not fail, if `send` held the pipes map across the adapter
    // call — so the useful signal is that this test finishes at all.
    for _ in 0..200 {
        t.send("p1", &[0u8; 1024]).unwrap();
    }
    assert_eq!(probe.sent_bytes(), 200 * 1024);
}

#[test]
fn the_send_window_pushes_back_and_reopens() {
    let (probe, t, _rx) = probed("p1");
    probe
        .ack_inline
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Fill the window without acknowledging: a radio that never drains must
    // not let the core queue without bound.
    let chunk = vec![0u8; 16 * 1024];
    let mut accepted = 0usize;
    let mut blocked = false;
    for _ in 0..8 {
        match t.send("p1", &chunk) {
            Ok(()) => accepted += chunk.len(),
            Err(TransportError::WouldBlock) => {
                blocked = true;
                break;
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert!(
        blocked,
        "the window never closed: {accepted} bytes accepted"
    );

    // Acknowledging returns credit, and the pipe is usable again.
    probe.radio().on_write_complete("p1", accepted);
    t.send("p1", &chunk).expect("credit was not returned");
}

#[test]
fn a_radio_reporting_one_pipe_twice_opens_it_once() {
    use rust_lib_hoppler::transport::ble::PlatformEvent;
    let (probe, t, rx) = probed("p1");
    // Both ends dialling at once is routine on BLE.
    probe
        .radio()
        .on_platform_event(PlatformEvent::PipeOpened { peer: "p1".into() });
    // The map holding one entry is not the point — a second PipeOpened on the
    // stream would have the core believing in two pipes.
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a duplicate open was re-announced to the core"
    );
    assert_eq!(t.pipes(), vec!["p1".to_string()]);
    // One close must therefore be enough to close it.
    probe
        .radio()
        .on_platform_event(PlatformEvent::PipeClosed { peer: "p1".into() });
    closed(&rx, "p1");
    assert!(t.pipes().is_empty(), "a duplicate open left a second pipe");
}

#[test]
fn a_link_that_dies_before_opening_is_a_failure_not_a_close() {
    use rust_lib_hoppler::transport::ble::{BleTransport, PlatformEvent};
    let probe = Probe::new();
    let (sink, rx) = recorder();
    let t = BleTransport::new("probe", probe.clone(), sink).unwrap();
    probe.attach(t.ingress());

    // Android reports a disconnect for a dial that never completed. The core
    // treats PipeClosed as proof a pipe existed, so passing it through would
    // have it tear down state it never built (rule 2).
    probe
        .radio()
        .on_platform_event(PlatformEvent::PipeClosed { peer: "p1".into() });
    match rx.recv_timeout(Duration::from_secs(5)).expect("no event") {
        TransportEvent::PipeFailed { peer, .. } => assert_eq!(peer, "p1"),
        other => panic!("expected PipeFailed, got {other:?}"),
    }
    drop(t);
}

#[test]
fn bytes_on_a_pipe_the_core_closed_are_dropped() {
    use rust_lib_hoppler::transport::ble::PlatformEvent;
    let (probe, t, rx) = probed("p1");
    t.disconnect("p1").unwrap();
    closed(&rx, "p1");
    // A radio with bytes already in flight when we hung up. Delivering them
    // would put Received after PipeClosed, which the core may treat as
    // impossible.
    probe.radio().on_platform_event(PlatformEvent::Received {
        peer: "p1".into(),
        bytes: b"late".to_vec(),
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "bytes arrived after the pipe closed"
    );
}

#[test]
fn a_malformed_radio_id_never_reaches_the_core() {
    use rust_lib_hoppler::transport::ble::{BleTransport, PlatformEvent};
    let probe = Probe::new();
    let (sink, rx) = recorder();
    let t = BleTransport::new("probe", probe.clone(), sink).unwrap();
    probe.attach(t.ingress());

    // Ids cross rungs in composite form ("ble:1f3a"), and a `.` or `:` splits
    // differently on the two ends of a pipe — one connection, two PeerIds.
    for bad in ["has.dot", "has:colon", "-leading", "", &"x".repeat(64)] {
        probe.radio().on_platform_event(PlatformEvent::PeerFound {
            peer: bad.to_string(),
            payload: b"p".to_vec(),
        });
    }
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a malformed radio id reached the core"
    );
    assert!(t.peers().is_empty());
}

#[test]
fn the_ingress_outlives_the_transport() {
    use rust_lib_hoppler::transport::ble::{BleTransport, PlatformEvent};
    let probe = Probe::new();
    let (sink, rx) = recorder();
    let t = BleTransport::new("probe", probe.clone(), sink).unwrap();
    probe.attach(t.ingress());
    let radio = probe.radio();
    drop(t);
    // Wait for the dispatch thread to release its own reference, so the events
    // below genuinely land on a freed transport rather than a live one that
    // happens to be quiet — otherwise this passes without testing anything.
    let freed = Instant::now();
    while radio.is_live() && freed.elapsed() < Duration::from_secs(5) {
        std::thread::yield_now();
    }
    assert!(!radio.is_live(), "the transport was never freed");

    // An adapter cannot un-schedule work already queued on a radio callback
    // thread, so a dropped transport must absorb late events rather than the
    // adapter having to prevent them.
    radio.on_platform_event(PlatformEvent::PeerFound {
        peer: "p1".into(),
        payload: b"p".to_vec(),
    });
    radio.on_write_complete("p1", 10);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn the_adapter_learns_our_id_even_when_we_never_advertise() {
    use rust_lib_hoppler::transport::ble::BleTransport;
    let probe = Probe::new();
    let (sink, _rx) = recorder();
    let t = BleTransport::new("probe", probe.clone(), sink).unwrap();
    probe.attach(t.ingress());

    // Discovery off still dials paired peers (R0-F2), and the dialer has to
    // introduce itself by the id the core knows it by — never by its MAC. If
    // the adapter only learned the id from start_advertising, a node that
    // never advertises would have no name to offer.
    assert_eq!(probe.ids.lock().unwrap().clone(), vec!["probe".to_string()]);

    t.set_local_id("probe-rotated").unwrap();
    assert_eq!(
        probe.ids.lock().unwrap().clone(),
        vec!["probe".to_string(), "probe-rotated".to_string()],
        "a rotation with nothing advertised must still reach the adapter"
    );
}

/// Rule 6 says the rung is silent *once `shutdown` returns*. That is a claim
/// about a delivery already in flight, which the suite's drain-then-check can
/// only catch by luck — and did not: the BLE rung shipped cloning the sink out
/// and releasing the lock before calling it, so `shutdown` returned while a
/// delivery was still running.
///
/// Run against every rung rather than the one that had the bug, because the
/// next rung is where it would come back.
fn shutdown_waits_for_in_flight_delivery(
    rung: &str,
    make: impl FnOnce(Box<dyn Fn(TransportEvent) + Send + Sync>) -> Arc<dyn Transport>,
) {
    use std::sync::Condvar;

    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (e, r) = (entered.clone(), release.clone());

    let t = make(Box::new(move |_| {
        *e.0.lock().unwrap() = true;
        e.1.notify_all();
        let mut go = r.0.lock().unwrap();
        while !*go {
            go = r.1.wait(go).unwrap();
        }
    }));

    // Dialling an unknown peer emits PipeFailed on every rung, so it is the one
    // event this helper can provoke without a second node.
    let _ = t.connect("ghost");

    {
        let mut in_sink = entered.0.lock().unwrap();
        while !*in_sink {
            let (guard, timeout) = entered
                .1
                .wait_timeout(in_sink, Duration::from_secs(5))
                .unwrap();
            in_sink = guard;
            assert!(!timeout.timed_out(), "{rung}: the sink was never called");
        }
    }

    let (done_tx, done_rx) = channel();
    let shutting = t.clone();
    let handle = std::thread::spawn(move || {
        shutting.shutdown();
        let _ = done_tx.send(());
    });

    assert!(
        done_rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "{rung}: shutdown returned while a delivery was still running — the sink \
         can then be called after the caller believes the rung is silent"
    );

    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("shutdown never completed once the sink returned");
    handle.join().unwrap();
}

#[test]
fn loopback_shutdown_waits_for_an_in_flight_delivery() {
    let net = LoopbackNet::new();
    shutdown_waits_for_in_flight_delivery("loopback", |sink| Arc::new(net.join("sd", sink)));
}

#[test]
fn lan_shutdown_waits_for_an_in_flight_delivery() {
    shutdown_waits_for_in_flight_delivery("lan", |sink| {
        Arc::new(LanTransport::new("sd-lan", sink).unwrap())
    });
}

#[test]
fn ble_shutdown_waits_for_an_in_flight_delivery() {
    use rust_lib_hoppler::transport::ble::BleTransport;
    shutdown_waits_for_in_flight_delivery("ble", |sink| {
        let probe = Probe::new();
        let t = Arc::new(BleTransport::new("sd-ble", probe.clone(), sink).unwrap());
        probe.attach(t.ingress());
        t
    });
}

/// Mirrors `SEND_WINDOW` in the rung; a chunk above it always fails the check.
const SEND_WINDOW_HINT: usize = 64 * 1024;

/// An adapter that acknowledges more than it was given must not wedge the pipe.
///
/// The contract asks for the count that actually reached the radio, so this is
/// a misbehaving adapter — which is exactly the case the core exists to absorb.
/// Both the acknowledgement and the window refund subtract saturatingly; a
/// plain `fetch_sub` would wrap `outstanding` to near `usize::MAX` and the pipe
/// would report `WouldBlock` for ever.
///
/// What this test pins is the acknowledgement side, which is deterministic. The
/// refund side is prevented **by construction** rather than demonstrated here:
/// wrapping it requires an acknowledgement to land between a `send` charging
/// the window and refunding it — a gap of a few instructions that no amount of
/// hammering reaches reliably. A stress version of this test passed against the
/// wrapping code 3/3, so it is not carried; saturating arithmetic on both paths
/// is the guarantee, not the test.
#[test]
fn an_over_acknowledging_adapter_cannot_wedge_the_pipe() {
    let (probe, t, _rx) = probed("p1");
    probe
        .ack_inline
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Put real bytes in flight, then acknowledge far more than exist.
    let chunk = vec![0u8; 16 * 1024];
    t.send("p1", &chunk).unwrap();
    probe.radio().on_write_complete("p1", usize::MAX / 2);

    // A wrapped counter shows up here: every subsequent send is refused with
    // WouldBlock and no acknowledgement can ever bring it back down.
    for i in 0..8 {
        t.send("p1", &chunk).unwrap_or_else(|e| {
            panic!("pipe wedged after an over-acknowledgement (send {i}): {e}")
        });
        probe.radio().on_write_complete("p1", chunk.len());
    }

    // And a chunk larger than the whole window is refused rather than wrapping
    // the counter on its way out through the refund path.
    assert!(matches!(
        t.send("p1", &vec![0u8; SEND_WINDOW_HINT + 1]),
        Err(TransportError::WouldBlock)
    ));
    t.send("p1", b"still alive")
        .expect("the oversized attempt left the window charged");
}
