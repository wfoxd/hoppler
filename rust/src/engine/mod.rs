//! The core engine behind the bridge (`crate::api`). Holds process-wide state —
//! identity, store, and the event sink — and runs the six API areas' operations
//! against the real store and identity, with a fake network standing in for the
//! radio plane (T08–T10). Not scanned by flutter_rust_bridge (only `crate::api`
//! is), so it stays free of bridge concerns.
//!
//! Dev keystore caveat: the store's master is held in an in-memory
//! `SoftwareKeystore`, so identity does not survive a process restart yet — the
//! native Secure Enclave / StrongBox backend (deferred with T05/T08) fixes that.
//! Everything within a session persists to the real encrypted store.
//!
//! The fake network ([`fake`]) is the Ring-0 implementation, not a test double:
//! it ships in this build because no real transport exists yet. When T08–T10
//! land, they replace it behind this same `crate::api` surface, and the
//! contacts it writes (with placeholder Layer-2 keys) must be superseded by
//! real pairing before any code trusts them.

pub mod fake;
pub mod net;
pub mod pipe;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::types::{ChatMessageDto, CoreEvent, NearbyDevice, PersonaDto, ThreadSummary};
use crate::crypto::rng;
use crate::frb_generated::StreamSink;
use crate::identity::keystore::SoftwareKeystore;
use crate::identity::{Identity, Persona};
use crate::store::{
    Direction, MessageState, NewContact, NewMessage, NewTransfer, Store, StoreError, TransferState,
};

struct Core {
    store: Store,
    /// Shared with [`net::Net`] rather than owned: the persona endpoint must
    /// serve what a rename produced, not what existed at start-up.
    identity: Arc<Mutex<Identity>>,
    /// The radio plane. `None` when no transport could be built — the app still
    /// runs, reports itself unavailable, and does not pretend to be reachable.
    net: Option<Arc<net::Net>>,
}

static CORE: Mutex<Option<Core>> = Mutex::new(None);
static EVENT_SINK: Mutex<Option<StreamSink<CoreEvent>>> = Mutex::new(None);

// ── event bus ───────────────────────────────────────────────────────────────

/// Register the Dart event sink. Replaces any previous one.
pub fn set_event_sink(sink: StreamSink<CoreEvent>) {
    *EVENT_SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
}

/// Emit an event to Dart, if a sink is registered. Recovers a poisoned lock so
/// one bad emit can't brick the event bus. Never call while holding `CORE`.
pub fn emit(event: CoreEvent) {
    if let Some(sink) = EVENT_SINK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        let _ = sink.add(event);
    }
}

// ── lifecycle ─────────────────────────────────────────────────────────────────

/// Which rung the core should run on.
///
/// Not the tech spec §9 ladder — that runs several rungs at once and is T15's
/// job. This picks exactly one, because the BLE acceptance needs the radio in
/// isolation: with LAN also running, a peer found over Wi-Fi is
/// indistinguishable from one found over the air, and the run would prove
/// nothing about the radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Radio {
    /// mDNS and TCP. The default, and the only rung with hardware behind it.
    #[default]
    Lan,
    /// The BLE rung, driven through the platform seam. Requires a host
    /// dispatcher to be attached, or every command queues and nothing happens.
    Ble,
}

/// Initialise the core at `support_dir`, returning the local persona.
pub fn init(support_dir: String) -> Result<PersonaDto, String> {
    init_on(support_dir, Radio::default())
}

/// Initialise on a chosen rung.
pub fn init_on(support_dir: String, radio: Radio) -> Result<PersonaDto, String> {
    let store = open_store(support_dir)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let sink: crate::transport::EventSink = Box::new(move |e| {
        let _ = tx.send(e);
    });
    let local_id = fresh_node_id();

    // A transport that will not start is not fatal. The app runs, discovery
    // reports itself unavailable, and every reach fails with a reason — which
    // is what lets the UI say why instead of showing an empty list that reads
    // as "nobody is nearby" (R0-F2).
    let transport: Option<Arc<dyn crate::transport::Transport>> = match radio {
        Radio::Lan => crate::transport::lan::LanTransport::new(&local_id, sink)
            .ok()
            .map(|t| Arc::new(t) as Arc<dyn crate::transport::Transport>),
        Radio::Ble => {
            let bridge = crate::platform::bridge().clone();
            let radio = crate::platform::ble::HostBleRadio::new(bridge.clone());
            crate::transport::ble::BleTransport::new(&local_id, Arc::new(radio), sink)
                .ok()
                .map(|t| {
                    // Facts have somewhere to land before any command goes out,
                    // so a radio that answers immediately is not reporting into
                    // nothing.
                    bridge.attach_ble(t.ingress());
                    Arc::new(t) as Arc<dyn crate::transport::Transport>
                })
        }
    };
    log::info!(
        "core starting on the {radio:?} rung (transport built: {})",
        transport.is_some()
    );
    install(store, transport, &local_id, rx)
}

