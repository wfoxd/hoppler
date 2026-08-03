//! The BLE rung (T08b) — tech spec §4, §9; radio layer of R0-F2.
//!
//! BLE is the only rung that cannot be exercised in-process: it needs two
//! radios. So this module is deliberately split in two.
//!
//! - **[`BleTransport`] — the whole contract, in Rust.** Peer and pipe
//!   bookkeeping, event ordering, rotation refusal, shutdown inertness, and
//!   backpressure all live here, where the conformance suite reaches them.
//! - **[`BlePlatform`] — the radio, and nothing else.** Six commands out, a
//!   handful of facts back. The Android adapter implements exactly this and
//!   holds no state the contract depends on.
//!
//! That split is the point. "Keep adapters thin: bytes only" is easy to write
//! and easy to lose: every rule the adapter is trusted to honour is a rule no
//! Rust test can check, on the one rung where a failing test is expensive to
//! even observe. Here the adapter cannot violate rule 2 or 6 — it is not the
//! component that decides them.
//!
//! # What the platform is *not* trusted with
//!
//! - **Emitting `PipeOpened` for an already-open pipe** (rule 2). The adapter
//!   reports what the radio did; re-announcing an existing pipe is this
//!   module's job.
//! - **Silence after shutdown** (rule 6). Late events from a radio that has not
//!   finished tearing down are expected, and dropped here.
//! - **Refusing to rotate under an open pipe** (rule 4).
//! - **`PipeClosed` for a pipe that never opened** (rule 2). Turned into
//!   `PipeFailed`, because that is what the core acts on. We deliberately do
//!   *not* track which peers we dialled in order to tell a failed dial from an
//!   unsolicited close: that is state the adapter's own reconnection would keep
//!   invalidating, and a `PipeFailed` for a peer we hold no pipe to costs the
//!   core nothing — whereas a `PipeClosed` for a pipe it never opened would
//!   have it tearing down state it never built.
//!
//!   One qualifier, learned the hard way: a `PipeFailed` while a pipe *is*
//!   live is no longer free. The engine queues a Ping until a session exists,
//!   so a spurious failure cancels it and reports it undeliverable moments
//!   before it would have been delivered. Suppressed below.
//!
//! An adapter that gets any of these wrong is still correct from the core's
//! side, which is what makes per-OEM BLE quirks survivable.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use super::{
    is_valid_peer_id, EventSink, PeerId, SharedSink, Transport, TransportError, TransportEvent,
    TransportLimits,
};

/// What the core may put in an advertisement, after the adapter's framing.
///
/// The budget, worst case, on the smallest extended-advertising buffer that
/// ships (`getLeMaximumAdvertisingDataLength()` reports 191 on budget hardware):
///
/// ```text
///   191  extended advertising data
///  - 18  service-data AD structure (1 len + 1 type + 16 UUID)
///  -  2  L2CAP PSM, so a scanner can dial without a GATT round-trip
///  -  1  id length
///  - 63  local id (the longest `is_valid_peer_id` permits)
///  = 107 left for the core
/// ```
///
/// Rounded down to 100. Sizing to the *floor* rather than to a flagship is the
/// difference between a payload the core composes freely and one the radio
/// silently refuses on somebody's cheap phone.
const MAX_ADVERTISING_PAYLOAD: usize = 100;

/// L2CAP CoC MTU in practice. The core frames to this so the adapter never
/// grows a fragmentation state machine (see the module docs on `limits`).
const PREFERRED_WRITE_SIZE: usize = 240;

/// How many unacknowledged bytes may be outstanding on one pipe before `send`
/// reports [`TransportError::WouldBlock`].
///
/// A radio is orders of magnitude slower than the code feeding it, so without a
/// window a sender fills an unbounded queue in the adapter and the first
/// evidence is an OOM on a phone. The credit is returned by
/// [`BleTransport::on_write_complete`], which is the adapter's *only*
/// obligation beyond delivering events.
const SEND_WINDOW: usize = 64 * 1024;

