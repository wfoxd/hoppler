//! The LAN rung: mDNS for discovery, TCP for the byte pipe (tech spec §9,
//! rung 4).
//!
//! Pure Rust, so the *same* implementation runs on Linux desktop and Android
//! with no native plugin — which is why it lands before BLE: it makes the
//! transport trait real and testable today, and gives the session layer (T10)
//! something honest to develop against while the radio rungs are built.
//!
//! Addressing note: TCP tells the accepting side an IP, not who dialled, so a
//! dialer opens with a one-shot `[len][node_id]` hello. That is link-layer
//! addressing — the thing BLE hands you for free in a connection handle — and
//! deliberately the *only* bytes this layer writes on its own. Everything after
//! it is opaque payload.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{EventSink, PeerId, Transport, TransportError, TransportEvent};

const SERVICE_TYPE: &str = "_hoppler._tcp.local.";
/// TXT key carrying the hex-encoded advertising payload.
const PAYLOAD_KEY: &str = "p";
/// Cap on a single read from a pipe; also the hello's maximum node-id length.
const READ_CHUNK: usize = 16 * 1024;
/// Per-address dial timeout. A LAN peer answers in milliseconds; this bounds
/// how long a wrong candidate costs. Candidates are raced in parallel, so this
/// is the cost of the whole `connect`, not of each address.
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Upper bound on addresses raced at once — a guard against a pathological
/// advertisement, not a filter we expect to bind.
const MAX_DIAL_CANDIDATES: usize = 16;

fn io_err(e: impl std::fmt::Display) -> TransportError {
    TransportError::Io(e.to_string())
}

fn unavailable(e: impl std::fmt::Display) -> TransportError {
    TransportError::Unavailable(e.to_string())
}

struct Inner {
    node_id: String,
    sink: EventSink,
    mdns: ServiceDaemon,
    port: u16,
    /// Peers seen advertising: id → every address they might be reached at,
    /// in dial-preference order. A multi-homed host (Wi-Fi + docker bridges +
    /// link-local v6) advertises several, and mDNS resolves them
    /// incrementally, so we keep them all and try in turn rather than betting
    /// on the first one seen.
    peers: Mutex<HashMap<PeerId, Vec<SocketAddr>>>,
    /// Open pipes: id → the write half.
    pipes: Mutex<HashMap<PeerId, TcpStream>>,
    advertising: AtomicBool,
    scanning: AtomicBool,
    shutdown: AtomicBool,
}

impl Inner {
    fn emit(&self, event: TransportEvent) {
        if !self.shutdown.load(Ordering::SeqCst) {
            (self.sink)(event);
        }
    }

    /// Register an open pipe and start its reader. Both the dialer and the
    /// accepter land here, so the two directions behave identically.
    fn adopt(self: &Arc<Self>, peer: PeerId, stream: TcpStream) -> Result<(), TransportError> {
        let write_half = stream.try_clone().map_err(io_err)?;
        {
            let mut pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(old) = pipes.insert(peer.clone(), write_half) {
                let _ = old.shutdown(std::net::Shutdown::Both);
            }
        }
        self.emit(TransportEvent::PipeOpened { peer: peer.clone() });

        let inner = Arc::clone(self);
        thread::spawn(move || inner.read_loop(peer, stream));
        Ok(())
    }

    fn read_loop(self: Arc<Self>, peer: PeerId, mut stream: TcpStream) {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,  // peer hung up
                Err(_) => break, // severed link
                Ok(n) => self.emit(TransportEvent::Received {
                    peer: peer.clone(),
                    bytes: buf[..n].to_vec(),
                }),
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return; // shutdown() already reported the closure
            }
        }
        // Only the side still holding the pipe reports the close, so a
        // disconnect() plus a severed read don't both fire PipeClosed.
        let removed = {
            let mut pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes.remove(&peer).is_some()
        };
        if removed {
            self.emit(TransportEvent::PipeClosed { peer });
        }
    }
}

/// A transport over the local network.
pub struct LanTransport {
    inner: Arc<Inner>,
}