/// Initialise against a caller-supplied transport.
///
/// `CORE` is process-wide, so a test cannot stand two engines against each
/// other; this is how a test drives the real engine over the loopback rung
/// rather than opening sockets. Deliberately not on the `crate::api` surface —
/// the bridge has no business offering it.
pub fn init_with_transport(
    support_dir: String,
    transport: Arc<dyn crate::transport::Transport>,
    local_id: &str,
    events: std::sync::mpsc::Receiver<crate::transport::TransportEvent>,
) -> Result<PersonaDto, String> {
    let store = open_store(support_dir)?;
    install(store, Some(transport), local_id, events)
}

fn open_store(support_dir: String) -> Result<Store, String> {
    let dir = PathBuf::from(support_dir);
    let db = dir.join("hoppler.db");
    let files = dir.join("files");
    let keystore = Arc::new(SoftwareKeystore::new());

    // Record whether a master already existed *before* open (open seals a fresh
    // one on first use). A stale DB is safe to reset only when no master
    // exists — the ciphertext is then provably unkeyable. Any other open
    // failure is propagated with the DB preserved; `crypto_erase` is the only
    // path that may destroy data.
    let had_master = Store::master_is_sealed(keystore.as_ref());
    match Store::open(keystore.clone(), &db, &files) {
        Ok(s) => Ok(s),
        Err(_) if !had_master && db.exists() => {
            reset_stale_db(&db)?;
            // The file store is keyed by the same (now absent) master, so its
            // ciphertext is likewise unrecoverable — clear it too.
            let _ = std::fs::remove_dir_all(&files);
            Store::open(keystore, &db, &files).map_err(stringify)
        }
        Err(e) => Err(stringify(e)),
    }
}

fn install(
    store: Store,
    transport: Option<Arc<dyn crate::transport::Transport>>,
    local_id: &str,
    events: std::sync::mpsc::Receiver<crate::transport::TransportEvent>,
) -> Result<PersonaDto, String> {
    let identity = Arc::new(Mutex::new(Identity::generate("Me", 0x0044_88ff)));
    let dto = persona_dto(identity.lock().unwrap_or_else(|e| e.into_inner()).persona());

    let net = transport.map(|t| {
        let net = Arc::new(net::Net::new(
            t,
            identity.clone(),
            local_id,
            std::time::Instant::now(),
        ));
        spawn_pump(net.clone(), events);
        net
    });

    *CORE.lock().map_err(|_| "core lock".to_string())? = Some(Core {
        store,
        identity,
        net,
    });
    Ok(dto)
}

/// Drain transport events into `Net` and turn what it reports into `CoreEvent`s.
///
/// On its own thread because the alternative is doing radio work on whatever
/// thread Dart happens to call in on, and because `emit` must never run while
/// the store lock is held.
fn spawn_pump(
    net: Arc<net::Net>,
    events: std::sync::mpsc::Receiver<crate::transport::TransportEvent>,
) {
    std::thread::Builder::new()
        .name("hoppler-core-pump".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                for out in net.handle(event, std::time::Instant::now()) {
                    on_net_event(&net, out);
                }
            }
        })
        .expect("core event pump");
}