/// What the Rust core asks the radio to do. Everything here is
/// fire-and-forget: results arrive as [`PlatformEvent`]s, because a BLE
/// operation that "succeeded" synchronously has usually only been queued.
///
/// Deliberately six commands. Each maps to one Android API call.
pub trait BlePlatform: Send + Sync {
    /// Tell the adapter how we are named. Called at construction and on every
    /// rotation, *not* only when advertising: a node with Discovery off still
    /// dials paired peers (R0-F2) and must introduce itself by the id the core
    /// knows it by, never by its MAC.
    fn set_local_id(&self, local_id: &str) -> Result<(), TransportError>;
    /// Advertise `payload` under the current local id. Replaces any current
    /// advertisement.
    fn start_advertising(&self, payload: &[u8]) -> Result<(), TransportError>;
    /// Stop advertising. Must be a real radio stop, not a flag (F2).
    fn stop_advertising(&self) -> Result<(), TransportError>;
    /// Begin scanning; sightings come back as `PeerFound`/`PeerLost`.
    fn start_scanning(&self) -> Result<(), TransportError>;
    /// Stop scanning. Open pipes are unaffected.
    fn stop_scanning(&self) -> Result<(), TransportError>;
    /// Open an L2CAP CoC (or GATT fallback) to `peer`. Acceptance only —
    /// success arrives as `PipeOpened`, failure as `PipeFailed`.
    fn connect(&self, peer: &str) -> Result<(), TransportError>;
    /// Write bytes to an open pipe. The adapter must preserve order and must
    /// call back [`BleTransport::on_write_complete`] once the bytes are gone.
    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError>;
    /// Close one pipe.
    fn disconnect(&self, peer: &str) -> Result<(), TransportError>;
    /// Release every radio resource. Late events after this are tolerated.
    fn shutdown(&self);
}

/// A fact from the radio. Narrower than [`TransportEvent`]: the platform
/// reports what happened, and [`BleTransport`] decides what the core is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    /// A peer's advertisement was seen, with its current payload.
    PeerFound { peer: PeerId, payload: Vec<u8> },
    /// A peer's advertisement has aged out.
    PeerLost { peer: PeerId },
    /// A pipe is established — either direction.
    PipeOpened { peer: PeerId },
    /// A dial failed.
    PipeFailed { peer: PeerId, why: String },
    /// A pipe dropped.
    PipeClosed { peer: PeerId },
    /// Bytes arrived.
    Received { peer: PeerId, bytes: Vec<u8> },
    /// The radio became usable or unusable (Bluetooth toggled, permission
    /// revoked). Reported rather than inferred, so the UI can say why (F2).
    Availability {
        available: bool,
        reason: Option<String>,
    },
}

/// Give back window credit that was charged but never spent.
///
/// Saturating, and it has to be: `on_write_complete` also saturates at zero, so
/// an adapter that acknowledges more than it was given can empty the counter
/// between the charge and the refund. A plain `fetch_sub` would then wrap to
/// near `usize::MAX` and the pipe would report `WouldBlock` forever — a
/// permanent wedge from a transient over-count.
fn refund(pipe: &Pipe, bytes: usize) {
    let _ = pipe
        .outstanding
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            Some(n.saturating_sub(bytes))
        });
}

struct Pipe {
    /// Serialises writes to this pipe so concurrent sends cannot interleave
    /// (rule 3). Held only for the duration of one `platform.send`.
    write: Mutex<()>,
    /// Bytes handed to the adapter and not yet acknowledged. Atomic, and the
    /// pipes map is *not* locked while a write is in flight, because an adapter
    /// may acknowledge synchronously from inside `send` — a fast inline write
    /// does exactly that, and a held map lock would deadlock the core against
    /// its own adapter.
    outstanding: AtomicUsize,
}

