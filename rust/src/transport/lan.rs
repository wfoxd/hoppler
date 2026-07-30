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

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
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
/// How long a pipe may sit idle before the kernel starts probing the peer.
///
/// A phone that vanishes — aeroplane mode, out of range, a battery pull — sends
/// no FIN and no RST: its interface simply stops existing. Nothing arrives to
/// tell us, so without probes the socket stays open for the life of the
/// process: `read` blocks forever, `send` keeps returning `Ok` into a kernel
/// buffer that will never drain, and no `PipeClosed` is ever emitted. Every
/// layer above then believes a dead peer is reachable. The kernel default is two
/// hours, which for a device that walks out of a room is indistinguishable from
/// never.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(10);
/// Gap between probes once one has gone unanswered.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// Unanswered probes before the kernel declares the connection dead.
/// `10s + 3 × 5s` puts a vanished peer's `PipeClosed` at roughly 25 seconds.
const KEEPALIVE_RETRIES: u32 = 3;

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
    /// Serializes writes. Only `send` takes this.
    write: Pipe,
    /// An independent handle used *only* to tear the socket down. Deliberately
    /// not behind the write mutex: teardown must be able to abort a write that
    /// is currently blocked on a stalled peer, and anything that merely wants
    /// to close a pipe must never queue behind that writer.
    teardown: TcpStream,
}

/// Lock order, where more than one is held: **`local_id` → `advertising` →
/// `peers` → `pipes`**. (`scanning` is never held alongside another.)
///
/// `set_local_id`, `start_advertising` and `stop_advertising` each hold the
/// first two *together*, and must: taking them in opposite orders deadlocks a
/// rotation against a concurrent advertise, but merely reading the id and
/// releasing it is not enough either — the publish would then race a rotation
/// and leave the pre-rotation name registered, which defeats the rotation.
struct Inner {
    /// How we appear to peers. Rotatable (tech spec §4), so behind a lock.
    local_id: Mutex<PeerId>,
    /// Every name we currently have registered with mDNS. Normally 0 or 1 —
    /// it is a set because the bug worth catching is a *second* name surviving
    /// a rotation, and a scalar cannot represent the state it fails in.
    registered: Mutex<HashSet<PeerId>>,
    /// Queue to the sink's own thread — no Transport method may call the sink
    /// before returning (contract rule 1).
    events: Sender<TransportEvent>,
    /// Cleared under the write lock by `shutdown`, so suppression is atomic
    /// with respect to a dispatch already in flight (contract rule 6).
    sink: Arc<RwLock<Option<super::SharedSink>>>,
    /// Set before revocation and checked under the read guard, so a sink that
    /// is *about* to be called is stopped even when `shutdown` cannot take the
    /// write lock (see `revoke`).
    revoked: Arc<AtomicBool>,
    /// Which thread runs the sink. `shutdown` called *from* a sink must not
    /// block on the write lock that thread already read-holds.
    dispatch_thread: Arc<OnceLock<thread::ThreadId>>,
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

    /// Stop delivering, and — unless we are *on* the dispatch thread — wait for
    /// any call already in flight, so silence is exact once `shutdown` returns.
    /// A sink that calls `shutdown` itself takes the reentrant path: it already
    /// read-holds the lock, so taking the write lock here would deadlock.
    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        let on_dispatch = self.dispatch_thread.get() == Some(&thread::current().id());
        if !on_dispatch {
            *self.sink.write().unwrap_or_else(|e| e.into_inner()) = None;
        }
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
        // Both directions funnel through here, so one call covers dialer and
        // accepter. Keepalive lives on the file description, so the clones below
        // — the reader that has to unblock, and the teardown handle — inherit it.
        arm_keepalive(&stream);