/// A fresh node id for the rung: random, and carrying nothing derived from our
/// keys, since anything with structure would survive rotation as a fingerprint.
fn fresh_node_id() -> String {
    let bytes = rng::random_array::<8>();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── identity ──────────────────────────────────────────────────────────────────

pub fn current_persona() -> Result<PersonaDto, String> {
    with_core(|core| {
        let identity = core.identity.lock().unwrap_or_else(|e| e.into_inner());
        Ok(persona_dto(identity.persona()))
    })
}

pub fn update_persona(name: String, colour: u32) -> Result<PersonaDto, String> {
    with_core_mut(|core| {
        let mut identity = core.identity.lock().unwrap_or_else(|e| e.into_inner());
        identity.update_persona(name, colour);
        Ok(persona_dto(identity.persona()))
    })
}

// ── discovery ─────────────────────────────────────────────────────────────────

pub fn set_discovery(enabled: bool) -> Result<(), String> {
    let net = require_net()?;
    net.discovery()
        .set_enabled(enabled, std::time::Instant::now())
        .map_err(|e| e.to_string())?;
    if enabled {
        net.discovery()
            .start_scanning()
            .map_err(|e| e.to_string())?;
    }
    emit(CoreEvent::DiscoveryUpdated {
        devices: nearby_devices()?,
    });
    Ok(())
}

/// The devices currently visible.
///
/// Empty while Discovery is off is *our* choice, not the transport's: sightings
/// survive `stop_scanning` at the rung (a peer seen a moment ago is still
/// reachable, and rule 5's dial-back depends on the record), and hiding them is
/// a core-layer decision. See the note on `Transport::peers`.
pub fn nearby_devices() -> Result<Vec<NearbyDevice>, String> {
    let net = match with_core(|core| Ok(core.net.clone()))? {
        Some(net) => net,
        None => return Ok(Vec::new()),
    };
    if !net.discovery().is_on() {
        return Ok(Vec::new());
    }
    Ok(net
        .discovery()
        .sightings()
        .into_iter()
        .map(|s| {
            // A peer whose persona we have not fetched yet is real and
            // reachable; showing it unnamed beats hiding it until a round trip
            // completes, which is what "who's nearby" is asking.
            let (name, colour) = match &s.persona {
                Some(p) => (p.name.clone(), p.colour),
                None => (String::new(), 0),
            };
            NearbyDevice {
                device_id: s.peer,
                name,
                colour,
                paired: false,
            }
        })
        .collect())
}

// ── sessions / threads ────────────────────────────────────────────────────────

/// Ping a device visible in Discovery. Only reachable while Discovery is open —
/// closing it makes pings undeliverable (F3). The `Pinged` event is the ack.
///
/// Real cross-device reachability (the peer's Discovery state) and the
/// receiver-side rate limiter (tech spec §7) arrive with the session layer
/// (T09/T10); here reachability is modelled by our own Discovery flag.
pub fn ping(device_id: String) -> Result<(), String> {
    let net = require_net()?;
    if !net.discovery().is_on() {
        return Err("discovery is off — no one is reachable".to_string());
    }
    // Acceptance, not delivery. A real peer acknowledges when it answers, so
    // `Pinged` is now an *inbound* event rather than something this call
    // produces — the fake used to emit it synchronously and the UI must not
    // rely on that any more.
    //
    // `Net::ping` reaches for itself, so the `reach` that used to stand here
    // was a second dial for one tap. On TCP that is invisible — a duplicate
    // connect to a peer already being connected is absorbed — but two L2CAP
    // channels opened to one remote in the same millisecond fail, and on two
    // phones every Ping did. Two `dialling` lines per tap, microseconds apart,
    // is what the adapter's logging finally showed.
    //
    // It was also in the wrong order. `Net::ping` queues the Ping *before*
    // reaching, so that a pipe which opens immediately still finds it; reaching
    // first opens that window for no benefit.
    net.ping(&device_id, std::time::Instant::now())?;
    watch_for_an_undelivered_ping(net);
    Ok(())
}

/// Wake up once, after the Ping deadline, to report anything still waiting.
///
/// A thread per tap rather than a permanent ticker: a queued Ping is the only
/// thing here with a deadline, taps are rare and human-paced, and a loop that
/// runs forever costs battery every second of a session that may never queue
/// anything at all.
///
/// It sleeps slightly past the deadline so the check lands after expiry rather
/// than racing it, and finding nothing to report is the normal case — the
/// session usually opens first and takes the Ping with it.
fn watch_for_an_undelivered_ping(net: Arc<net::Net>) {
    let _ = std::thread::Builder::new()
        .name("hoppler-ping-deadline".into())
        .spawn(move || {
            std::thread::sleep(net::PING_DEADLINE + std::time::Duration::from_millis(100));
            for out in net.expire_pings(std::time::Instant::now()) {
                on_net_event(&net, out);
            }
        });
}

/// Send a chat message: writes the outgoing row, then (fake) receives a canned
/// reply and emits `MessageReceived`. Returns the outgoing message as stored.
///
/// Fake-faithfulness note: the reply row is written synchronously here, but the
/// event is emitted after the store lock is released and delivered on Dart's
/// event loop (never synchronously during this call). Real transports reply
/// seconds later; UI must not assume the reply is present when this returns.
pub fn send_chat(device_id: String, text: String) -> Result<ChatMessageDto, String> {
    let (dto, msg_id_bytes) = with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, &device_id, now)?;

        let (out_bytes, out_hex) = new_msg_id();
        let out_bytes_for_state = out_bytes.clone();
        let seq = core.store.next_seq(thread, Direction::Outgoing)?;
        core.store.add_message(&NewMessage {
            thread_id: thread,
            seq,
            msg_id: out_bytes,
            body: text.clone().into_bytes(),
            direction: Direction::Outgoing,
            // Queued, not Sent: nothing has left the device yet. Writing
            // Sent here and hoping would make the row lie whenever the send
            // fails, and a resend queue would have no way to find it.
            state: MessageState::Queued,
            created_at: now,
        })?;
        let dto = ChatMessageDto {
            msg_id: out_hex,
            thread_id: thread,
            text: text.clone(),
            outgoing: true,
            created_at: now,
        };

        Ok((dto, out_bytes_for_state))
    })?;

    // On the wire only after the row exists, so a failed send leaves the
    // message in the thread rather than losing what the person typed. The row
    // is promoted to Sent only once the bytes are actually away — a caller
    // that sees Sent can trust it.
    let net = require_net()?;
    net.send_chat(&device_id, &text, std::time::Instant::now())?;
    with_core(|core| {
        core.store
            .set_message_state(&msg_id_bytes, MessageState::Sent)?;
        Ok(())
    })?;
    Ok(dto)
}