/// Lock order, where more than one is held: **`local_id` → `advertising` →
/// `scanning` → `peers` → `pipes`**.
///
/// The LAN rung learned this the expensive way (see
/// `docs/ring0/findings/T08-transport-findings.md`): reading a value out of one
/// lock and releasing it before taking the next removes the deadlock *and* the
/// atomicity, which loses a rotation instead of hanging. Where two are needed,
/// hold both.
struct Inner {
    local_id: Mutex<PeerId>,
    advertising: Mutex<Option<Vec<u8>>>,
    scanning: Mutex<bool>,
    peers: Mutex<HashSet<PeerId>>,
    pipes: Mutex<HashMap<PeerId, Arc<Pipe>>>,
    /// Revoked on shutdown under a write lock, so suppression is atomic with
    /// the flag rather than racing it (rule 6).
    sink: RwLock<Option<SharedSink>>,
    /// Set before the write lock is taken, so a delivery that already holds the
    /// read guard still suppresses — and so a sink that shuts us down from
    /// inside itself does not deadlock waiting for its own guard.
    revoked: AtomicBool,
    dispatch_thread: OnceLock<std::thread::ThreadId>,
    tx: Mutex<Option<Sender<TransportEvent>>>,
    dead: AtomicBool,
    available: AtomicBool,
    platform: Arc<dyn BlePlatform>,
}