impl LanTransport {
    /// Bind a listener, start the mDNS daemon, and begin accepting pipes.
    /// `node_id` names this node within the transport (see [`PeerId`]).
    pub fn new(node_id: impl Into<PeerId>, sink: EventSink) -> Result<Self, TransportError> {
        let node_id = node_id.into();
        if node_id.is_empty() || node_id.len() > 255 {
            return Err(unavailable("node_id must be 1..=255 bytes"));
        }
        // Bind dual-stack: mDNS advertises whatever the interfaces carry, which
        // on many hosts is IPv6 link-local *only*. An IPv4-only listener would
        // refuse exactly the addresses we advertise. `::` accepts both families
        // where the OS allows it; fall back to IPv4 where it doesn't.
        let listener = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
            .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))
            .map_err(io_err)?;
        let port = listener.local_addr().map_err(io_err)?.port();
        let mdns = ServiceDaemon::new().map_err(unavailable)?;

        let inner = Arc::new(Inner {
            node_id,
            sink,
            mdns,
            port,
            peers: Mutex::new(HashMap::new()),
            pipes: Mutex::new(HashMap::new()),
            advertising: AtomicBool::new(false),
            scanning: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });

        let accept_inner = Arc::clone(&inner);
        thread::spawn(move || accept_loop(accept_inner, listener));
        Ok(Self { inner })
    }

    /// The port this node listens on (useful in tests and diagnostics).
    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Every address a discovered peer might be dialled at, best first.
    pub fn peer_addrs(&self, peer: &str) -> Vec<SocketAddr> {
        self.inner
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned()
            .unwrap_or_default()
    }

    /// Dial a peer directly, bypassing mDNS. Discovery is one way to learn an
    /// address; tests and a future "paste an address" path are others.
    pub fn connect_addr(&self, peer: &str, addr: SocketAddr) -> Result<(), TransportError> {
        let stream = TcpStream::connect_timeout(&addr, DIAL_TIMEOUT).map_err(io_err)?;
        send_hello(&stream, &self.inner.node_id)?;
        self.inner.adopt(peer.to_string(), stream)
    }
}

/// Dial every candidate concurrently and keep the first connection that comes
/// up; losers are closed. Bounded by one [`DIAL_TIMEOUT`].
///
/// Losing sockets are dropped before any hello is written, so the far side
/// sees a connection that closes without identifying itself and discards it
/// (see [`accept_loop`]) — no phantom pipe is ever reported.
fn dial_race(candidates: &[SocketAddr]) -> Result<TcpStream, TransportError> {
    let (tx, rx) = std::sync::mpsc::channel();
    for &addr in candidates {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(TcpStream::connect_timeout(&addr, DIAL_TIMEOUT));
        });
    }
    drop(tx); // so the loop ends once every dialer has reported

    let deadline = std::time::Instant::now() + DIAL_TIMEOUT;
    let mut last = TransportError::Io("no address answered".into());
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last = io_err(e),
            Err(_) => return Err(last), // all dialers failed, or we ran out of time
        }
    }
}

fn send_hello(mut stream: &TcpStream, node_id: &str) -> Result<(), TransportError> {
    let bytes = node_id.as_bytes();
    let mut hello = Vec::with_capacity(1 + bytes.len());
    hello.push(bytes.len() as u8);
    hello.extend_from_slice(bytes);
    stream.write_all(&hello).map_err(io_err)
}

fn read_hello(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut len = [0u8; 1];
    stream.read_exact(&mut len)?;
    let mut id = vec![0u8; len[0] as usize];
    stream.read_exact(&mut id)?;
    String::from_utf8(id)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "node id not utf-8"))
}

/// Every address a resolved service might be dialled at, best first, capped at
/// [`MAX_DIAL_CANDIDATES`].
///
/// Ordering is routable-IPv4 → loopback-IPv4 → routable-IPv6 → link-local
/// IPv6, because that is roughly cheapest-and-most-likely first. A link-local
/// IPv6 (`fe80::/10`) is only dialable with its interface scope id — without
/// it, `connect` fails `EINVAL` — so we carry the scope through instead of
/// dropping the address, which on some hosts is the only kind advertised.
fn dial_candidates(info: &mdns_sd::ResolvedService) -> Vec<SocketAddr> {
    let port = info.port;
    let mut ranked: Vec<(u8, SocketAddr)> = Vec::new();
    for scoped in &info.addresses {
        match scoped {
            mdns_sd::ScopedIp::V4(a) => {
                let addr = *a.addr();
                let rank = if addr.is_loopback() { 1 } else { 0 };
                ranked.push((rank, SocketAddr::new(IpAddr::V4(addr), port)));
            }
            mdns_sd::ScopedIp::V6(a) => {
                let addr = *a.addr();
                let rank = if addr.is_unicast_link_local() { 3 } else { 2 };
                let sock = SocketAddrV6::new(addr, port, 0, a.scope_id().index);
                ranked.push((rank, SocketAddr::V6(sock)));
            }
            // ScopedIp is #[non_exhaustive]: skip families we can't dial.
            _ => {}
        }
    }
    // Stable sort by rank, then take the best few.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked
        .into_iter()
        .map(|(_, a)| a)
        .take(MAX_DIAL_CANDIDATES)
        .collect()
}

fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    for stream in listener.incoming() {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = stream else { continue };
        // A peer that never sends its hello must not wedge the accept loop.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(peer) = read_hello(&mut stream) else {
            continue;
        };
        let _ = stream.set_read_timeout(None);
        let _ = inner.adopt(peer, stream);
    }
}

impl Transport for LanTransport {
    fn name(&self) -> &'static str {
        "lan"
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        // Re-registering the same instance replaces the record — the rotation
        // hook the core drives.
        let props = [(PAYLOAD_KEY.to_string(), hex::encode(&payload))]
            .into_iter()
            .collect::<HashMap<_, _>>();
        let host = format!("{}.local.", self.inner.node_id);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.inner.node_id,
            &host,
            (), // resolve our addresses automatically
            self.inner.port,
            props,
        )
        .map_err(unavailable)?
        .enable_addr_auto();

        self.inner.mdns.register(info).map_err(unavailable)?;
        self.inner.advertising.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        if self.inner.advertising.swap(false, Ordering::SeqCst) {
            let fullname = format!("{}.{SERVICE_TYPE}", self.inner.node_id);
            self.inner.mdns.unregister(&fullname).map_err(unavailable)?;
        }
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        if self.inner.scanning.swap(true, Ordering::SeqCst) {
            return Ok(()); // already scanning
        }
        let receiver = self.inner.mdns.browse(SERVICE_TYPE).map_err(unavailable)?;
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if inner.shutdown.load(Ordering::SeqCst) || !inner.scanning.load(Ordering::SeqCst) {
                    return;
                }
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let peer = info.fullname.split('.').next().unwrap_or("").to_string();
                        if peer.is_empty() || peer == inner.node_id {
                            continue; // never discover ourselves
                        }
                        let payload = info
                            .txt_properties
                            .get_property_val_str(PAYLOAD_KEY)
                            .and_then(|hex| hex::decode(hex).ok())
                            .unwrap_or_default();
                        let candidates = dial_candidates(&info);
                        if candidates.is_empty() {
                            continue;
                        }
                        inner
                            .peers
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(peer.clone(), candidates);
                        inner.emit(TransportEvent::PeerFound { peer, payload });
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let peer = fullname.split('.').next().unwrap_or("").to_string();
                        if peer.is_empty() || peer == inner.node_id {
                            continue;
                        }
                        inner
                            .peers
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&peer);
                        inner.emit(TransportEvent::PeerLost { peer });
                    }
                    _ => {}
                }
            }
        });
        Ok(())
    }

    fn stop_scanning(&self) -> Result<(), TransportError> {
        if self.inner.scanning.swap(false, Ordering::SeqCst) {
            let _ = self.inner.mdns.stop_browse(SERVICE_TYPE);
        }
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        if self
            .inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(peer)
        {
            return Ok(()); // already open
        }
        let candidates = self.peer_addrs(peer);
        if candidates.is_empty() {
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        // Race the candidates. A multi-homed host advertises one address per
        // interface — a dozen link-local ones on a box with docker bridges —
        // and only some are reachable. Dialling them serially would cost the
        // timeout once per dead address; racing costs it once in total.
        let stream = dial_race(&candidates)?;
        send_hello(&stream, &self.inner.node_id)?;
        self.inner.adopt(peer.to_string(), stream)
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        let mut stream = {
            let pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes
                .get(peer)
                .ok_or_else(|| TransportError::NoSuchPeer(peer.to_string()))?
                .try_clone()
                .map_err(io_err)?
        };
        stream.write_all(bytes).map_err(io_err)
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        let removed = {
            let mut pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes.remove(peer)
        };
        if let Some(stream) = removed {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            self.inner.emit(TransportEvent::PipeClosed {
                peer: peer.to_string(),
            });
        }
        Ok(())
    }

    fn shutdown(&self) {
        if self.inner.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.stop_advertising();
        let _ = self.stop_scanning();
        let pipes: Vec<TcpStream> = {
            let mut guard = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            guard.drain().map(|(_, s)| s).collect()
        };
        for s in pipes {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let _ = self.inner.mdns.shutdown();
        // Unblock the accept loop, which exits on the shutdown flag.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.inner.port));
    }
}

impl Drop for LanTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}
