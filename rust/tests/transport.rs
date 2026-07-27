//! Transport-layer tests (T08). The same behavioural suite is run against
//! *both* rungs — the in-process loopback and the real LAN transport — because
//! the point of the trait is that the session layer above cannot tell them
//! apart. A rung that passes these is a drop-in for the next one (BLE).
//!
//! mDNS discovery needs multicast, which container CI usually lacks, so the
//! discovery test is `#[ignore]`d and run locally; every other LAN test dials a
//! known address over loopback TCP and runs everywhere.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rust_lib_hoppler::transport::lan::LanTransport;
use rust_lib_hoppler::transport::loopback::LoopbackNet;
use rust_lib_hoppler::transport::{Transport, TransportError, TransportEvent};

/// Collects a node's events into a channel.
fn recorder() -> (
    Box<dyn Fn(TransportEvent) + Send + Sync>,
    Receiver<TransportEvent>,
) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let sink: Box<dyn Fn(TransportEvent) + Send + Sync> = Box::new(move |e| {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(e);
    });
    (sink, rx)
}

/// Wait for the first event matching `pred`, ignoring others.
fn wait_for(
    rx: &Receiver<TransportEvent>,
    pred: impl Fn(&TransportEvent) -> bool,
) -> TransportEvent {
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

fn received_bytes(rx: &Receiver<TransportEvent>, want: usize) -> Vec<u8> {
    let mut got = Vec::new();
    while got.len() < want {
        match wait_for(rx, |e| matches!(e, TransportEvent::Received { .. })) {
            TransportEvent::Received { bytes, .. } => got.extend_from_slice(&bytes),
            _ => unreachable!(),
        }
    }
    got
}

// ── loopback ────────────────────────────────────────────────────────────────

#[test]
fn loopback_discovers_advertisers_and_pipes_bytes() {
    let net = LoopbackNet::new();
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = net.join("a", sink_a);
    let b = net.join("b", sink_b);

    b.start_advertising(b"beacon-b".to_vec()).unwrap();
    a.start_scanning().unwrap();
    let found = wait_for(&rx_a, |e| matches!(e, TransportEvent::PeerFound { .. }));
    assert_eq!(
        found,
        TransportEvent::PeerFound {
            peer: "b".into(),
            payload: b"beacon-b".to_vec()
        }
    );

    a.connect("b").unwrap();
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeOpened { .. }));
    a.send("b", b"hello").unwrap();
    assert_eq!(received_bytes(&rx_b, 5), b"hello");

    // The pipe is bidirectional.
    b.send("a", b"hi").unwrap();
    assert_eq!(received_bytes(&rx_a, 2), b"hi");

    a.disconnect("b").unwrap();
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeClosed { .. }));
    assert!(matches!(
        a.send("b", b"x"),
        Err(TransportError::NoSuchPeer(_))
    ));
}

#[test]
fn loopback_stop_advertising_hides_us_and_notifies_scanners() {
    let net = LoopbackNet::new();
    let (sink_a, rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = net.join("a", sink_a);
    let b = net.join("b", sink_b);

    a.start_scanning().unwrap();
    b.start_advertising(b"x".to_vec()).unwrap();
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PeerFound { .. }));

    b.stop_advertising().unwrap();
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PeerLost { .. }));
}

#[test]
fn loopback_never_discovers_itself() {
    let net = LoopbackNet::new();
    let (sink, rx) = recorder();
    let a = net.join("a", sink);
    a.start_advertising(b"me".to_vec()).unwrap();
    a.start_scanning().unwrap();
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a node must not discover its own advertisement"
    );
}

#[test]
fn loopback_connect_to_unknown_peer_errors() {
    let net = LoopbackNet::new();
    let (sink, _rx) = recorder();
    let a = net.join("a", sink);
    assert!(matches!(
        a.connect("ghost"),
        Err(TransportError::NoSuchPeer(_))
    ));
}

// ── LAN ─────────────────────────────────────────────────────────────────────

#[test]
fn lan_pipe_carries_bytes_both_ways() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("lan-a", sink_a).unwrap();
    let b = LanTransport::new("lan-b", sink_b).unwrap();

    // Dial by address — discovery is exercised separately (needs multicast).
    a.connect_addr("lan-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PipeOpened { .. }));
    // The accepting side learns who dialled from the link-layer hello.
    assert_eq!(
        wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeOpened { .. })),
        TransportEvent::PipeOpened {
            peer: "lan-a".into()
        }
    );

    a.send("lan-b", b"ping").unwrap();
    assert_eq!(received_bytes(&rx_b, 4), b"ping");
    b.send("lan-a", b"pong").unwrap();
    assert_eq!(received_bytes(&rx_a, 4), b"pong");
}

#[test]
fn lan_reassembles_a_large_payload_in_order() {
    let (sink_a, _rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("big-a", sink_a).unwrap();
    let b = LanTransport::new("big-b", sink_b).unwrap();
    a.connect_addr("big-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeOpened { .. }));

    // TCP does not preserve message boundaries (the trait says so), but the
    // byte stream must arrive complete and in order.
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    a.send("big-b", &payload).unwrap();
    assert_eq!(received_bytes(&rx_b, payload.len()), payload);
}

#[test]
fn lan_disconnect_closes_both_ends() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("dis-a", sink_a).unwrap();
    let b = LanTransport::new("dis-b", sink_b).unwrap();
    a.connect_addr("dis-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeOpened { .. }));

    a.disconnect("dis-b").unwrap();
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PipeClosed { .. }));
    // The severed link reaches the far side too.
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeClosed { .. }));
    assert!(matches!(
        a.send("dis-b", b"x"),
        Err(TransportError::NoSuchPeer(_))
    ));
}

#[test]
fn lan_shutdown_is_idempotent_and_silences_the_node() {
    let (sink_a, rx_a) = recorder();
    let (sink_b, _rx_b) = recorder();
    let a = LanTransport::new("sd-a", sink_a).unwrap();
    let b = LanTransport::new("sd-b", sink_b).unwrap();
    a.connect_addr("sd-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PipeOpened { .. }));

    a.shutdown();
    a.shutdown(); // idempotent
                  // Nothing more is emitted, and the node is unusable.
    if let Ok(e) = rx_a.recv_timeout(Duration::from_millis(200)) {
        panic!("event after shutdown: {e:?}");
    }
    assert!(a.send("sd-b", b"x").is_err());
}

#[test]
fn lan_send_to_unknown_peer_errors() {
    let (sink, _rx) = recorder();
    let a = LanTransport::new("solo", sink).unwrap();
    assert!(matches!(
        a.send("ghost", b"x"),
        Err(TransportError::NoSuchPeer(_))
    ));
    assert!(matches!(
        a.connect("ghost"),
        Err(TransportError::NoSuchPeer(_))
    ));
}

/// Throughput smoke for the byte pipe (T08 asks for ≥ 10 kB/s; loopback TCP is
/// orders of magnitude above that, so this guards against a pathological
/// regression rather than measuring the radio).
#[test]
fn lan_pipe_throughput_is_sane() {
    let (sink_a, _rx_a) = recorder();
    let (sink_b, rx_b) = recorder();
    let a = LanTransport::new("tp-a", sink_a).unwrap();
    let b = LanTransport::new("tp-b", sink_b).unwrap();
    a.connect_addr("tp-b", ([127, 0, 0, 1], b.port()).into())
        .unwrap();
    wait_for(&rx_b, |e| matches!(e, TransportEvent::PipeOpened { .. }));

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
    wait_for(&rx_a, |e| matches!(e, TransportEvent::PipeOpened { .. }));
}