/// Messages of a thread in chronological display order. `seq` is per-sender so
/// it can't be the display key, and the wall-clock `created_at` is coarse and
/// skewable; order by `id` (rowid) — the store's local causal insertion order.
/// Bodies are decoded lossily as UTF-8 (v0 is text-only).
pub fn thread_messages(thread_id: i64) -> Result<Vec<ChatMessageDto>, String> {
    with_core(|core| {
        let mut msgs = core.store.messages_for_thread(thread_id)?;
        msgs.sort_by_key(|m| m.id);
        Ok(msgs
            .into_iter()
            .map(|m| ChatMessageDto {
                msg_id: hex::encode(&m.msg_id),
                thread_id: m.thread_id,
                text: String::from_utf8_lossy(&m.body).into_owned(),
                outgoing: m.direction == Direction::Outgoing,
                created_at: m.created_at,
            })
            .collect())
    })
}

/// The existing thread with a device, if any — for opening a conversation
/// without sending first.
pub fn thread_for_device(device_id: String) -> Result<Option<i64>, String> {
    with_core(|core| match contact_id_for_device(core, &device_id)? {
        Some(id) => core.store.thread_for_contact(id),
        None => Ok(None),
    })
}

/// All conversations, for the UI's thread list.
pub fn list_threads() -> Result<Vec<ThreadSummary>, String> {
    with_core(|core| {
        let mut out = Vec::new();
        for (thread_id, contact_id) in core.store.list_threads()? {
            if let Some(c) = core.store.contact_by_id(contact_id)? {
                out.push(ThreadSummary {
                    thread_id,
                    name: c.name,
                    colour: c.colour,
                });
            }
        }
        Ok(out)
    })
}

// ── transfers ─────────────────────────────────────────────────────────────────

/// Offer a Drop: records a transfer row linked to the device's thread and
/// (fake) emits progress then completion. Returns the transfer id.
pub fn offer_drop(device_id: String, name: String, size: u64) -> Result<String, String> {
    let size = i64::try_from(size).map_err(|_| "transfer size too large".to_string())?;
    let transfer_id = with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, &device_id, now)?;
        let id = core.store.add_transfer(&NewTransfer {
            thread_id: Some(thread),
            direction: Direction::Outgoing,
            name,
            size,
            mime: "application/octet-stream".into(),
            state: TransferState::Complete,
            root_hash: [0u8; 32],
            chunk_bitmap: Vec::new(),
            created_at: now,
        })?;
        Ok(format!("xfer-{id}"))
    })?;

    let total = size.max(1) as u64;
    for received in [0, total / 2, total] {
        emit(CoreEvent::TransferProgress {
            transfer_id: transfer_id.clone(),
            received,
            total,
        });
    }
    emit(CoreEvent::TransferCompleted {
        transfer_id: transfer_id.clone(),
        success: true,
    });
    Ok(transfer_id)
}

