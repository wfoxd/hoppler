//! The transport layer (tech spec §4, §9 control plane; radio layer of R0-F2).
//!
//! One trait, many rungs. Every transport — LAN today, BLE and Wi-Fi Aware next
//! — offers the same capabilities the core needs: **advertise** a payload so
//! peers can find us, **scan** for peers advertising theirs, and open a **byte
//! pipe** to a chosen peer. Nothing above this layer knows which rung it is
//! riding (P3).
//!
//! Deliberate boundaries:
//! - **Bytes only, no protocol.** A transport never parses, frames, or
//!   interprets what it carries; Noise, envelopes, and framing live in Rust
//!   above it where they are testable. Native adapters stay correspondingly
//!   thin — which is why [`Transport::limits`] exists: a rung reports its
//!   geometry so the *core* can size frames, instead of a Kotlin adapter
//!   growing a fragmentation state machine no Rust test can reach.
//! - **The core owns policy.** Reconnection, rotation cadence, and who to dial
//!   are decisions for T09/T10; a transport reports what happened and does what
//!   it is told.
//!
//! # The contract every rung must honour
//!
//! These are the rules the conformance suite (`tests/transport.rs`) enforces
//! against *all* rungs, so the layer above genuinely cannot tell them apart.
//!
//! 1. **Events never fire on the caller's thread.** No `Transport` method may
//!    invoke the sink before it returns. The core calls `connect` while holding
//!    its own locks; a re-entrant `PipeOpened` would deadlock it.
//! 2. **`connect` returns `Ok` on *acceptance*, not completion.** A pipe is
//!    usable only once [`TransportEvent::PipeOpened`] arrives — which is always
//!    emitted, including when the pipe was already open. Failure to establish
//!    arrives as [`TransportEvent::PipeFailed`], never as `PipeClosed`.
//! 3. **`send` is atomic per call and ordered per pipe.** Concurrent sends to
//!    one peer never interleave their bytes. Message *boundaries* are not
//!    preserved (see [`TransportEvent::Received`]) — a rung may split or merge
//!    freely, and the LAN and BLE rungs both do.
//! 4. **A `PeerId` is stable while a pipe is open.** A rung must not rename a
//!    peer it holds a pipe to. When an advertisement rotates with no pipe open,
//!    emit `PeerLost` then `PeerFound` under the new id.
//! 5. **A peer that dialled us is dialable back**, without waiting to discover
//!    it.
//! 6. **After `shutdown` the rung is silent and inert.** No event reaches the
//!    sink once it returns. Idempotent; `Drop` implies it.

pub mod lan;
pub mod loopback;

use std::fmt;
use std::sync::Arc;

/// How a peer is named *within one transport*. Opaque above this layer — a LAN
/// instance name today, a rotating BLE advertisement id tomorrow. It is **not**
/// an identity: mapping a `PeerId` to a pseudonym or a Layer-1 key is the
/// session layer's job, after a handshake (tech spec §5).
///
/// Ids are only unique within a rung. A composite transport riding several
/// rungs at once (tech spec §9's ladder) must namespace them — `"ble:1f3a"` —
/// since the same device is reachable under a different id on each.
pub type PeerId = String;

/// What a rung can carry. Reported by [`Transport::limits`] so the core can
/// size advertisements and frames per rung instead of guessing against TCP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportLimits {
    /// Largest advertising payload this rung accepts. BLE legacy advertising
    /// leaves ~26 usable bytes after a 128-bit service UUID; extended
    /// advertising and LAN allow far more.
    pub max_advertising_payload: usize,
    /// The write size this rung is happiest with. Callers may send more — a
    /// rung fragments internally — but frames at or below this avoid the cost.
    pub preferred_write_size: usize,
}

/// Something a transport observed. Delivered to the sink given at construction,
/// always from a transport-owned thread (contract rule 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    /// A peer is advertising in range, carrying its current payload. Re-sent
    /// when the payload changes; treat it as "latest known", not one-shot.
    PeerFound { peer: PeerId, payload: Vec<u8> },
    /// A previously seen peer is gone (out of range, or stopped advertising).
    PeerLost { peer: PeerId },
    /// A byte pipe to `peer` is open — either we dialled or they did. Always
    /// emitted for a successful `connect`, including an already-open pipe.
    PipeOpened { peer: PeerId },
    /// A pipe could not be established. Distinct from `PipeClosed`, which only
    /// ever refers to a pipe that *was* open.
    PipeFailed { peer: PeerId, why: String },
    /// An open pipe closed: severed link, peer hung up, or we disconnected.
    PipeClosed { peer: PeerId },
    /// Bytes arrived on an open pipe. Delivery is reliable and ordered per
    /// pipe; boundaries are **not** preserved, so callers frame their own
    /// messages.
    Received { peer: PeerId, bytes: Vec<u8> },
    /// The rung became usable or unusable — Bluetooth toggled, permission
    /// revoked, network lost. Lets the UI say "Bluetooth is off" instead of
    /// showing an empty list that looks like "nobody is nearby" (F2).
    Availability {
        available: bool,
        reason: Option<String>,
    },
}