        let (read_half, teardown) = match (stream.try_clone(), stream.try_clone()) {
            (Ok(r), Ok(t)) => (r, t),
            _ => {
                self.emit(TransportEvent::PipeFailed {
                    peer,
                    why: "could not duplicate socket".into(),
                });
                return;
            }
        };
        let me = self.id();
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);

        // Record the address BEFORE announcing the pipe. PipeOpened is emitted
        // under the pipes guard below, so a caller that reacts to it by dialling
        // back would otherwise race an address book that is still empty —
        // rule 5 must hold the moment the event is observable.
        if let Some(addr) = addr {
            let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
            let known = peers.entry(peer.clone()).or_default();
            if !known.contains(&addr) {
                known.push(addr);
            }
        }
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
                    // Close via the teardown handle: the old writer may be
                    // blocked holding its own write mutex.
                    let _ = old.teardown.shutdown(std::net::Shutdown::Both);
                }
            }
            pipes.insert(
                peer.clone(),
                PipeEntry {
                    generation,
                    write: Arc::new(Mutex::new(stream)),
                    teardown,
                },
            );
            // Emitted under the same guard as the insert, so {insert,
            // PipeOpened} is atomic against {remove, PipeClosed}. Without this a
            // concurrent disconnect could publish PipeClosed first and leave the
            // caller believing a dead pipe is live, permanently. `emit` only
            // enqueues, so holding the lock here is safe (rule 1).
            self.emit(TransportEvent::PipeOpened { peer: peer.clone() });
        }

        let inner = Arc::clone(self);
        thread::spawn(move || inner.read_loop(peer, read_half, generation));
    }

    fn read_loop(self: Arc<Self>, peer: PeerId, mut stream: TcpStream, generation: u64) {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,  // peer hung up
                Err(_) => break, // severed link
                Ok(n) => {
                    // Publish under the pipes guard, and only while we are still
                    // the current pipe: otherwise a concurrent disconnect can
                    // emit PipeClosed first and these bytes arrive for a peer
                    // the caller has already torn down.
                    let pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
                    if pipes.get(&peer).is_none_or(|e| e.generation != generation) {
                        return;
                    }
                    self.emit(TransportEvent::Received {
                        peer: peer.clone(),
                        bytes: buf[..n].to_vec(),
                    });
                }
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return; // shutdown() already reported the closure
            }
        }
        // Tear down only the entry this reader owns. A newer pipe under the
        // same peer belongs to a newer reader, and killing it here would report
        // PipeClosed for a live connection — and then deliver Received after it.
        let mut pipes = self.pipes.lock().unwrap_or_else(|e| e.into_inner());
        if pipes.get(&peer).is_some_and(|e| e.generation == generation) {
            pipes.remove(&peer);
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
        let listener = bind_dual_stack().map_err(io_err)?;
        let local_addr = listener.local_addr().map_err(io_err)?;
        let port = local_addr.port();
        let mdns = ServiceDaemon::new().map_err(unavailable)?;

        // The sink lives behind a lock the dispatch thread reads per event, so
        // `shutdown` can revoke it atomically rather than racing a check.
        let sink: Arc<RwLock<Option<super::SharedSink>>> =
            Arc::new(RwLock::new(Some(Arc::from(sink))));
        let revoked = Arc::new(AtomicBool::new(false));
        let dispatch_thread: Arc<OnceLock<thread::ThreadId>> = Arc::new(OnceLock::new());
        let (events, rx) = channel::<TransportEvent>();
        let dispatch_sink = Arc::clone(&sink);
        let dispatch_revoked = Arc::clone(&revoked);
        let dispatch_id = Arc::clone(&dispatch_thread);
        thread::spawn(move || {
            let _ = dispatch_id.set(thread::current().id());
            while let Ok(event) = rx.recv() {
                // The guard is held ACROSS the call, so a `shutdown` on another
                // thread blocks until this delivery finishes and then revokes —
                // making "silent once shutdown returns" exact rather than
                // best-effort. `revoked` covers the case where shutdown could
                // not take the write lock because the sink itself called it.
                let guard = dispatch_sink.read().unwrap_or_else(|e| e.into_inner());
                if dispatch_revoked.load(Ordering::SeqCst) {
                    continue;
                }
                if let Some(s) = guard.as_ref() {
                    s(event);
                }
            }
        });

        let inner = Arc::new(Inner {
            local_id: Mutex::new(node_id),
            registered: Mutex::new(HashSet::new()),
            events,
            sink,
            revoked,
            dispatch_thread,
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

    /// Refuse anything mutating once shut down: a dead rung must not dial
    /// sockets, burn a peer's handshake budget, or return `Ok` for work it will
    /// never do (contract rule 6).
    fn alive(&self) -> Result<(), TransportError> {
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return Err(TransportError::Unavailable(
                "transport has shut down".into(),
            ));
        }
        Ok(())
    }

    /// The port this node listens on (useful in tests and diagnostics).
    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Names currently registered with mDNS. Exposed so the rotation tests can
    /// assert the invariant that matters — a rotation leaves exactly one name
    /// behind, never the old one as well.
    #[doc(hidden)]
    pub fn registered_names(&self) -> Vec<PeerId> {
        self.inner
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
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
        self.alive()?;
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
        if let Err(e) = send_hello(&stream, &self.inner.id(), self.inner.port) {
            self.inner.emit(TransportEvent::PipeFailed {
                peer: peer.to_string(),
                why: e.to_string(),
            });
            return Err(e);
        }
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
        self.inner.mdns.register(info).map_err(unavailable)?;
        self.inner
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string());
        Ok(())
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

/// Bind a listener that accepts **both** address families.
///
/// `::` is dual-stack only where the OS says so — Linux/Android default to
/// `bindv6only=0`, but others (and hardened containers) default the other way,
/// where an IPv6 listener silently refuses the IPv4 candidates we advertise.
/// That failure mode is the one recorded in the T08 findings: discovery looks
/// healthy while every dial gets `ECONNREFUSED`. So we ask for it explicitly,
/// and fall back to IPv4 only if IPv6 is unavailable entirely.
fn bind_dual_stack() -> std::io::Result<TcpListener> {
    use socket2::{Domain, Socket, Type};

    let attempt = || -> std::io::Result<TcpListener> {
        let sock = Socket::new(Domain::IPV6, Type::STREAM, None)?;
        sock.set_only_v6(false)?;
        sock.set_reuse_address(true)?;
        sock.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)).into())?;
        sock.listen(128)?;
        Ok(sock.into())
    };
    attempt().or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))
}