// ── internals ─────────────────────────────────────────────────────────────────

/// The radio plane, or a reason there is none.
fn require_net() -> Result<Arc<net::Net>, String> {
    with_core(|core| Ok(core.net.clone()))?
        .ok_or_else(|| "no radio available on this device".to_string())
}

/// Turn what `Net` reports into events Dart can act on, and persist what needs
/// persisting. Runs on the pump thread, never while the store lock is held.
fn on_net_event(net: &Arc<net::Net>, event: net::NetEvent) {
    match event {
        net::NetEvent::PeersChanged => {
            if let Ok(devices) = nearby_devices() {
                emit(CoreEvent::DiscoveryUpdated { devices });
            }
        }
        net::NetEvent::SessionOpened { peer, .. } => {
            // The moment the peer is proved is the moment a row keyed on their
            // rotating id can be moved onto the real one. Doing it only on the
            // next send or receive would leave the UI opening a conversation by
            // device id and landing on a thread that is about to be folded into
            // another. Creates nothing, so a peer who says nothing still leaves
            // no trace.
            if let Err(why) = with_core_mut(|core| reconcile_contact(core, &peer)) {
                log::warn!("could not reconcile the contact for {peer}: {why}");
            }
            if let Ok(devices) = nearby_devices() {
                emit(CoreEvent::DiscoveryUpdated { devices });
            }
        }
        net::NetEvent::SessionClosed { .. } => {
            if let Ok(devices) = nearby_devices() {
                emit(CoreEvent::DiscoveryUpdated { devices });
            }
        }
        net::NetEvent::Pinged { peer, persona_name } => {
            emit(CoreEvent::Pinged {
                device_id: peer,
                name: persona_name,
            });
        }
        net::NetEvent::PingAcked { peer } => {
            emit(CoreEvent::PingAcked { device_id: peer });
        }
        net::NetEvent::RadioChanged { available, reason } => {
            emit(CoreEvent::RadioChanged { available, reason });
        }
        net::NetEvent::PingUndeliverable { peer, why } => {
            emit(CoreEvent::PingFailed {
                device_id: peer,
                reason: why,
            });
        }
        net::NetEvent::ChatReceived { peer, text } => {
            let _ = net;
            if let Ok(Some(event)) = store_incoming_chat(&peer, &text) {
                emit(event);
            }
        }
    }
}

/// Write an inbound chat line and return the event announcing it.
fn store_incoming_chat(device_id: &str, text: &str) -> Result<Option<CoreEvent>, String> {
    with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, device_id, now)?;
        let (bytes, hex) = new_msg_id();
        let seq = core.store.next_seq(thread, Direction::Incoming)?;
        core.store.add_message(&NewMessage {
            thread_id: thread,
            seq,
            msg_id: bytes,
            body: text.as_bytes().to_vec(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: now,
        })?;
        Ok(Some(CoreEvent::MessageReceived {
            thread_id: thread,
            msg_id: hex,
            text: text.to_string(),
        }))
    })
}

fn with_core<T>(f: impl FnOnce(&Core) -> Result<T, StoreError>) -> Result<T, String> {
    let guard = CORE.lock().map_err(|_| "core lock".to_string())?;
    let core = guard
        .as_ref()
        .ok_or_else(|| "core not initialised".to_string())?;
    f(core).map_err(stringify)
}

fn with_core_mut<T>(f: impl FnOnce(&mut Core) -> Result<T, StoreError>) -> Result<T, String> {
    let mut guard = CORE.lock().map_err(|_| "core lock".to_string())?;
    let core = guard
        .as_mut()
        .ok_or_else(|| "core not initialised".to_string())?;
    f(core).map_err(stringify)
}

/// The peer's durable identity, if a session has authenticated one.
fn pseudonym_of(core: &Core, device_id: &str) -> Option<[u8; 32]> {
    core.net
        .as_ref()
        .and_then(|net| net.pseudonym(device_id))
        .map(|p| p.0)
}

/// The contact for this device, if there already is one.
///
/// The read-only twin of [`ensure_contact`]: same two keys, tried in the same
/// order, but it creates nothing and adopts nothing. Sharing the *order* is the
/// point — a lookup that consulted only the device id would miss every contact
/// that had already moved onto its session key, and report no conversation for
/// someone the user has been talking to all afternoon.
fn contact_id_for_device(core: &Core, device_id: &str) -> Result<Option<i64>, StoreError> {
    if let Some(real) = pseudonym_of(core, device_id) {
        if let Some(c) = core.store.contact_by_l1(&real)? {
            return Ok(Some(c.id));
        }
    }
    Ok(core
        .store
        .contact_by_l1(&fake::fake_l1_pub(device_id))?
        .map(|c| c.id))
}