impl Inner {
    fn alive(&self) -> Result<(), TransportError> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(TransportError::Unavailable("transport is shut down".into()));
        }
        Ok(())
    }

    /// Stop delivering, atomically with respect to a delivery in flight.
    fn revoke(&self) {
        // Ordered first: a dispatch already holding the read guard sees this
        // even though the write lock below cannot be taken until it finishes.
        self.revoked.store(true, Ordering::SeqCst);
        // Skip the write lock when the sink itself called shutdown — we would
        // be waiting on a read guard held by this very thread.
        if self.dispatch_thread.get() != Some(&std::thread::current().id()) {
            *self.sink.write().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// Queue an event for the dispatch thread. Never calls the sink inline —
    /// rule 1 — so a platform callback that arrives on the radio's own thread
    /// cannot re-enter the core.
    fn emit(&self, event: TransportEvent) {
        if self.dead.load(Ordering::SeqCst) {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(event);
        }
    }
}

/// The BLE rung. See the module docs for how work is divided with the adapter.
pub struct BleTransport {
    inner: Arc<Inner>,
}

impl BleTransport {
    /// Wire a platform adapter to a sink. The returned transport owns a
    /// dispatch thread; events reach `sink` only from it.
    pub fn new(
        local_id: &str,
        platform: Arc<dyn BlePlatform>,
        sink: EventSink,
    ) -> Result<Self, TransportError> {
        if !is_valid_peer_id(local_id) {
            return Err(TransportError::Io(format!("invalid local id: {local_id}")));
        }
        platform.set_local_id(local_id)?;
        let (tx, rx) = channel::<TransportEvent>();
        let inner = Arc::new(Inner {
            local_id: Mutex::new(local_id.to_string()),
            advertising: Mutex::new(None),
            scanning: Mutex::new(false),
            peers: Mutex::new(HashSet::new()),
            pipes: Mutex::new(HashMap::new()),
            sink: RwLock::new(Some(Arc::from(sink))),
            revoked: AtomicBool::new(false),
            dispatch_thread: OnceLock::new(),
            tx: Mutex::new(Some(tx)),
            dead: AtomicBool::new(false),
            available: AtomicBool::new(true),
            platform,
        });

        // One dispatch thread per transport: the sink is called from here and
        // nowhere else, so ordering is total and no radio thread is blocked by
        // a slow core.
        let dispatch = inner.clone();
        std::thread::Builder::new()
            .name("hoppler-ble-dispatch".into())
            .spawn(move || {
                let _ = dispatch.dispatch_thread.set(std::thread::current().id());
                while let Ok(event) = rx.recv() {
                    // The guard is held ACROSS the call, so a `shutdown` on
                    // another thread blocks until this delivery finishes and
                    // only then revokes — which is what makes "silent once
                    // shutdown returns" exact rather than best-effort (rule 6).
                    // Cloning the sink out and releasing first would let
                    // shutdown return while this call was still running.
                    let guard = dispatch.sink.read().unwrap_or_else(|e| e.into_inner());
                    if dispatch.revoked.load(Ordering::SeqCst) {
                        continue;
                    }
                    if let Some(sink) = guard.as_ref() {
                        sink(event);
                    }
                }
            })
            .map_err(|e| TransportError::Io(format!("dispatch thread: {e}")))?;

        Ok(Self { inner })
    }

    /// A cloneable handle for pushing radio observations in.
    ///
    /// The adapter must not own the transport — the core does — so this is the
    /// only thing it holds. It keeps working after the transport is dropped
    /// (events are simply discarded), which spares the adapter a teardown
    /// barrier it has no good way to implement.
    pub fn ingress(&self) -> BleIngress {
        BleIngress {
            inner: Arc::downgrade(&self.inner),
        }
    }
}

/// The adapter's side of the seam: push observations, return send credit.
///
/// Holds a [`Weak`] reference deliberately. `Inner` owns the platform and the
/// platform holds this, so an owning handle would be a reference cycle that
/// never frees the transport — and the weak reference is also exactly the
/// "discard events once the transport is gone" behaviour the adapter needs.
#[derive(Clone)]
pub struct BleIngress {
    inner: Weak<Inner>,
}

impl BleIngress {
    /// Feed a radio observation in, from whatever thread the platform uses.
    ///
    /// Everything the contract promises but a radio cannot is decided here.
    pub fn on_platform_event(&self, event: PlatformEvent) {
        // Gone means the core dropped the transport; the radio may still be
        // tearing down and reporting. Discarding here is why the adapter needs
        // no teardown barrier of its own.
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // Rule 6: shut down is silent, even for events already in flight.
        if inner.dead.load(Ordering::SeqCst) {
            return;
        }
        match event {
            PlatformEvent::PeerFound { peer, payload } => {
                // A radio id that no other rung would accept must not reach the
                // core: ids cross rungs in composite form (`"ble:1f3a"`) and a
                // malformed one desyncs the two ends of a pipe.
                if !is_valid_peer_id(&peer) {
                    return;
                }
                inner
                    .peers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.clone());
                inner.emit(TransportEvent::PeerFound { peer, payload });
            }
            PlatformEvent::PeerLost { peer } => {
                // A peer we hold a pipe to is still reachable whatever the
                // advertisement says — forgetting it would break rule 5's
                // dial-back for a peer that is, right now, connected.
                let has_pipe = inner
                    .pipes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&peer);
                if !has_pipe {
                    inner
                        .peers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&peer);
                }
                inner.emit(TransportEvent::PeerLost { peer });
            }
            PlatformEvent::PipeOpened { peer } => {
                if !is_valid_peer_id(&peer) {
                    return;
                }
                // Record the peer before announcing the pipe, so a core woken
                // by PipeOpened never sees a peer list that omits it.
                inner
                    .peers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(peer.clone());
                let fresh = inner
                    .pipes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        peer.clone(),
                        Arc::new(Pipe {
                            write: Mutex::new(()),
                            outstanding: AtomicUsize::new(0),
                        }),
                    )
                    .is_none();
                // Both ends dialling at once is routine on BLE and yields two
                // opens for one pipe. A second PipeOpened would have the core
                // counting two pipes and closing one of them.
                if fresh {
                    inner.emit(TransportEvent::PipeOpened { peer });
                }
            }
            PlatformEvent::PipeFailed { peer, why } => {
                // Not while a pipe to them is live. Both ends dial at once
                // often enough that one connection losing the race is normal,
                // and the adapter reports that loser's closure as a failure of
                // the *peer* — which it is not, because the survivor is
                // carrying traffic.
                //
                // `PipeOpened` above already dedupes for exactly this reason.
                // This did not, on the stated grounds that "an unsolicited
                // PipeFailed costs the core nothing" — true when written, and
                // false since the engine began queueing a Ping until a session
                // exists: a spurious failure now cancels that Ping and tells
                // the person it could not be delivered, moments before it is.
                // Observed on two phones, first Ping every time.
                let live = inner
                    .pipes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&peer);
                if live {
                    log::info!("ignoring a pipe failure for {peer}: a pipe is live ({why})");
                } else {
                    inner.emit(TransportEvent::PipeFailed { peer, why });
                }
            }
            PlatformEvent::PipeClosed { peer } => {
                let existed = inner
                    .pipes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&peer)
                    .is_some();
                if existed {
                    inner.emit(TransportEvent::PipeClosed { peer });
                } else {
                    // Rule 2: PipeClosed refers only to a pipe that was open. A
                    // radio reporting a disconnect for a dial that never
                    // completed is reporting a failure, and the core acts on
                    // the two very differently.
                    inner.emit(TransportEvent::PipeFailed {
                        peer,
                        why: "link closed before the pipe opened".into(),
                    });
                }
            }
            PlatformEvent::Received { peer, bytes } => {
                // Bytes on a pipe we no longer consider open would arrive after
                // PipeClosed, which the core is entitled to treat as impossible.
                let open = inner
                    .pipes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&peer);
                if open {
                    inner.emit(TransportEvent::Received { peer, bytes });
                }
            }
            PlatformEvent::Availability { available, reason } => {
                let was = inner.available.swap(available, Ordering::SeqCst);
                // A radio that has come back has come back empty, and only this
                // rung knows what it was doing before. See `rearm`.
                if available && !was {
                    rearm(&inner);
                }
                inner.emit(TransportEvent::Availability { available, reason });
            }
        }
    }

    /// Whether the transport behind this handle still exists. An adapter can
    /// use it to stop feeding a radio whose core has gone away.
    pub fn is_live(&self) -> bool {
        self.inner.strong_count() > 0
    }

    /// Return send credit once the adapter has actually written `bytes` to the
    /// radio. Without this the window closes and `send` reports `WouldBlock`
    /// forever.
    pub fn on_write_complete(&self, peer: &str, bytes: usize) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // Clone the entry out and release the map lock before touching the
        // counter: this may be called from inside `platform.send`.
        let pipe = inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned();
        if let Some(pipe) = pipe {
            let _ = pipe
                .outstanding
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_sub(bytes))
                });
        }
    }
}