/// Ask the kernel to notice a peer that leaves without saying goodbye.
///
/// See [`KEEPALIVE_IDLE`] for why this is not optional on a rung whose peers are
/// phones. Best-effort: a platform that refuses one of these knobs still gets a
/// working pipe, only a slower-to-fail one, which is not worth refusing the
/// connection over. All three are settable on Linux, Android and Apple targets;
/// a platform without `TCP_KEEPCNT` would need `with_retries` cfg'd out.
fn arm_keepalive(stream: &TcpStream) {
    let probe = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    let _ = socket2::SockRef::from(stream).set_tcp_keepalive(&probe);
}

/// Why a write could not be completed.
enum WriteFailure {
    /// Not a single byte reached the wire — safe for the caller to retry.
    Stalled,
    /// Some bytes were delivered before the failure. The stream is torn: a
    /// retry would duplicate the prefix already sent.
    Torn(String),
}

/// Write every byte, or fail saying whether anything was delivered.
///
/// The deadline is absolute and re-armed per syscall. `SO_SNDTIMEO` alone is
/// per-`write`, so a peer accepting a trickle resets it indefinitely — the same
/// trap as `SO_RCVTIMEO` on the read side, so both now count against a
/// wall-clock deadline.
fn write_all_by(stream: &TcpStream, buf: &[u8], deadline: Instant) -> Result<(), WriteFailure> {
    // `Write` is implemented for `&TcpStream`, so no mutable handle is needed.
    let mut sink = stream;
    let mut written = 0usize;
    let fail = |written: usize, why: String| {
        if written == 0 {
            WriteFailure::Stalled
        } else {
            WriteFailure::Torn(why)
        }
    };
    while written < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(fail(written, "write deadline exceeded".into()));
        }
        if stream.set_write_timeout(Some(left)).is_err() {
            return Err(fail(written, "could not arm write timeout".into()));
        }
        match sink.write(&buf[written..]) {
            Ok(0) => return Err(fail(written, "peer closed during write".into())),
            Ok(n) => written += n,
            // Loop; the deadline check above decides when to give up.
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(fail(written, e.to_string())),
        }
    }
    Ok(())
}