/// The contact this device belongs to, created, adopted or merged as needed.
///
/// Keyed on the peer's session pseudonym — its static DH public, which the
/// Noise IK handshake authenticates — and **not** on the device id. The id
/// rotates every twelve minutes under R0-F2, so a contact keyed on it became a
/// different contact, with a different thread, four or five times an hour: one
/// conversation shattered into a row of identical-looking strangers.
///
/// The device id is still the fallback, because before a session it is
/// genuinely all there is — a chat can be sent to someone not yet connected.
/// A row opened that way does not stay there: [`reconcile_contact`] moves it,
/// either by re-keying it or by folding it into the row that already holds the
/// real key. Without that, the split would only have moved from every twelve
/// minutes to once, which is a quieter bug rather than no bug.
///
/// Reconciliation runs here **and** from the session-open event, so it does not
/// wait for the next send: a UI opening a conversation by device id would
/// otherwise land on a thread about to be folded into another.
/// Move a device-id-keyed row onto the pseudonym now that one is proved.
///
/// **Creates nothing.** Run from the session-open event, so it meets peers the
/// user has never written to, and this module promises that "a device that
/// connects and says nothing leaves no trace beyond the transport's own
/// bookkeeping". A reconcile that inserted would break exactly that.
///
/// Both keys can hold a row at once — the person was known from an earlier
/// session, their id rotated, and something was written to the new id before
/// the next session proved it was them — so this either re-keys the stray or
/// folds it into the row that already owns the real key.
fn reconcile_contact(core: &Core, device_id: &str) -> Result<(), StoreError> {
    let Some(real) = pseudonym_of(core, device_id) else {
        return Ok(());
    };
    let Some(stray) = core
        .store
        .contact_by_l1(&fake::fake_l1_pub(device_id))?
        .map(|c| c.id)
    else {
        return Ok(());
    };
    match core.store.contact_by_l1(&real)?.map(|c| c.id) {
        // Re-keying cannot help: the real key is already taken.
        Some(known) => core.store.merge_contact(stray, known)?,
        None => {
            core.store.rekey_contact(stray, &real)?;
        }
    }
    Ok(())
}

fn ensure_contact(core: &Core, device_id: &str, now: i64) -> Result<i64, StoreError> {
    // Reconcile first, so the lookup below cannot find the stale row and hand
    // back a thread that is about to be folded into another.
    reconcile_contact(core, device_id)?;
    if let Some(id) = contact_id_for_device(core, device_id)? {
        return Ok(id);
    }

    // Nothing on file. Prefer the durable key if a session has proved one;
    // otherwise the device id, which `reconcile_contact` will move later.
    let key = pseudonym_of(core, device_id).unwrap_or_else(|| fake::fake_l1_pub(device_id));
    let (name, colour) = fake::peer(device_id)
        .map(|p| (p.name.to_owned(), p.colour))
        .unwrap_or_else(|| ("Unknown".into(), 0));
    core.store.add_contact(&NewContact {
        l1_pub: key,
        l2_pub: [0u8; 32], // placeholder — real Layer-2 arrives with pairing (T08–T10)
        name,
        colour,
        persona_version: 1,
        paired_at: now,
    })
}

fn ensure_thread(core: &Core, device_id: &str, now: i64) -> Result<i64, StoreError> {
    let contact_id = ensure_contact(core, device_id, now)?;
    match core.store.thread_for_contact(contact_id)? {
        Some(t) => Ok(t),
        None => core.store.create_thread(contact_id, now),
    }
}

fn persona_dto(p: &Persona) -> PersonaDto {
    PersonaDto {
        name: p.name.clone(),
        colour: p.colour,
        version: p.version,
    }
}

fn new_msg_id() -> (Vec<u8>, String) {
    let bytes = rng::random_array::<16>();
    (bytes.to_vec(), hex::encode(bytes))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn stringify(e: StoreError) -> String {
    e.to_string()
}

/// Remove a stale (unkeyable) database and its WAL sidecars before recreating.
fn reset_stale_db(db: &Path) -> Result<(), String> {
    std::fs::remove_file(db).map_err(|_| "could not reset stale database".to_string())?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = db.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
    Ok(())
}
