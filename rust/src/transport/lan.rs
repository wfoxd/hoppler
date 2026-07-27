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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{EventSink, PeerId, Transport, TransportError, TransportEvent, TransportLimits};

const SERVICE_TYPE: &str = "_hoppler._tcp.local.";
/// TXT key carrying the hex-encoded advertising payload.
const PAYLOAD_KEY: &str = "p";
/// Cap on a single read from a pipe.
const READ_CHUNK: usize = 16 * 1024;
/// Largest advertising payload this rung can carry. A TXT property is capped at
/// 255 bytes including its key and separator, and we hex-encode:
/// `1 ("p") + 1 + 2·payload ≤ 255`.
const MAX_ADVERTISING_PAYLOAD: usize = 126;
/// Per-address dial timeout. Candidates are raced in parallel, so this bounds
/// the whole `connect`, not each address.
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Guard against a pathological advertisement, not a filter we expect to bind.
const MAX_DIAL_CANDIDATES: usize = 16;
/// Bounds how long a stalled peer can block a `send` before we report
/// backpressure rather than hanging the caller.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Absolute budget for a peer to identify itself after connecting. Enforced as
/// a deadline, not a socket timeout: `SO_RCVTIMEO` applies per `read` syscall,
/// so a peer dribbling one byte at a time would reset it forever.
const HELLO_DEADLINE: Duration = Duration::from_secs(5);
/// Ceiling on connections awaiting a hello. Beyond this we drop new ones rather
/// than let an attacker spawn unbounded handshake threads.
const MAX_PENDING_HANDSHAKES: usize = 64;

fn io_err(e: impl std::fmt::Display) -> TransportError {
    TransportError::Io(e.to_string())
}

fn unavailable(e: impl std::fmt::Display) -> TransportError {
    TransportError::Unavailable(e.to_string())
}

/// An open pipe. The stream is behind its own mutex so concurrent sends to one
/// peer serialize instead of interleaving their bytes on the fd (contract
/// rule 3).
type Pipe = Arc<Mutex<TcpStream>>;

/// A pipe plus the generation that created it. A reader only tears down the
/// entry it owns: without this, a replacement pipe (peer redialled after a
/// Wi-Fi flap) is deleted by the *old* reader waking up, which emits
/// `PipeClosed` for a pipe that is very much alive.
struct PipeEntry {
    generation: u64,
    stream: Pipe,
}

struct Inner {
    /// How we appear to peers. Rotatable (tech spec §4), so behind a lock.
    local_id: Mutex<PeerId>,
    /// Queue to the sink's own thread — no Transport method may call the sink
    /// before returning (contract rule 1).
    events: Sender<TransportEvent>,
    /// Cleared under the write lock by `shutdown`, so suppression is atomic
    /// with respect to a dispatch already in flight (contract rule 6).
    sink: Arc<RwLock<Option<EventSink>>>,
    mdns: ServiceDaemon,
    port: u16,
    /// The address the listener actually bound, for the shutdown wake-up.
    local_addr: SocketAddr,
    /// Monotonic pipe generation.
    next_generation: AtomicU64,
    /// Connections accepted but not yet identified.
    pending_handshakes: Arc<AtomicU64>,
    /// Peers seen advertising, or that dialled us: id → every address they
    /// might be reached at, in dial-preference order. A multi-homed host
    /// advertises several and mDNS resolves them incrementally, so we keep them
    /// all and race rather than betting on the first seen.
    peers: Mutex<HashMap<PeerId, Vec<SocketAddr>>>,
    pipes: Mutex<HashMap<PeerId, PipeEntry>>,
    advertising: Mutex<Option<Vec<u8>>>,
    /// A mutex, not an atomic: the flag and the mDNS browse/stop call must move
    /// together, or a start/stop race can leave the flag off while a live
    /// browse keeps multicasting — discovery "off" that is still emitting (F2).
    scanning: Mutex<bool>,
    shutdown: AtomicBool,
}

impl Inner {
    fn emit(&self, event: TransportEvent) {
        if !self.shutdown.load(Ordering::SeqCst) {
            let _ = self.events.send(event);
        }
    }