/// Put the radio back the way this rung already believes it is.
///
/// A radio that becomes available again comes back **blank**. An advertising
/// set and a scan callback are registrations in the Bluetooth stack, and
/// toggling Bluetooth destroys both; so does starting up without the permission
/// to create them and being granted it later. Nothing downstream notices,
/// because every piece of state here still says we are advertising and
/// scanning — `start_scanning` is a no-op while `scanning` is already true.
///
/// Without this, Bluetooth off-and-on is permanent: the rung reports itself
/// available, advertises nothing, hears nothing, and only a restart fixes it.
/// That is acceptance checks 5 and 7 both failing, silently.
///
/// It re-applies remembered state rather than inventing any, which is why it
/// cannot resurrect something the core meant to stop: Discovery off clears
/// `advertising` and `scanning` first, so a radio coming back under it stays
/// dark (R0-F2).
///
/// Failures are logged, not propagated. There is no caller to return them to,
/// and the radio has just said it is available — a refusal means it changed its
/// mind, and another `Availability` is already on its way.
fn rearm(inner: &Inner) {
    // Lock order: local_id before advertising, as everywhere else in this file.
    // Both held across the re-advertise so a rotation racing this cannot
    // publish the old name.
    let local = inner.local_id.lock().unwrap_or_else(|e| e.into_inner());
    let advertising = inner.advertising.lock().unwrap_or_else(|e| e.into_inner());
    // The adapter keeps the local id in a plain field, so it survives the radio
    // — but a host that restarted, or one that rejected the id while
    // unpermitted, has not got it. Re-sending is idempotent and cheap; guessing
    // which host we have is neither.
    if let Err(why) = inner.platform.set_local_id(&local) {
        log::warn!("could not re-name the radio after it came back: {why}");
    }
    if let Some(payload) = advertising.as_ref() {
        if let Err(why) = inner.platform.start_advertising(payload) {
            log::warn!("could not re-advertise after the radio came back: {why}");
        }
    }
    drop(advertising);
    drop(local);

    if *inner.scanning.lock().unwrap_or_else(|e| e.into_inner()) {
        if let Err(why) = inner.platform.start_scanning() {
            log::warn!("could not resume scanning after the radio came back: {why}");
        }
    }
}