/// `[len][node_id][listen_port:2]`.
///
/// The port matters: the accepting side sees only the dialer's *ephemeral
/// source* port, which is useless for dialling back once this connection
/// closes. Carrying the listening port is what makes "a peer that dialled us
/// is dialable back" (contract rule 5) actually true rather than nominally so.
fn send_hello(stream: &TcpStream, node_id: &str, listen_port: u16) -> Result<(), TransportError> {
    let bytes = node_id.as_bytes();
    let mut hello = Vec::with_capacity(3 + bytes.len());
    hello.push(bytes.len() as u8);
    hello.extend_from_slice(bytes);
    hello.extend_from_slice(&listen_port.to_be_bytes());
    // Deadline-bounded: a peer that accepts the connection and then stalls must
    // not hang `connect`, which the contract treats as an acceptance operation.
    write_all_by(stream, &hello, Instant::now() + HELLO_DEADLINE)
        .map_err(|_| TransportError::Io("peer did not accept the hello".into()))
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
    if super::is_valid_peer_id(id) {
        Ok(())
    } else {
        Err(unavailable(
            "id must be 1..=63 chars, alphanumeric or '-', not leading/trailing '-'",
        ))
    }
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
            // Keep the scope id: a link-local IPv6 peer is undialable without
            // the interface it arrived on (the same EINVAL that bit us in
            // dial_candidates).
            let dialable = addr.map(|a| match a {
                SocketAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(*v4.ip(), listen_port)),
                SocketAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(
                    *v6.ip(),
                    listen_port,
                    v6.flowinfo(),
                    v6.scope_id(),
                )),
            });
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
        self.alive()?;
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
            // that leaves both visible is not a rotation. If the withdrawal
            // fails we must NOT report success — a caller told the id rotated
            // would believe it unlinkable while the old name still answers.
            self.inner
                .mdns
                .unregister(&Self::fullname(&id))
                .map_err(unavailable)?;
            self.inner
                .registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&*id);
        }
        *id = new_id.to_string();
        // Keep the guard until the new name is published: dropping it here lets
        // a concurrent `start_advertising` read the new id and publish its own
        // payload first, which this call then overwrites.
        if let Some(payload) = payload {
            self.publish(new_id, &payload)?;
        }
        drop(id);
        Ok(())
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        self.alive()?;
        // Lock order: local_id before advertising (see `Inner`). Holding the
        // advertising guard across the mDNS call keeps start/stop linearizable
        // — otherwise a concurrent stop can unregister a record published a
        // moment later, leaving us silent while reporting that we advertise.
        let id = self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut advertising = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.publish(&id, &payload)?;
        *advertising = Some(payload);
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        self.alive()?;
        // Lock order: local_id before advertising (see `Inner`).
        let id = self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut advertising = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if advertising.is_some() {
            // Unregister first: reporting success while the old record still
            // answers is the failure mode this ordering exists to prevent.
            self.inner
                .mdns
                .unregister(&Self::fullname(&id))
                .map_err(unavailable)?;
            self.inner
                .registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&*id);
            *advertising = None;
        }
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        self.alive()?;
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
                        if peer == inner.id() {
                            continue; // never discover ourselves
                        }
                        // The name came off the network: hold it to the same
                        // rule as our own before it becomes a PeerId and
                        // travels up to the core.
                        if check_label(&peer).is_err() {
                            continue;
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
                        {
                            // Merge, don't replace: an address learned from an
                            // inbound dial (rule 5) or seeded by hand is still
                            // valid, and a later mDNS resolve must not drop the
                            // only candidate that actually works.
                            let mut peers = inner.peers.lock().unwrap_or_else(|e| e.into_inner());
                            let known = peers.entry(peer.clone()).or_default();
                            for addr in candidates {
                                if !known.contains(&addr) {
                                    known.push(addr);
                                }
                            }
                        }
                        inner.emit(TransportEvent::PeerFound { peer, payload });
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let peer = fullname.split('.').next().unwrap_or("").to_string();
                        if peer == inner.id() || check_label(&peer).is_err() {
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
        self.alive()?;
        let mut scanning = self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *scanning {
            // If the browse cannot be stopped we are still multicasting, so
            // reporting "scanning off" would be an F2 lie. Leave the flag set
            // and surface the failure.
            self.inner
                .mdns
                .stop_browse(SERVICE_TYPE)
                .map_err(unavailable)?;
            *scanning = false;
        }
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        self.alive()?;
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
        if let Err(e) = send_hello(&stream, &self.inner.id(), self.inner.port) {
            self.inner.emit(TransportEvent::PipeFailed {
                peer: peer.to_string(),
                why: e.to_string(),
            });
            return Err(e);
        }
        self.inner.adopt(peer.to_string(), stream, Some(addr), true);
        Ok(())
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        // Checked before the lookup: during shutdown the flag is set before the
        // pipes are drained, and without this a caller could still push bytes
        // onto a socket that is in the middle of being torn down.
        self.alive()?;
        let pipe = {
            let pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            pipes
                .get(peer)
                .map(|e| Arc::clone(&e.write))
                .ok_or_else(|| TransportError::NoSuchPeer(peer.to_string()))?
        };
        // Hold the per-pipe lock across the whole write so concurrent sends to
        // one peer can't interleave their bytes (contract rule 3).
        let stream = pipe.lock().unwrap_or_else(|e| e.into_inner());
        match write_all_by(&stream, bytes, Instant::now() + WRITE_TIMEOUT) {
            Ok(()) => Ok(()),
            // Nothing reached the wire, so a retry is safe — the only case that
            // may report WouldBlock.
            Err(WriteFailure::Stalled) => Err(TransportError::WouldBlock),
            // Bytes were already delivered. Retrying would duplicate a prefix
            // and desync the framer above us, so the pipe is unrecoverable:
            // tear it down and report a hard error instead of inviting a retry.
            Err(WriteFailure::Torn(why)) => {
                drop(stream);
                let _ = self.disconnect(peer);
                Err(TransportError::Io(why))
            }
        }
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        self.alive()?;
        let removed = {
            let mut pipes = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            let entry = pipes.remove(peer);
            if entry.is_some() {
                // Under the guard, so it cannot be reordered before a
                // concurrent adopt's PipeOpened.
                self.inner.emit(TransportEvent::PipeClosed {
                    peer: peer.to_string(),
                });
            }
            entry
        };
        if let Some(entry) = removed {
            // The teardown handle is never locked, so this aborts a write that
            // is currently blocked instead of queueing behind it.
            let _ = entry.teardown.shutdown(std::net::Shutdown::Both);
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
        // Not via the public methods: they refuse once the flag is set (and
        // rightly so). `mdns.shutdown()` below withdraws every registration and
        // ends the browse, so clearing local state is all that is left.
        *self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = false;
        let pipes: Vec<TcpStream> = {
            let mut guard = self.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
            guard.drain().map(|(_, e)| e.teardown).collect()
        };
        for s in pipes {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let _ = self.inner.mdns.shutdown();
        self.inner.revoke();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe must be probed for liveness, or a peer that vanishes without a FIN
    /// is never noticed: `read` blocks forever, `send` succeeds into a buffer
    /// that will never drain, and no `PipeClosed` is emitted. That is a real
    /// hardware failure, not a hypothetical — aeroplane mode reproduces it, and
    /// the symptom was a Ping that produced no outcome at all until the app was
    /// force-closed.
    ///
    /// Asserted on the socket rather than at the call site because the point is
    /// that the option reaches the file description both halves share. The
    /// interval is checked too: `SO_KEEPALIVE` alone inherits the kernel's
    /// two-hour idle default, which for a phone leaving a room is no better than
    /// never noticing.
    ///
    /// Lives here rather than in `tests/transport.rs` because it reads private
    /// state; an integration test would need a public accessor that exists for
    /// no other reason.
    #[test]
    fn an_adopted_pipe_is_probed_for_liveness() {
        let t = LanTransport::new("ka-local", Box::new(|_| {})).unwrap();

        // Act as a peer directly: this exercises the accept path, and `adopt` is
        // the shared funnel, so covering one direction covers both.
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, t.inner.port))
            .or_else(|_| TcpStream::connect((Ipv6Addr::LOCALHOST, t.inner.port)))
            .expect("the listener should accept a local dial");
        send_hello(&stream, "ka-peer", 4321).expect("hello should be accepted");

        // The hello is read on a worker, so the pipe appears asynchronously.
        // Cloning the handle out mirrors `send`: never hold the pipes lock
        // across the per-pipe one.
        let deadline = Instant::now() + Duration::from_secs(5);
        let write = loop {
            let found = {
                let pipes = t.inner.pipes.lock().unwrap_or_else(|e| e.into_inner());
                pipes.get("ka-peer").map(|e| Arc::clone(&e.write))
            };
            if let Some(w) = found {
                break w;
            }
            assert!(Instant::now() < deadline, "the pipe never opened");
            thread::sleep(Duration::from_millis(20));
        };

        let socket = write.lock().unwrap_or_else(|e| e.into_inner());
        let socket = socket2::SockRef::from(&*socket);
        assert!(
            socket.keepalive().unwrap_or(false),
            "an adopted pipe must have keepalive armed, or a vanished peer is never detected"
        );
        assert!(
            socket
                .tcp_keepalive_time()
                .expect("keepalive time readable")
                <= Duration::from_secs(60),
            "the kernel's default idle time is hours; a phone that leaves must surface in seconds"
        );
    }
}