    fn id(&self) -> PeerId {
        self.local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Register an open pipe and start its reader. Both the dialer and the
    /// accepter land here, so the two directions behave identically.
    ///
    /// `PipeOpened` is emitted from the reader thread rather than here, so it
    /// never reaches the sink on the caller's thread (contract rule 1).
    ///
    /// When a pipe to this peer already exists — a redial after a flap, or both
    /// sides dialling at once — the two ends must agree on which connection
    /// survives, or they each keep the one the other discarded and both end up
    /// with nothing. The tie-break is deterministic and symmetric: **keep the
    /// connection dialled by the lexicographically smaller node id.**
    fn adopt(
        self: &Arc<Self>,
        peer: PeerId,
        stream: TcpStream,
        addr: Option<SocketAddr>,
        we_dialled: bool,
    ) {
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let read_half = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                self.emit(TransportEvent::PipeFailed {
                    peer,
                    why: e.to_string(),
                });
                return;
            }
        };
        let me = self.id();
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        {
            let mut pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
            // A handshake still in flight when shutdown began must not resurrect
            // a pipe the drain already swept. Checking under the same lock the
            // drain takes makes this race-free in both orders: either we insert
            // and shutdown closes it, or shutdown wins and we refuse here.
            if self.shutdown.load(Ordering::SeqCst) {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            if pipes.contains_key(&peer) {
                let winner_is_us = me <= peer; // whose dial survives
                if we_dialled != winner_is_us {
                    // The existing pipe wins; drop this one silently.
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return;
                }
                if let Some(old) = pipes.remove(&peer) {
                    if let Ok(s) = old.stream.lock() {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                }
            }
            pipes.insert(
                peer.clone(),
                PipeEntry {
                    generation,
                    stream: Arc::new(Mutex::new(stream)),
                },
            );
        }
        // A peer that dialled us must be dialable back (contract rule 5).
        if let Some(addr) = addr {
            let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
            let known = peers.entry(peer.clone()).or_default();
            if !known.contains(&addr) {
                known.push(addr);
            }
        }

        let inner = Arc::clone(self);
        thread::spawn(move || inner.read_loop(peer, read_half, generation));
    }

    fn read_loop(self: Arc<Self>, peer: PeerId, mut stream: TcpStream, generation: u64) {
        self.emit(TransportEvent::PipeOpened { peer: peer.clone() });

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
        // Tear down only the entry this reader owns. A newer pipe under the
        // same peer belongs to a newer reader, and killing it here would report
        // PipeClosed for a live connection — and then deliver Received after it.
        let removed = {
            let mut pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
            match pipes.get(&peer) {
                Some(entry) if entry.generation == generation => pipes.remove(&peer).is_some(),
                _ => false,
            }
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
        check_label(&node_id)?;
        // Bind dual-stack: mDNS advertises whatever the interfaces carry, which
        // on many hosts is IPv6 link-local *only*. An IPv4-only listener would
        // refuse exactly the addresses we advertise. `::` accepts both families
        // where the OS allows it; fall back to IPv4 where it doesn't.
        let listener = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
            .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))
            .map_err(io_err)?;
        let local_addr = listener.local_addr().map_err(io_err)?;
        let port = local_addr.port();
        let mdns = ServiceDaemon::new().map_err(unavailable)?;

        // The sink lives behind a lock the dispatch thread reads per event, so
        // `shutdown` can revoke it atomically rather than racing a check.
        let sink = Arc::new(RwLock::new(Some(sink)));
        let (events, rx) = channel::<TransportEvent>();
        let dispatch_sink = Arc::clone(&sink);
        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                if let Some(s) = dispatch_sink
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    s(event);
                }
            }
        });

        let inner = Arc::new(Inner {
            local_id: Mutex::new(node_id),
            events,
            sink,
            mdns,
            port,
            local_addr,
            next_generation: AtomicU64::new(0),
            pending_handshakes: Arc::new(AtomicU64::new(0)),
            peers: Mutex::new(HashMap::new()),
            pipes: Mutex::new(HashMap::new()),
            advertising: Mutex::new(None),
            scanning: Mutex::new(false),
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

    /// Teach this node where a peer can be reached, without discovering it.
    /// Discovery is one way to learn an address; a pasted address or a test
    /// harness are others. After this, plain [`Transport::connect`] works.
    pub fn add_peer_addr(&self, peer: &str, addr: SocketAddr) {
        let mut peers = self.inner.peers.lock().unwrap_or_else(|e| e.into_inner());
        let known = peers.entry(peer.to_string()).or_default();
        if !known.contains(&addr) {
            known.push(addr);
        }
    }

    /// Dial a peer directly at `addr`, bypassing discovery.
    pub fn connect_addr(&self, peer: &str, addr: SocketAddr) -> Result<(), TransportError> {
        let stream = match TcpStream::connect_timeout(&addr, DIAL_TIMEOUT) {
            Ok(s) => s,
            Err(e) => {
                self.inner.emit(TransportEvent::PipeFailed {
                    peer: peer.to_string(),
                    why: e.to_string(),
                });
                return Err(io_err(e));
            }
        };
        send_hello(&stream, &self.inner.id(), self.inner.port)?;
        self.inner.adopt(peer.to_string(), stream, Some(addr), true);
        Ok(())
    }

    fn fullname(id: &str) -> String {
        format!("{id}.{SERVICE_TYPE}")
    }

    /// (Re-)publish the mDNS record for the given id and payload.
    fn publish(&self, id: &str, payload: &[u8]) -> Result<(), TransportError> {
        if payload.len() > MAX_ADVERTISING_PAYLOAD {
            return Err(TransportError::PayloadTooLarge {
                max: MAX_ADVERTISING_PAYLOAD,
            });
        }
        let props: HashMap<String, String> = [(PAYLOAD_KEY.to_string(), hex::encode(payload))]
            .into_iter()
            .collect();
        let host = format!("{id}.local.");
        let info = ServiceInfo::new(SERVICE_TYPE, id, &host, (), self.inner.port, props)
            .map_err(unavailable)?
            .enable_addr_auto();
        self.inner.mdns.register(info).map_err(unavailable)
    }
}

/// Dial every candidate concurrently and keep the first connection that comes
/// up; losers are closed. Bounded by one [`DIAL_TIMEOUT`].
///
/// Losing sockets are dropped before any hello is written, so the far side
/// sees a connection that closes without identifying itself and discards it
/// (see [`accept_loop`]) — no phantom pipe is ever reported.
fn dial_race(candidates: &[SocketAddr]) -> Result<(TcpStream, SocketAddr), TransportError> {
    let (tx, rx) = channel();
    for &addr in candidates {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send((addr, TcpStream::connect_timeout(&addr, DIAL_TIMEOUT)));
        });
    }
    drop(tx); // so the loop ends once every dialer has reported

    let deadline = Instant::now() + DIAL_TIMEOUT;
    let mut last = TransportError::Io("no address answered".into());
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((addr, Ok(stream))) => return Ok((stream, addr)),
            Ok((_, Err(e))) => last = io_err(e),
            Err(_) => return Err(last), // all dialers failed, or we ran out of time
        }
    }
}