impl Transport for BleTransport {
    fn name(&self) -> &'static str {
        "ble"
    }

    fn limits(&self) -> TransportLimits {
        TransportLimits {
            max_advertising_payload: MAX_ADVERTISING_PAYLOAD,
            preferred_write_size: PREFERRED_WRITE_SIZE,
        }
    }

    fn is_available(&self) -> bool {
        !self.inner.dead.load(Ordering::SeqCst) && self.inner.available.load(Ordering::SeqCst)
    }

    fn set_local_id(&self, id: &str) -> Result<(), TransportError> {
        self.inner.alive()?;
        if !is_valid_peer_id(id) {
            return Err(TransportError::Io(format!("invalid local id: {id}")));
        }
        // Rule 4. BLE could in principle rotate under an open pipe — an L2CAP
        // channel survives RPA rotation — but only by keeping the connected
        // peer's view unchanged, which needs a second name space. Ring 0
        // rotates when idle, like every other rung.
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
        // Lock order: local_id before advertising. Both held across the
        // re-advertise so a concurrent start cannot publish the old id after
        // this one has moved on.
        let mut local = self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *local == id {
            return Ok(());
        }
        let advertising = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(payload) = advertising.as_ref() {
            // Withdraw the old name before publishing the new one: a rotation
            // that leaves both visible is not a rotation (F2).
            self.inner.platform.stop_advertising()?;
            self.inner.platform.set_local_id(id)?;
            *local = id.to_string();
            self.inner.platform.start_advertising(payload)?;
        } else {
            self.inner.platform.set_local_id(id)?;
            *local = id.to_string();
        }
        Ok(())
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        self.inner.alive()?;
        if payload.len() > MAX_ADVERTISING_PAYLOAD {
            return Err(TransportError::PayloadTooLarge {
                max: MAX_ADVERTISING_PAYLOAD,
            });
        }
        // Lock order: local_id before advertising. Held together so a
        // concurrent rotation cannot publish this payload under the old name.
        let _local = self
            .inner
            .local_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut advertising = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.inner.platform.start_advertising(&payload)?;
        *advertising = Some(payload);
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        self.inner.alive()?;
        let mut advertising = self
            .inner
            .advertising
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if advertising.is_some() {
            // Do not report success for a stop the radio refused: a caller told
            // it is invisible while still advertising is exactly the F2 failure
            // this rung exists to avoid.
            self.inner.platform.stop_advertising()?;
            *advertising = None;
        }
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        self.inner.alive()?;
        let mut scanning = self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !*scanning {
            self.inner.platform.start_scanning()?;
            *scanning = true;
        }
        Ok(())
    }

    fn stop_scanning(&self) -> Result<(), TransportError> {
        self.inner.alive()?;
        let mut scanning = self
            .inner
            .scanning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *scanning {
            self.inner.platform.stop_scanning()?;
            *scanning = false;
        }
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        self.inner.alive()?;
        // Rule 2: an already-open pipe still announces itself, so a caller
        // awaiting PipeOpened never hangs. Decided here, not in the adapter.
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
        if !self
            .inner
            .peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(peer)
        {
            self.inner.emit(TransportEvent::PipeFailed {
                peer: peer.to_string(),
                why: "unknown peer".into(),
            });
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        match self.inner.platform.connect(peer) {
            Ok(()) => Ok(()),
            Err(e) => {
                // A refused dial is a PipeFailed, never a PipeClosed (rule 2).
                self.inner.emit(TransportEvent::PipeFailed {
                    peer: peer.to_string(),
                    why: e.to_string(),
                });
                Err(e)
            }
        }
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        self.inner.alive()?;
        // Take the entry, then drop the map lock. The write itself is
        // serialised by the pipe's own mutex, so concurrent sends to one peer
        // cannot interleave (rule 3) without the map being held across a call
        // into the adapter.
        let pipe = self
            .inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned()
            .ok_or_else(|| TransportError::NoSuchPeer(peer.to_string()))?;

        // Charge the window before the write: the adapter may acknowledge
        // before `send` returns, so the credit has to exist to be returned.
        let before = pipe.outstanding.fetch_add(bytes.len(), Ordering::SeqCst);
        if before.saturating_add(bytes.len()) > SEND_WINDOW {
            refund(&pipe, bytes.len());
            return Err(TransportError::WouldBlock);
        }

        let _write = pipe.write.lock().unwrap_or_else(|e| e.into_inner());
        match self.inner.platform.send(peer, bytes) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Nothing reached the radio, so keeping the credit would shrink
                // the window permanently.
                refund(&pipe, bytes.len());
                Err(e)
            }
        }
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        self.inner.alive()?;
        let existed = self
            .inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .is_some();
        if existed {
            // Best effort: the pipe is gone from our side whatever the radio
            // says, and the core has been told. A failure here would leave the
            // two views disagreeing.
            let _ = self.inner.platform.disconnect(peer);
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
            .iter()
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
        if self.inner.dead.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.platform.shutdown();
        self.inner
            .pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
        // Waits for any delivery already in flight, then revokes.
        self.inner.revoke();
        // Drop the sender so the dispatch thread's recv fails and it exits.
        *self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Drop for BleTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A radio that records what it was asked to do.
    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn clear(&self) {
            self.calls.lock().unwrap().clear();
        }
        fn note(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }
    }

    impl BlePlatform for Recorder {
        fn set_local_id(&self, id: &str) -> Result<(), TransportError> {
            self.note(format!("set_local_id({id})"));
            Ok(())
        }
        fn start_advertising(&self, payload: &[u8]) -> Result<(), TransportError> {
            self.note(format!("start_advertising({payload:?})"));
            Ok(())
        }
        fn stop_advertising(&self) -> Result<(), TransportError> {
            self.note("stop_advertising");
            Ok(())
        }
        fn start_scanning(&self) -> Result<(), TransportError> {
            self.note("start_scanning");
            Ok(())
        }
        fn stop_scanning(&self) -> Result<(), TransportError> {
            self.note("stop_scanning");
            Ok(())
        }
        fn connect(&self, _: &str) -> Result<(), TransportError> {
            Ok(())
        }
        fn send(&self, _: &str, _: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
        fn disconnect(&self, _: &str) -> Result<(), TransportError> {
            Ok(())
        }
        fn shutdown(&self) {}
    }

    /// A transport over a recording radio, with construction's own calls
    /// cleared — every test here is about what happens afterwards.
    fn transport() -> (BleTransport, Arc<Recorder>) {
        let radio = Arc::new(Recorder::default());
        let t = BleTransport::new("a", radio.clone(), Box::new(|_| {})).unwrap();
        radio.clear();
        (t, radio)
    }

    fn radio_is(available: bool) -> PlatformEvent {
        PlatformEvent::Availability {
            available,
            reason: if available {
                None
            } else {
                Some("Bluetooth is off".into())
            },
        }
    }

    /// `is_available` is what the UI uses to say "Bluetooth is off" rather than
    /// showing an empty list that reads as "nobody is nearby" (F2), so it must
    /// track the radio and not merely the shutdown flag.
    #[test]
    fn availability_tracks_the_radio() {
        let (t, _) = transport();
        let radio = t.ingress();
        assert!(t.is_available());
        radio.on_platform_event(PlatformEvent::Availability {
            available: false,
            reason: Some("bluetooth off".into()),
        });
        assert!(!t.is_available(), "a radio that went away is not available");
        radio.on_platform_event(PlatformEvent::Availability {
            available: true,
            reason: None,
        });
        assert!(t.is_available());
        t.shutdown();
        // A shut-down rung is not available whatever the radio last said.
        assert!(!t.is_available());
    }

    /// A radio that comes back comes back **blank**: the advertising set and
    /// the scan callback are registrations in the Bluetooth stack, and toggling
    /// Bluetooth destroys both. Nothing downstream notices, because every flag
    /// here still says we are advertising and scanning — which makes an
    /// off-and-on toggle permanent until the app is restarted, and acceptance
    /// checks 5 and 7 unpassable.
    #[test]
    fn a_radio_that_comes_back_is_put_back_to_work() {
        let (t, radio) = transport();
        t.start_advertising(vec![7, 7]).unwrap();
        t.start_scanning().unwrap();
        let ingress = t.ingress();

        ingress.on_platform_event(radio_is(false));
        radio.clear();
        ingress.on_platform_event(radio_is(true));

        let calls = radio.calls();
        assert!(
            calls.contains(&"start_advertising([7, 7])".to_string()),
            "the same advertisement must go back up, got {calls:?}"
        );
        assert!(
            calls.contains(&"start_scanning".to_string()),
            "scanning must resume, got {calls:?}"
        );
    }

    /// The re-arm re-applies what the rung *remembers*, never what it once did.
    /// Discovery off clears both flags first, so a radio returning while hidden
    /// must stay dark — a re-arm that resurrected the last advertisement would
    /// put a hidden node back on the air, which is precisely the R0-F2 failure
    /// the Discovery switch exists to prevent.
    #[test]
    fn a_radio_that_comes_back_under_discovery_off_stays_dark() {
        let (t, radio) = transport();
        t.start_advertising(vec![7, 7]).unwrap();
        t.start_scanning().unwrap();
        t.stop_advertising().unwrap();
        t.stop_scanning().unwrap();
        let ingress = t.ingress();

        ingress.on_platform_event(radio_is(false));
        radio.clear();
        ingress.on_platform_event(radio_is(true));

        let calls = radio.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("start_advertising")),
            "a hidden node must not come back on the air, got {calls:?}"
        );
        assert!(
            !calls.contains(&"start_scanning".to_string()),
            "scanning was stopped and must stay stopped, got {calls:?}"
        );
    }

    /// Only a *change* re-arms.
    ///
    /// The adapter reports availability whenever the UI subscribes, and again
    /// after a failed scan. Re-arming on each one would matter: our own
    /// `start_advertising` stops the current set before starting the next, so
    /// every redundant report would blink the advertisement off and on — which
    /// a sniffer sees and a peer mid-dial may not survive.
    #[test]
    fn an_availability_report_that_changes_nothing_does_not_re_arm() {
        let (t, radio) = transport();
        t.start_advertising(vec![1]).unwrap();
        t.start_scanning().unwrap();
        let ingress = t.ingress();
        radio.clear();

        ingress.on_platform_event(radio_is(true));

        assert!(
            radio.calls().is_empty(),
            "nothing changed, so nothing should have been re-issued, got {:?}",
            radio.calls()
        );
    }
}