/// Errors a transport can report. Coarse by design — a caller learns that an
/// operation failed, not the radio's internal reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The rung is not usable (radio off, permission denied, no network).
    Unavailable(String),
    /// No pipe is open to that peer, or the peer is unknown.
    NoSuchPeer(PeerId),
    /// The advertising payload exceeds what this rung can carry.
    PayloadTooLarge { max: usize },
    /// The pipe's buffers are full. The caller should retry later; this is the
    /// backpressure signal a slow radio needs.
    WouldBlock,
    /// The operation failed at the link layer.
    Io(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Unavailable(why) => write!(f, "transport unavailable: {why}"),
            TransportError::NoSuchPeer(p) => write!(f, "no pipe to peer {p}"),
            TransportError::PayloadTooLarge { max } => {
                write!(f, "advertising payload exceeds {max} bytes")
            }
            TransportError::WouldBlock => write!(f, "pipe is full, retry later"),
            TransportError::Io(why) => write!(f, "link error: {why}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// The rule every rung applies to a local id and to any id arriving off the
/// network. Rungs must agree, or code developed against one picks ids another
/// refuses at runtime — and a `.` in particular splits differently on the two
/// ends of a pipe, yielding one connection under two `PeerId`s.
///
/// Constrained by the strictest rung: a DNS label (mDNS instance/host name).
pub fn is_valid_peer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 63
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Where a transport delivers its events. `Send + Sync` because transports
/// deliver from their own threads.
pub type EventSink = Box<dyn Fn(TransportEvent) + Send + Sync>;

/// A rung's internal handle on the sink. Shared rather than boxed so a delivery
/// thread can clone it out and *release* the lock before calling — a sink that
/// reacts by shutting the transport down must not deadlock against the
/// revocation lock.
pub(crate) type SharedSink = Arc<dyn Fn(TransportEvent) + Send + Sync>;

/// A rung of the transport ladder. See the module docs for the contract every
/// implementation must honour.
pub trait Transport: Send + Sync {
    /// Human-readable rung name, for logs and diagnostics (`"lan"`, `"ble"`).
    fn name(&self) -> &'static str;

    /// What this rung can carry (advertisement size, preferred write size).
    fn limits(&self) -> TransportLimits;

    /// Whether the rung is currently usable. Transitions arrive as
    /// [`TransportEvent::Availability`].
    fn is_available(&self) -> bool {
        true
    }

    /// Change how we appear to peers. Rotating the local id is how a rung
    /// implements unlinkability (tech spec §4): rotating only the payload while
    /// a stable name rides alongside it lets an observer link across rotations.
    /// Cadence is the core's decision.
    ///
    /// **Refused while any pipe is open** ([`TransportError::Unavailable`]).
    /// A connected peer knows us by the id it dialled, and rule 4 forbids
    /// renaming a peer under an open pipe — so the core rotates when idle. A
    /// future rung whose link layer decouples identity from the advertisement
    /// (BLE keeps a connection handle valid across RPA rotation) may relax
    /// this, but only by keeping connected peers' view unchanged.
    fn set_local_id(&self, id: &str) -> Result<(), TransportError>;

    /// Advertise `payload` so peers can discover us. Calling again replaces the
    /// payload and re-advertises. Fails with
    /// [`TransportError::PayloadTooLarge`] above [`TransportLimits`].
    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError>;

    /// Stop advertising. After this returns, the rung emits nothing
    /// discoverable — the F2 "toggle off is really off" guarantee.
    fn stop_advertising(&self) -> Result<(), TransportError>;

    /// Begin discovering peers; sightings arrive as `PeerFound` / `PeerLost`.
    fn start_scanning(&self) -> Result<(), TransportError>;

    /// Stop discovering. Open pipes are unaffected.
    fn stop_scanning(&self) -> Result<(), TransportError>;

    /// Ask for a byte pipe to a peer. Returns `Ok` on *acceptance*; the pipe is
    /// usable only once `PipeOpened` arrives (contract rule 2).
    fn connect(&self, peer: &str) -> Result<(), TransportError>;

    /// Send bytes on an open pipe. Atomic per call, ordered per pipe; not
    /// message-framed. May return [`TransportError::WouldBlock`].
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError>;

    /// Close a pipe. Emits `PipeClosed`. Closing an absent pipe is not an error.
    fn disconnect(&self, peer: &str) -> Result<(), TransportError>;

    /// Peers currently known — those discovered and not since lost, plus any
    /// that dialled us.
    ///
    /// **Not** cleared by [`Transport::stop_scanning`]: rule 5 depends on the
    /// record surviving, and a peer seen a moment ago is still reachable. The
    /// "show nothing while discovery is off" behaviour is a core-layer
    /// decision, not a transport one.
    fn peers(&self) -> Vec<PeerId>;

    /// Peers we currently hold an open pipe to.
    fn pipes(&self) -> Vec<PeerId>;

    /// Stop advertising and scanning, close every pipe, and revoke the sink.
    /// Idempotent.
    ///
    /// The guarantee is *silence*, not thread-joining: once this returns, no
    /// further event reaches the sink (a rung revokes it atomically). Worker
    /// threads may still be unwinding as their sockets close, but they can no
    /// longer be observed — which is what a caller tearing down its state
    /// actually needs.
    fn shutdown(&self);
}