/// `[len][node_id][listen_port:2]`.
///
/// The port matters: the accepting side sees only the dialer's *ephemeral
/// source* port, which is useless for dialling back once this connection
/// closes. Carrying the listening port is what makes "a peer that dialled us
/// is dialable back" (contract rule 5) actually true rather than nominally so.
fn send_hello(
    mut stream: &TcpStream,
    node_id: &str,
    listen_port: u16,
) -> Result<(), TransportError> {
    let bytes = node_id.as_bytes();
    let mut hello = Vec::with_capacity(3 + bytes.len());
    hello.push(bytes.len() as u8);
    hello.extend_from_slice(bytes);
    hello.extend_from_slice(&listen_port.to_be_bytes());
    stream.write_all(&hello).map_err(io_err)
}

/// Read the peer's hello, giving up at `deadline`.
///
/// The deadline is absolute and re-armed before every read. A socket timeout
/// alone would not do: `SO_RCVTIMEO` applies per `read` syscall, so a peer
/// dribbling one byte at a time resets it indefinitely and holds the
/// handshake open forever.
fn read_hello_by(stream: &mut TcpStream, deadline: Instant) -> std::io::Result<(String, u16)> {
    fn fill(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> std::io::Result<()> {
        let mut filled = 0;
        while filled < buf.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "hello deadline exceeded",
                ));
            }
            stream.set_read_timeout(Some(left))?;
            match stream.read(&mut buf[filled..])? {
                0 => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "closed during hello",
                    ))
                }
                n => filled += n,
            }
        }
        Ok(())
    }

    let mut len = [0u8; 1];
    fill(stream, &mut len, deadline)?;
    let mut id = vec![0u8; len[0] as usize];
    fill(stream, &mut id, deadline)?;
    let mut port = [0u8; 2];
    fill(stream, &mut port, deadline)?;
    let id = String::from_utf8(id)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "node id not utf-8"))?;
    Ok((id, u16::from_be_bytes(port)))
}

