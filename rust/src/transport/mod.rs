//! The transport layer (tech spec §4, §9 control plane; radio layer of R0-F2).
//!
//! One trait, many rungs. Every transport — LAN today, BLE and Wi-Fi Aware next
//! — offers the same three capabilities the core needs: **advertise** a payload
//! so peers can find us, **scan** for peers advertising theirs, and open a
//! **byte pipe** to a chosen peer. Nothing above this layer knows which rung it
//! is riding (P3): the session layer (T10) speaks bytes, and `transports.rs`
//! picks the rung.
//!
//! Deliberate boundaries:
//! - **Bytes only, no protocol.** A transport never parses, frames, or
//!   interprets what it carries; Noise, envelopes, and framing live in Rust
//!   above it where they are testable. Native adapters stay correspondingly
//!   thin.
//! - **The core owns policy.** Reconnection, rotation cadence, and who to dial
//!   are decisions for T09/T10; a transport reports what happened and does what
//!   it is told.
//! - **Events, not callbacks into locks.** Everything asynchronous arrives as a
//!   [`TransportEvent`] on the sink handed to the constructor, so a transport
//!   never calls back into a core that might be holding a lock.

pub mod lan;
pub mod loopback;

use std::fmt;

/// How a peer is named *within one transport*. Opaque above this layer — a LAN
/// instance name today, a rotating BLE payload id tomorrow. It is **not** an
/// identity: mapping a `PeerId` to a pseudonym or a Layer-1 key is the session
/// layer's job, after a handshake (tech spec §5).
pub type PeerId = String;

/// Something a transport observed. Delivered to the sink given at construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    /// A peer began advertising in range, carrying its current payload.
    PeerFound { peer: PeerId, payload: Vec<u8> },
    /// A previously seen peer is gone (out of range, or stopped advertising).
    PeerLost { peer: PeerId },
    /// A byte pipe to `peer` is open — either we dialled or they did.
    PipeOpened { peer: PeerId },
    /// The pipe closed: severed link, peer hung up, or we disconnected.
    PipeClosed { peer: PeerId },
    /// Bytes arrived on an open pipe. Delivery is reliable and ordered per
    /// pipe; boundaries are **not** preserved, so callers frame their own
    /// messages.
    Received { peer: PeerId, bytes: Vec<u8> },
}

/// Errors a transport can report. Coarse by design — a caller learns that an
/// operation failed, not the radio's internal reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The transport is not usable (radio off, permission denied, no network).
    Unavailable(String),
    /// No pipe is open to that peer, or the peer is unknown.
    NoSuchPeer(PeerId),
    /// The operation failed at the link layer.
    Io(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Unavailable(why) => write!(f, "transport unavailable: {why}"),
            TransportError::NoSuchPeer(p) => write!(f, "no pipe to peer {p}"),
            TransportError::Io(why) => write!(f, "link error: {why}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Where a transport delivers its events. `Send + Sync` because transports
/// deliver from their own threads.
pub type EventSink = Box<dyn Fn(TransportEvent) + Send + Sync>;

/// A rung of the transport ladder.
///
/// Implementations must be safe to call from any thread, and must be inert
/// after [`Transport::shutdown`] — releasing radios, sockets, and threads, and
/// emitting no further events. Dropping a transport implies shutdown.
pub trait Transport: Send + Sync {
    /// Human-readable rung name, for logs and diagnostics (`"lan"`, `"ble"`).
    fn name(&self) -> &'static str;

    /// Advertise `payload` so peers can discover us. Calling again replaces the
    /// payload and re-advertises — this is the rotation hook the core drives
    /// (tech spec §4); the *cadence* is the core's decision, not ours.
    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError>;

    /// Stop advertising. After this returns, the transport emits nothing
    /// discoverable — the F2 "toggle off is really off" guarantee.
    fn stop_advertising(&self) -> Result<(), TransportError>;

    /// Begin discovering peers; sightings arrive as `PeerFound` / `PeerLost`.
    fn start_scanning(&self) -> Result<(), TransportError>;

    /// Stop discovering. Open pipes are unaffected.
    fn stop_scanning(&self) -> Result<(), TransportError>;

    /// Open a byte pipe to a discovered peer. Completion arrives as
    /// `PipeOpened`; failure as an `Err` or a `PipeClosed`.
    fn connect(&self, peer: &str) -> Result<(), TransportError>;

    /// Send bytes on an open pipe. Ordered and reliable; not message-framed.
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError>;

    /// Close a pipe. Emits `PipeClosed`. Closing an absent pipe is not an error.
    fn disconnect(&self, peer: &str) -> Result<(), TransportError>;

    /// Release everything: stop advertising and scanning, close pipes, join
    /// threads. Idempotent.
    fn shutdown(&self);
}