/// Every address a resolved service might be dialled at, best first, capped at
/// [`MAX_DIAL_CANDIDATES`].
///
/// Ordering is routable-IPv4 → loopback-IPv4 → routable-IPv6 → link-local
/// IPv6, roughly cheapest-and-most-likely first. A link-local IPv6
/// (`fe80::/10`) is only dialable with its interface scope id — without it,
/// `connect` fails `EINVAL` — so we carry the scope through instead of dropping
/// the address, which on some hosts is the only kind advertised.
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
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked
        .into_iter()
        .map(|(_, a)| a)
        .take(MAX_DIAL_CANDIDATES)
        .collect()
}

/// A `node_id` becomes a DNS label in the mDNS instance name, so it must be
/// label-safe. A dot is the dangerous one: the scanner splits the fullname on
/// `.` to recover the peer id, so `"a.b"` would be seen as `"a"` by the scanner
/// and `"a.b"` by the hello — the same pipe under two ids on the two ends.
fn check_label(id: &str) -> Result<(), TransportError> {
    if id.is_empty() || id.len() > 63 {
        return Err(unavailable("node_id must be 1..=63 bytes"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(unavailable("node_id must be alphanumeric, '-' or '_'"));
    }
    Ok(())
}

fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    for stream in listener.incoming() {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = stream else { continue };
        let addr = stream.peer_addr().ok();

        // Cap unidentified connections: the handshake worker below is cheap,
        // but not free, and an attacker controls how many it spawns.
        if inner.pending_handshakes.load(Ordering::SeqCst) >= MAX_PENDING_HANDSHAKES as u64 {
            continue; // socket drops
        }
        inner.pending_handshakes.fetch_add(1, Ordering::SeqCst);

        // Handshake off the accept loop: a peer that connects and never
        // identifies itself must not wedge accepts for everyone else.
        let inner = Arc::clone(&inner);
        thread::spawn(move || {
            let pending = Arc::clone(&inner.pending_handshakes);
            let identified = read_hello_by(&mut stream, Instant::now() + HELLO_DEADLINE);
            pending.fetch_sub(1, Ordering::SeqCst);
            let Ok((peer, listen_port)) = identified else {
                return;
            };
            if check_label(&peer).is_err() {
                return;
            }
            let _ = stream.set_read_timeout(None);
            // Their source port is ephemeral; pair their IP with the listening
            // port they told us about, or the address is undialable later.
            let dialable = addr.map(|a| SocketAddr::new(a.ip(), listen_port));
            inner.adopt(peer, stream, dialable, false);
        });
    }
}

impl Transport for LanTransport {
    fn name(&self) -> &'static str {
        "lan"
    }

    fn limits(&self) -> TransportLimits {
        TransportLimits {
            max_advertising_payload: MAX_ADVERTISING_PAYLOAD,
            preferred_write_size: READ_CHUNK,
        }
    }

    fn is_available(&self) -> bool {
        !self.inner.shutdown.load(Ordering::SeqCst)
    }

    fn set_local_id(&self, new_id: &str) -> Result<(), TransportError> {
        check_label(new_id)?;
        // Same rule on every rung: a connected peer knows us by the id it
        // dialled, so the core rotates when idle (contract rule 4).
        if !self
            .inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return Err(TransportError::Unavailable(
                "cannot rotate the local id while pipes are open".into(),
            ));
        }
        let mut id = self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *id == new_id {
            return Ok(());
        }
        let payload = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if payload.is_some() {
            // Withdraw the old name before publishing the new one: a rotation
            // that leaves both visible is not a rotation.
            let _ = self.inner.mdns.unregister(&Self::fullname(&id));
        }
        *id = new_id.to_string();
        drop(id);
        if let Some(payload) = payload {
            self.publish(new_id, &payload)?;
        }
        Ok(())
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        let id = self.inner.id();
        self.publish(&id, &payload)?;
        *self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(payload);
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        let was = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if was.is_some() {
            self.inner
                .mdns
                .unregister(&Self::fullname(&self.inner.id()))
                .map_err(unavailable)?;
        }
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        // Hold the flag lock across the browse call so a concurrent stop can't
        // leave us multicasting with scanning reported as off.
        let mut scanning = self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *scanning {
            return Ok(()); // already scanning
        }
        let receiver = self.inner.mdns.browse(SERVICE_TYPE).map_err(unavailable)?;
        *scanning = true;
        drop(scanning);
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                let still_scanning = *inner.scanning.lock().unwrap_or_else(|e| e.into_inner());
                if inner.shutdown.load(Ordering::SeqCst) || !still_scanning {
                    return;
                }
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let peer = info.fullname.split('.').next().unwrap_or("").to_string();
                        if peer.is_empty() || peer == inner.id() {
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
                        if peer.is_empty() || peer == inner.id() {
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
        let mut scanning = self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *scanning {
            let _ = self.inner.mdns.stop_browse(SERVICE_TYPE);
            *scanning = false;
        }
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        // Already open: still announce it, so a caller awaiting PipeOpened after
        // connect never hangs (contract rule 2).
        if self
            .inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(peer)
        {
            self.inner.emit(TransportEvent::PipeOpened {
                peer: peer.to_string(),
            });
            return Ok(());
        }
        let candidates = self.peer_addrs(peer);
        if candidates.is_empty() {
            self.inner.emit(TransportEvent::PipeFailed {
                peer: peer.to_string(),
                why: "peer not discovered".into(),
            });
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        // Race the candidates: a multi-homed host advertises one address per
        // interface and only some are reachable. Serial dialling would cost the
        // timeout once per dead address; racing costs it once in total.
        let (stream, addr) = match dial_race(&candidates) {
            Ok(pair) => pair,
            Err(e) => {
                self.inner.emit(TransportEvent::PipeFailed {
                    peer: peer.to_string(),
                    why: e.to_string(),
                });
                return Err(e);
            }
        };
        send_hello(&stream, &self.inner.id(), self.inner.port)?;
        self.inner.adopt(peer.to_string(), stream, Some(addr), true);
        Ok(())
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        let pipe = {
            let pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes
                .get(peer)
                .map(|e| Arc::clone(&e.stream))
                .ok_or_else(|| TransportError::NoSuchPeer(peer.to_string()))?
        };
        // Hold the per-pipe lock across the whole write so concurrent sends to
        // one peer can't interleave their bytes (contract rule 3).
        let mut stream = pipe.lock().unwrap_or_else(|e| e.into_inner());
        stream.write_all(bytes).map_err(|e| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                TransportError::WouldBlock
            }
            _ => io_err(e),
        })
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        let removed = {
            let mut pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes.remove(peer)
        };
        if let Some(entry) = removed {
            if let Ok(s) = entry.stream.lock() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            self.inner.emit(TransportEvent::PipeClosed {
                peer: peer.to_string(),
            });
        }
        Ok(())
    }

    fn peers(&self) -> Vec<PeerId> {
        self.inner
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    fn pipes(&self) -> Vec<PeerId> {
        self.inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    fn shutdown(&self) {
        if self.inner.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.stop_advertising();
        let _ = self.stop_scanning();
        let pipes: Vec<Pipe> = {
            let mut guard = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            guard.drain().map(|(_, e)| e.stream).collect()
        };
        for p in pipes {
            if let Ok(s) = p.lock() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
        let _ = self.inner.mdns.shutdown();
        // Revoke the sink under the write lock: any event already dequeued but
        // not yet delivered finds None, so no event escapes after we return.
        *self.inner.sink.write().unwrap_or_else(|e| e.into_inner()) = None;
        // Unblock the accept loop, which exits on the shutdown flag. Dial the
        // address the listener actually bound — assuming v4-mapped dual stack
        // leaves the thread blocked forever where the OS doesn't provide it.
        let wake = match self.inner.local_addr {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::LOCALHOST, self.inner.port)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::LOCALHOST, self.inner.port)),
        };
        let _ = TcpStream::connect_timeout(&wake, Duration::from_secs(1));
    }
}

impl Drop for LanTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}
