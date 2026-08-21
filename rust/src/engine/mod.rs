//! The core engine behind the bridge (`crate::api`). Holds process-wide state —
//! identity, store, and the event sink — and runs the six API areas' operations
//! against the real store and identity, with a fake network standing in for the
//! radio plane (T08–T10). Not scanned by flutter_rust_bridge (only `crate::api`
//! is), so it stays free of bridge concerns.
//!
//! Keystore, and what is still owed. The store's master is sealed in a
//! [`FileKeystore`](crate::identity::filekeystore), so the database now
//! survives the app being closed — until this changed it did not, and the
//! phrase that stood here ("everything within a session persists to the real
//! encrypted store") was true only in the sense that a session was all there
//! ever was: an in-memory keystore meant no master was found on the next
//! launch, and the database was deleted unread.
//!
//! One thing remains owed: R0-F1's "master secret in platform hardware" needs
//! the Android Keystore backend. File permissions are what stands in for it
//! today, which is a real boundary on Android and a weak one on desktop. The
//! shape it plugs into is
//! [`WrappedKeystore`](crate::identity::wrapped::WrappedKeystore), and
//! `platform_keystore` is the one place that chooses it.
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

use crate::api::types::{
    ChatMessageDto, CoreEvent, NearbyDevice, PersonaDto, SasColourDto, ThreadSummary,
};
use crate::crypto::rng;
use crate::frb_generated::StreamSink;
use crate::identity::filekeystore::FileKeystore;
use crate::identity::keystore::Keystore;
use crate::identity::{Identity, Persona, VerifiedPersona};
use crate::identity::{COLOUR_MASK, MAX_PERSONA_NAME_LEN};
use crate::pairing::invite::Invite;
use crate::session::chat::{ChatEnvelope, Delivery, Inbox, Outbox, MAX_UNACKED, MSG_ID_LEN};
use crate::store::{
    Direction, InboxPosition, InsertOutcome, MessageState, NewContact, NewMessage, NewTransfer,
    Store, StoreError, TransferState,
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
    let (store, keystore) = open_store(support_dir)?;
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
    install(store, keystore, transport, &local_id, rx)
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
    let (store, keystore) = open_store(support_dir)?;
    install(store, keystore, Some(transport), local_id, events)
}

/// Whether the engine holds a session with `device_id`.
///
/// For tests, on the same terms as [`init_with_transport`] and deliberately not
/// on the `crate::api` surface. A session is a two-sided fact and the peer's
/// half arrives first: the engine adopts its own on the pump thread, a moment
/// later. A test that waits only on the peer is racing that pump, and two did —
/// invisibly on an idle machine, and 13 times in 15 under CPU load.
///
/// Reading it through the engine rather than the peer is the only way a test
/// can wait for the side it is actually about to make assertions on.
pub fn has_session(device_id: &str) -> bool {
    CORE.lock()
        .map(|guard| {
            guard
                .as_ref()
                .and_then(|core| core.net.as_ref())
                .is_some_and(|net| net.sessions().is_open(device_id))
        })
        .unwrap_or(false)
}

/// Open the store exactly as a launch does, for the persistence tests.
///
/// Deliberately the real thing rather than a fixture. What broke was not the
/// store but the keystore underneath it, and a test that supplied its own
/// keystore would have gone on passing throughout — which is what every store
/// test did. `pub` with a note, following the precedent in this crate, because
/// `#[cfg(test)]` items are invisible to `tests/`.
pub fn open_store_for_test(support_dir: String) -> Result<Store, String> {
    open_store(support_dir).map(|(store, _)| store)
}

/// Open the store, and hand back the keystore that keyed it.
///
/// Returned rather than rebuilt by the caller because there must be exactly one
/// view of what this device has sealed: a second `FileKeystore` over the same
/// directory would work, and would be a second place for the answer to "is this
/// a first launch" to come from.
///
/// `dyn` because the backend is a platform choice — see `platform_keystore`.
type Opened = (Store, Arc<dyn Keystore>);

/// The best keystore this platform has.
///
/// One function, so that "what protects the seeds here" has one answer and one
/// place to read it.
///
/// **Android** gets the file keystore inside a
/// [`WrappedKeystore`](crate::identity::wrapped::WrappedKeystore): every secret
/// is encrypted under a key generated in the Android Keystore, which cannot be
/// exported and cannot be used on another device. That is R0-F1's "master
/// secret in platform hardware".
///
/// **Linux desktop** gets the file keystore alone — file permissions, which
/// there means only "the account you are already logged in as", since anything
/// running as that user can read them. There is no platform key store to reach
/// for and the file keystore's own module doc says what that is worth.
///
/// # Why a missing bridge is fatal on Android rather than a fallback
///
/// If Kotlin never registered itself, this could quietly return the bare file
/// keystore and the app would start. It would also mean a build that says it
/// keeps the master in hardware and does not, on precisely the devices where
/// something went wrong, with nothing on screen to say so. The failure is a
/// wiring mistake inside one APK — the Kotlin and the Rust ship together — so
/// it is a bug to fix, not a condition to degrade through.
fn platform_keystore(dir: &Path) -> Result<Arc<dyn Keystore>, String> {
    let files = FileKeystore::open(dir.join("keys")).map_err(|e| e.to_string())?;
    harden(files)
}

/// Put the platform's key in front of the files, where there is one.
///
/// Two whole functions rather than a branch inside one, so that neither
/// platform's version has a case the other must read past — and so the Android
/// one can name Android types without the desktop build pretending they exist.
#[cfg(target_os = "android")]
fn harden(files: FileKeystore) -> Result<Arc<dyn Keystore>, String> {
    use crate::identity::android::{self, AndroidHardware};
    use crate::identity::wrapped::WrappedKeystore;

    if !android::is_available() {
        return Err("the hardware keystore was never registered; \
                    HardwareKeystore.install() must run before the core opens a store"
            .to_owned());
    }
    Ok(Arc::new(WrappedKeystore::new(files, AndroidHardware)))
}

#[cfg(not(target_os = "android"))]
fn harden(files: FileKeystore) -> Result<Arc<dyn Keystore>, String> {
    Ok(Arc::new(files))
}

fn open_store(support_dir: String) -> Result<Opened, String> {
    let dir = PathBuf::from(support_dir);
    let db = dir.join("hoppler.db");
    let files = dir.join("files");
    // Durable, which it was not until now. `SoftwareKeystore` keeps its
    // entries in a `HashMap`, so every launch got an empty one, found no
    // master, and took the stale-database path below — deleting the database
    // and the file store. Measured on a Pixel: `hoppler.db` came back with a
    // different inode after every restart. Nothing had ever survived a close.
    //
    // Which means the reset below has, until this line changed, run on every
    // single launch and never once in the situation it was written for.
    let keystore = platform_keystore(&dir)?;

    // Record whether a master already existed *before* open (open seals a fresh
    // one on first use). A stale DB is safe to reset only when no master
    // exists — the ciphertext is then provably unkeyable. Any other open
    // failure is propagated with the DB preserved; `crypto_erase` is the only
    // path that may destroy data.
    let had_master = Store::master_is_sealed(keystore.as_ref());
    match Store::open(keystore.clone(), &db, &files) {
        Ok(s) => Ok((s, keystore)),
        Err(_) if !had_master && db.exists() => {
            reset_stale_db(&db)?;
            // The file store is keyed by the same (now absent) master, so its
            // ciphertext is likewise unrecoverable — clear it too.
            let _ = std::fs::remove_dir_all(&files);
            Store::open(keystore.clone(), &db, &files)
                .map(|s| (s, keystore))
                .map_err(stringify)
        }
        Err(e) => Err(stringify(e)),
    }
}

fn install(
    store: Store,
    keystore: Arc<dyn Keystore>,
    transport: Option<Arc<dyn crate::transport::Transport>>,
    local_id: &str,
    events: std::sync::mpsc::Receiver<crate::transport::TransportEvent>,
) -> Result<PersonaDto, String> {
    let identity = Arc::new(Mutex::new(load_identity(&store, keystore.as_ref())?));
    let dto = persona_dto(
        identity.lock().unwrap_or_else(|e| e.into_inner()).persona(),
        needs_name(&store),
    );

    let net = transport.map(|t| {
        let net = Arc::new(net::Net::new(
            t,
            identity.clone(),
            local_id,
            std::time::Instant::now(),
        ));
        spawn_pump(net.clone(), events);
        spawn_clock(&net, CLOCK_INTERVAL);
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

/// How often the engine's clock wakes.
///
/// It serves two deadlines — a five-minute idle timeout and a twelve-minute id
/// rotation — so this only has to be small against those, and every second it
/// is smaller costs battery for nothing. Thirty seconds bounds a rotation's
/// lateness to 4% of its period while waking twice a minute, against a radio
/// that is scanning continuously; N4's budget is not spent here.
const CLOCK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Wake periodically and give `Net` its turn.
///
/// The engine is otherwise entirely event-driven — [`spawn_pump`] blocks on the
/// transport, and the only other thread is the one-shot that watches a Ping's
/// deadline. That was the whole design, and it left the two things that come
/// due *during silence* with nothing to call them: the advertised id never
/// rotated, so R0-F2 unlinkability was not delivered on any device, and no
/// session ever expired. A periodic wake is the only shape that fixes either,
/// because there is no event to hang them off — the absence of events is the
/// condition they fire on.
///
/// Holds a [`Weak`] rather than an `Arc`: nothing shuts the engine down today,
/// but a second `init` replaces the core, and a clock still ticking against the
/// previous one would rotate ids on a transport no longer in use. Losing the
/// upgrade is how this thread learns it has been replaced.
fn spawn_clock(net: &Arc<net::Net>, interval: std::time::Duration) {
    let net = Arc::downgrade(net);
    std::thread::Builder::new()
        .name("hoppler-core-clock".into())
        .spawn(move || loop {
            std::thread::sleep(interval);
            let Some(net) = net.upgrade() else { return };
            for out in net.tick(std::time::Instant::now()) {
                on_net_event(&net, out);
            }
        })
        // Loudly, like the pump. A dropped spawn error here is silent and total:
        // the id stops rotating and sessions stop expiring, which is precisely
        // the state this thread was added to end, restored without a trace. It
        // was written `let _ =` first, and review was right to call that out.
        .expect("core clock");
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
        Ok(persona_dto(identity.persona(), needs_name(&core.store)))
    })
}

pub fn update_persona(name: String, colour: u32) -> Result<PersonaDto, String> {
    with_core_mut(|core| {
        let mut identity = core.identity.lock().unwrap_or_else(|e| e.into_inner());
        // Stored first, then made live, and that order is the fix rather than a
        // preference. Renaming in memory and then failing to store leaves the
        // core serving and announcing a name the next launch will not have,
        // while telling the caller the rename failed — the same shape as a
        // ratchet turning before the exchange that could refuse it.
        let next = identity.persona_after(name.clone(), colour);
        core.store
            .settings_set(PERSONA_KEY, &encode_persona(&next))?;
        identity.update_persona(name, colour);
        debug_assert_eq!(
            identity.persona(),
            &next,
            "what was stored is not what went live"
        );
        // False from here on, and by construction rather than by assignment:
        // the row this just wrote is the thing `needs_name` reads.
        Ok(persona_dto(identity.persona(), false))
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
    // Discovery off means the radio is not looking, so there are no sightings.
    // It does not mean we have forgotten anybody: the paired rows below come
    // off the disk and cost no air at all.
    let sightings = match with_core(|core| Ok(core.net.clone()))? {
        Some(net) if net.discovery().is_on() => net.discovery().sightings(),
        _ => Vec::new(),
    };
    with_core(|core| {
        let mut rows = Vec::new();
        let mut in_front_of_us = Vec::new();
        for s in sightings {
            let contact = contact_for_sighting(core, &s.peer, s.persona.as_ref())?;
            if let Some(id) = contact {
                in_front_of_us.push(id);
            }
            let known = match contact {
                Some(id) => core.store.contact_by_id(id)?,
                None => None,
            };
            let paired = match contact {
                Some(id) => core.store.pairing_for_contact(id)?.is_some(),
                None => false,
            };
            // A peer whose persona we have not fetched yet is real and
            // reachable; showing it unnamed beats hiding it until a round trip
            // completes, which is what "who's nearby" is asking. Unnamed is not
            // the same as unknown, though — if we have met them before, their
            // name is on disk, and drawing a blank row for someone the user
            // knows is worse than drawing one for a stranger.
            let (name, colour) = match (&s.persona, &known) {
                (Some(p), _) => (p.name.clone(), p.colour),
                (None, Some(c)) => (c.name.clone(), c.colour),
                (None, None) => (String::new(), 0),
            };
            rows.push(NearbyDevice {
                device_id: Some(s.peer),
                thread_id: match contact {
                    Some(id) => core.store.thread_for_contact(id)?,
                    None => None,
                },
                name,
                colour,
                paired,
            });
        }
        // Everyone we have paired with and cannot see. R0-F4 makes pairing a
        // durable act, so these rows are the app remembering a person rather
        // than reporting a radio: no handle to dial, and a thread that still
        // takes what the user writes.
        for contact in core.store.list_contacts()? {
            if in_front_of_us.contains(&contact.id)
                || core.store.pairing_for_contact(contact.id)?.is_none()
            {
                continue;
            }
            rows.push(NearbyDevice {
                device_id: None,
                thread_id: core.store.thread_for_contact(contact.id)?,
                name: contact.name,
                colour: contact.colour,
                paired: true,
            });
        }
        Ok(rows)
    })
}

// ── sessions / threads ────────────────────────────────────────────────────────

/// Ping a device visible in Discovery. Only reachable while Discovery is open —
/// closing it makes pings undeliverable (F3).
///
/// The ack is `PingAcked`, never `Pinged`. `Pinged` is someone nudging *us*,
/// and an acknowledgement derived from it would only arrive when the other
/// person happened to nudge back — so an ordinary ping would always time out
/// while an unrelated incoming one was mistaken for the answer. The wire
/// carries a Pong for this reason.
///
/// The `is_on` check below is the F3 gate on this side: it says whether *we*
/// may ping, not whether the peer is reachable. Reachability is the transport's
/// answer, and arrives as `PingAcked` or `PingUndeliverable`.
///
/// **Not implemented: the receiver-side rate limiter for Ping (tech spec §7).**
/// Discovery has its own rate limiting for persona requests, which is a
/// different thing; nothing bounds how often a peer may ping us.
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
    let thread = with_core_mut(|core| ensure_thread(core, &device_id, now_millis()))?;
    write_then_send(thread, Some(device_id), text)
}

/// Write to a conversation rather than to a device.
///
/// The entry point an away row uses. R0-F4 makes pairing durable and R0-F2
/// rotates transport ids, so there are stretches — most of the day, for most
/// people — when someone we have paired with has no id we could name. A device
/// is not addressable then; the thread still is, and R0-F5 already says what to
/// do with what they type: keep it, and deliver it when they next meet.
///
/// Sends immediately if that thread's peer happens to be in front of us, which
/// is why this is one function and not two: "write to Wren" should not behave
/// differently depending on whether the radio can see her this second.
pub fn send_chat_to_thread(thread_id: i64, text: String) -> Result<ChatMessageDto, String> {
    let device_id = with_core(|core| device_for_thread(core, thread_id))?;
    write_then_send(thread_id, device_id, text)
}

/// The rest of a send, once the conversation is known.
///
/// `device_id` is `None` when nothing can carry it right now. That is not a
/// failure and is not reported as one — see the `NoSession` arm below, which
/// this is the same decision as, reached earlier.
fn write_then_send(
    thread: i64,
    device_id: Option<String>,
    text: String,
) -> Result<ChatMessageDto, String> {
    let (dto, envelope) = with_core_mut(|core| {
        let now = now_millis();
        let seq = core.store.next_seq(thread, Direction::Outgoing)?;
        // The envelope draws the id, so the number the row is stored under and
        // the number that goes on the wire are the same object rather than two
        // things that happen to agree. `new_msg_id` used to make one here and
        // the receiver made another at the far end, which is why a resend could
        // never be recognised as one.
        // Bounded, and refused rather than dropped. Somebody typing to a
        // person who is not there fills this up, and the honest answer is to
        // say so: silently discarding the oldest would lose something they
        // wrote and tell them nothing, and holding everything makes the queue a
        // peer's absence can grow without limit.
        let queued =
            core.store
                .count_in_state(thread, Direction::Outgoing, MessageState::Queued)?;
        if queued >= MAX_UNACKED {
            return Err(StoreError::Db(format!(
                "{MAX_UNACKED} messages are already waiting to be delivered"
            )));
        }

        // Checked, not cast. `next_seq` returns the store's `i64`, and a
        // negative one — a corrupt row, or a column somebody widened by hand —
        // would wrap into an enormous `u64` and go out as a sequence number no
        // peer could ever follow. Better to refuse to send than to send a
        // message that permanently breaks the far side's ordering.
        //
        // No test covers this and none can through the public API: `next_seq`
        // is `max(seq) + 1` over rows this code wrote, so reaching it means the
        // database already disagrees with the code that filled it. Recorded so
        // the surviving mutant reads as what it is — a guard against a state
        // nothing here can produce — rather than as a hole to close by deleting
        // the conversion. Its twin on the receiving side *is* reachable, since
        // the peer picks that number, and is tested.
        let seq_out = u64::try_from(seq)
            .map_err(|_| StoreError::Db(format!("thread {thread} has a negative seq {seq}")))?;
        let envelope = ChatEnvelope::new(seq_out, text.clone().into_bytes())
            .map_err(|e| StoreError::Db(e.to_string()))?;
        // Everything below reads the envelope. There is deliberately no second
        // copy of the id: the row, the value handed back to the caller and the
        // bytes on the wire are three uses of one value rather than three
        // things that have to be kept in step. They cannot be checked against
        // each other from here — the engine is a singleton, so no test can
        // stand up a second one to watch the wire — so the guarantee has to be
        // that there is nothing to diverge.
        core.store.add_message(&NewMessage {
            thread_id: thread,
            seq,
            msg_id: envelope.msg_id.to_vec(),
            body: text.clone().into_bytes(),
            direction: Direction::Outgoing,
            // Queued, not Sent: nothing has left the device yet. Writing
            // Sent here and hoping would make the row lie whenever the send
            // fails, and a resend queue would have no way to find it.
            state: MessageState::Queued,
            created_at: now,
        })?;
        let dto = ChatMessageDto {
            msg_id: hex::encode(envelope.msg_id),
            thread_id: thread,
            text: text.clone(),
            outgoing: true,
            created_at: now,
        };

        Ok((dto, envelope))
    })?;

    // Nobody to hand it to. The row is `Queued`, `reach_for_queued_messages`
    // will go looking when anyone appears, and `resend_queued` delivers it when
    // one of them turns out to be her. Deliberately before `require_net`: a
    // device with Discovery off has no radio to ask and is still perfectly able
    // to take down what someone typed.
    let Some(device_id) = device_id else {
        log::info!("holding a message on thread {thread} until we meet again");
        return Ok(dto);
    };

    // On the wire only after the row exists, so a failed send leaves the
    // message in the thread rather than losing what the person typed. The row
    // is promoted to Sent only once the bytes are actually away — a caller
    // that sees Sent can trust it.
    let net = require_net()?;
    match net.send_chat(&device_id, envelope.encode(), std::time::Instant::now()) {
        Ok(()) => {
            with_core(|core| {
                core.store
                    .set_message_state(&envelope.msg_id, MessageState::Sent)?;
                Ok(())
            })?;
        }
        // Out of range is not a failure to write a message, and saying so was
        // false: the row is already `Queued` and `resend_queued` delivers it at
        // the next encounter, which is exactly what R0-F5 promises. Reporting
        // an error made the app contradict the requirement to the one person
        // who could see both halves — measured on a Pixel, which showed
        // "Error: no session with …" over a message sitting safely in the
        // thread.
        //
        // Not swallowed: the row stays `Queued`, which is the whole difference
        // between this and a send that got away, and the reason goes to the log.
        Err(net::SendError::NoSession) => {
            log::info!("holding a message for {device_id} until we meet again");
        }
        // Everything else happened *with a session open*, so the reunion path
        // will not run: nothing is going to reopen a session that never closed,
        // and the row would wait forever beside a peer who is right there. Held
        // and reported are opposite answers, and only one of them is true here.
        Err(why) => return Err(format!("could not send that message: {why}")),
    }
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

// ── pairing (R0-F4) ───────────────────────────────────────────────────────────

/// Mint a code to show, and start showing it. Returns the text for the QR.
///
/// A fresh nonce every call, so a code that has been on screen and dismissed
/// cannot be reused from a photograph. That is also why this both mints *and*
/// shows: a caller that could do one without the other would eventually show a
/// code the device was not bound to, and the ceremony would fail with nothing
/// on either screen to explain it.
///
/// The hint inside is the rung id this device is advertising as right now. It
/// rotates every twelve minutes (R0-F2), so a photographed code goes stale —
/// [`begin_pairing`] falls back to Discovery rather than failing.
pub fn pairing_invite() -> Result<String, String> {
    let net = require_net()?;
    let l2_pub = with_core(|core| {
        Ok(core
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layer2_public())
    })?;
    let invite = Invite::fresh(l2_pub, net.discovery().local_id());
    let uri = invite.to_uri();
    net.show_invite(invite);
    Ok(uri)
}

/// Take the code off the screen.
///
/// Does not touch a ceremony already under way. Putting the phone down after
/// someone has scanned is not withdrawing consent to the ceremony they are in
/// — and the person on the other side is looking at colours, waiting.
pub fn stop_showing_invite() -> Result<(), String> {
    require_net()?.stop_showing();
    Ok(())
}

/// Begin pairing from a code the camera just read.
///
/// Returns the device id the ceremony is with, which is how the UI follows the
/// events that come back.
pub fn begin_pairing(code: String) -> Result<String, String> {
    let net = require_net()?;
    let invite = Invite::parse(&code).map_err(|e| e.to_string())?;
    let peer = resolve_invite(&net, &invite)
        .ok_or_else(|| "that code is for a device that is not nearby".to_string())?;
    // The start marker for the budget: the moment a code was successfully read,
    // which is where the ceremony proper begins. Time spent aiming the camera
    // before a decode lands is not in this and cannot be — nothing observes it
    // but the person holding the phone.
    log::info!("starting a ceremony with {peer}");
    net.begin_pairing(&peer, &invite, std::time::Instant::now())?;
    Ok(peer)
}

/// Which device a code belongs to.
///
/// The hint first, because it is right whenever the code is fresh and costs
/// nothing to try. Then Discovery, matched on the Layer-2 key the code names —
/// which is what makes a code that has been on screen across a rotation still
/// work. Without the fallback a perfectly good code fails after twelve minutes
/// for a reason nobody holding the phone could guess.
///
/// The hint is never trusted beyond being an address: whether the device there
/// is the one on the code is settled by the ceremony's own check, not here.
fn resolve_invite(net: &net::Net, invite: &Invite) -> Option<String> {
    let sightings = net.discovery().sightings();
    if !invite.ble_hint.is_empty() && sightings.iter().any(|s| s.peer == invite.ble_hint) {
        return Some(invite.ble_hint.clone());
    }
    sightings
        .into_iter()
        .find(|s| {
            s.persona
                .as_ref()
                .is_some_and(|p| p.l2_pub == invite.l2_pub)
        })
        .map(|s| s.peer)
}

/// This device's human confirmed the colours.
pub fn confirm_pairing(device_id: String) -> Result<(), String> {
    let net = require_net()?;
    for event in net.confirm_pairing(&device_id, std::time::Instant::now())? {
        on_net_event(&net, event);
    }
    Ok(())
}

/// Abandon a ceremony. Nothing was written, so nothing is undone.
pub fn cancel_pairing(device_id: String) -> Result<(), String> {
    require_net()?.cancel_pairing(&device_id);
    Ok(())
}

/// Write down a completed ceremony: the contact, its real identity, the thread.
///
/// The contact may already exist under a device id or a session pseudonym —
/// people chat and Ping before they pair (R0-F3, R0-F5) — so this adopts that
/// row rather than opening a second one, and only now fills in the Layer-2 key
/// and the persona, which the ceremony is the first thing to actually prove.
fn record_pairing(
    device_id: &str,
    name: &str,
    colour: u32,
    version: u32,
    l2_pub: &[u8; 32],
    l1_pub: &[u8; 32],
) -> Result<i64, String> {
    with_core_mut(|core| {
        let now = now_millis();
        let contact = ensure_contact(core, device_id, now)?;
        core.store
            .record_pairing(contact, l2_pub, name, colour, version, l1_pub, now)
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
/// Resend a thread's queued messages, for the contract tests.
///
/// `pub` with a note, as [`open_store_for_test`]: reunion is a transport event
/// no test can stage through the public API, and "the backlog goes out in
/// order, and only once" is the whole of R0-F5's reunion promise.
pub fn resend_queued_for_test(device_id: &str) -> Result<(), String> {
    resend_queued(device_id)
}

/// What a reunion would send, as `(seq, hex msg_id)`, for the contract tests.
///
/// `pub` with a note, as [`open_store_for_test`]. The sending needs a transport
/// and a peer; the *decision* — which messages, in what order, under which
/// identifiers — is the part with the rules in it. Every one of those rules
/// survived a mutant until this existed, because a test that watches only the
/// store passes whether anything was sent or not.
pub fn queued_for_resend_for_test(device_id: &str) -> Result<Vec<(u64, String)>, String> {
    let owed = with_core(|core| queued_for_resend(core, device_id))?;
    Ok(owed
        .into_iter()
        .map(|(_, e)| (e.seq, hex::encode(e.msg_id)))
        .collect())
}

/// How many of a thread's outgoing messages are still waiting, for the contract
/// tests.
///
/// `pub` with a note, as [`open_store_for_test`]: the difference between a
/// message held and a message sent is invisible from the public API — the DTO
/// carries no state — and it is the whole of what F3 promises about a closed
/// Discovery.
pub fn queued_on_thread_for_test(thread_id: i64) -> Result<usize, String> {
    with_core(|core| {
        core.store
            .count_in_state(thread_id, Direction::Outgoing, MessageState::Queued)
    })
}

/// Mark every queued message on a thread as sent, for the contract tests.
///
/// `pub` with a note, as [`open_store_for_test`]: reaching `Sent` needs a
/// transport, and "a reunion does not resend what already went" is a rule about
/// that state.
pub fn mark_sent_for_test(thread_id: i64) -> Result<(), String> {
    with_core(|core| {
        for m in
            core.store
                .messages_in_state(thread_id, Direction::Outgoing, MessageState::Queued)?
        {
            core.store
                .set_message_state(&m.msg_id, MessageState::Sent)?;
        }
        Ok(())
    })
}

/// Open a session with sighted devices we are not talking to, when a message is
/// waiting for one of them.
///
/// R0-F5 says queued messages are "delivered at the next direct encounter", and
/// `resend_queued` does exactly that — on `SessionOpened`, which nothing but a
/// manual Ping ever produced. So a message queued for someone out of range sat
/// there while they stood next to you, because no part of the app was curious
/// enough to ask who had just appeared. The reunion machinery was complete and
/// unreachable.
///
/// Identity is why asking is necessary rather than merely convenient: R0-F2's
/// transport ids rotate and are unlinkable, so a sighting we cannot name cannot
/// be matched to the contact a message is waiting for by looking at it. Only a
/// session says who is there.
///
/// # Why "not connected" and not "not named"
///
/// This gated on `persona.is_none()` first, which reads like the same question
/// and is not. Discovery keeps a verified persona after the session that proved
/// it has gone — `note_persona` outlives `SessionClosed`, which does not even
/// emit `PeersChanged` — so a peer whose idle session was swept stays named and
/// was skipped here forever after. That is precisely the person most likely to
/// be owed something: someone we have talked to before.
///
/// Gated on something actually being owed, and on it being owed to *them* where
/// we can tell. Reaching every sighting on sight would be a handshake nobody
/// asked for, paid for in radio time, battery (N4) and — on BLE, which has run
/// out of them before — connection slots. A peer we can name is dialled only if
/// their thread has something queued; a peer we cannot name might be anyone,
/// including the one we owe, so they are worth the one dial that answers it.
///
/// # What a test covers, and what it does not
///
/// That this runs at all is covered: `a_queued_message_goes_out_when_she_appears`
/// queues a message for someone absent, brings her onto the loopback rung, and
/// pumps only *her* side — so the session can only form if this device went and
/// asked. Deleting the call fails it. An earlier note here said no test could,
/// which was wrong: the engine's own persona fetch opens a pipe but never a
/// session, so reaching really is the only thing that closes the gap.
///
/// The three narrowing mutants survive — dropping the not-connected filter, the
/// owed gate, or the whose-debt check. Each makes this dial *more* than it needs
/// to, and every assertion still holds, because nothing observes the cost of a
/// dial nobody needed. That cost is the reason they are here (radio time, N4
/// battery, and BLE connection slots, which we have run out of before), so they
/// are recorded rather than left to look like an oversight.
///
/// Two phones checked the whole path first: a message written to a paired peer
/// who had gone was delivered the moment she came back, with nobody pressing
/// anything. The Pixel logged `resending 1 queued messages on reunion` and the
/// Samsung showed the message.
fn reach_for_queued_messages() {
    let Ok(net) = require_net() else {
        return;
    };
    // Everyone in sight we are not already talking to. `reach` no-ops on an
    // open session by itself; asking here is what keeps the store work below
    // off the path a connected peer takes on every discovery event.
    let candidates: Vec<(String, Option<[u8; 32]>)> = net
        .discovery()
        .sightings()
        .into_iter()
        .filter(|s| !net.sessions().is_open(&s.peer))
        .map(|s| (s.peer, s.persona.map(|p| p.l2_pub.0)))
        .collect();
    if candidates.is_empty() {
        return;
    }
    let wanted = with_core(|core| {
        let owed = core
            .store
            .threads_in_state(Direction::Outgoing, MessageState::Queued)?;
        if owed.is_empty() {
            return Ok(Vec::new());
        }
        let mut wanted = Vec::new();
        for (peer, l2) in candidates {
            // A named peer we have no contact for is not someone we can rule
            // out — `paired_contact_by_l2` answers for paired contacts only,
            // and a message can be queued for a stranger. Unknown means dial.
            let theirs = match l2 {
                Some(l2) => match core.store.paired_contact_by_l2(&l2)? {
                    Some(c) => core
                        .store
                        .thread_for_contact(c.id)?
                        .is_some_and(|thread| owed.contains(&thread)),
                    None => true,
                },
                None => true,
            };
            if theirs {
                wanted.push(peer);
            }
        }
        Ok(wanted)
    })
    .unwrap_or_default();
    for peer in wanted {
        // Best effort and quiet: a peer that will not answer is the ordinary
        // case, not an error worth a line each time round the loop.
        let _ = net.reach(&peer);
    }
}

/// Send everything this thread still owes, oldest first.
///
/// # What counts as owed
///
/// `Queued` — written, never away. `Sent` is deliberately excluded: without an
/// acknowledgement protocol we cannot tell a delivered message from a lost one,
/// and resending every message ever sent on every reunion would be worse than
/// the gap it covers. Narrowing that to "sent but unacknowledged" is what an
/// ack lands with.
///
/// # Why the outbox rather than the rows directly
///
/// The store can already answer "what is queued", and for a while this did
/// exactly that. What it cannot answer on its own is the two rules that make a
/// resend safe: send them in `seq` order, and never hold more than
/// [`MAX_UNACKED`]. Both live in [`Outbox`], resumed here from the rows —
/// which is the arrangement its own doc describes, the store holding the
/// messages and the outbox holding the decision.
///
/// Order is the one that bites. Out of order, every message sits in the far
/// side's `ahead` set until the lowest happens to arrive, so a reunion's whole
/// backlog stays invisible until its first message lands.
fn queued_for_resend(
    core: &Core,
    device_id: &str,
) -> Result<Vec<(Vec<u8>, ChatEnvelope)>, StoreError> {
    let Some(contact) = contact_id_for_device(core, device_id)? else {
        return Ok(Vec::new());
    };
    let Some(thread) = core.store.thread_for_contact(contact)? else {
        return Ok(Vec::new());
    };
    // Scoped in SQL. Direction is part of it even though `Queued` is only ever
    // written by the send path, so state already implies it — "the outgoing
    // messages this thread still owes" is what the function means, and saying
    // so costs nothing here.
    let queued = core
        .store
        .messages_in_state(thread, Direction::Outgoing, MessageState::Queued)?;
    if queued.is_empty() {
        return Ok(Vec::new());
    }
    let next = core.store.next_seq(thread, Direction::Outgoing)?;
    let outbox = Outbox::resumed(
        u64::try_from(next).unwrap_or(1),
        queued.iter().filter_map(|m| u64::try_from(m.seq).ok()),
    );
    // Rebuilt from the row, not from anything held in memory: the `seq` and
    // `msg_id` the far side already saw are in the row, and a resend that
    // invented either would arrive as a new message rather than a repeat.
    Ok(outbox
        .unacked()
        .into_iter()
        .filter_map(|seq| {
            let m = queued
                .iter()
                .find(|m| u64::try_from(m.seq).ok() == Some(seq))?;
            let msg_id: [u8; MSG_ID_LEN] = m.msg_id.clone().try_into().ok()?;
            Some((
                m.msg_id.clone(),
                ChatEnvelope {
                    seq,
                    msg_id,
                    body: m.body.clone(),
                },
            ))
        })
        .collect())
}

fn resend_queued(device_id: &str) -> Result<(), String> {
    let pending = with_core(|core| queued_for_resend(core, device_id))?;

    if pending.is_empty() {
        return Ok(());
    }
    let net = require_net()?;
    log::info!("resending {} queued messages on reunion", pending.len());
    for (msg_id, envelope) in pending {
        // One at a time, and the state moves per message: a send that fails
        // half way leaves the ones that got away marked `Sent` and the rest
        // still `Queued`, so the next reunion picks up exactly where this
        // stopped rather than starting again.
        // Flattened, unlike the first send: this runs because a session just
        // opened, so there is no out-of-range case left to tell apart — a
        // refusal here is a refusal. Stopping is right either way, because
        // order matters and a peer who has dropped will not take the next one.
        net.send_chat(device_id, envelope.encode(), std::time::Instant::now())
            .map_err(|why| why.to_string())?;
        with_core(|core| {
            core.store.set_message_state(&msg_id, MessageState::Sent)?;
            Ok(())
        })?;
    }
    Ok(())
}

fn require_net() -> Result<Arc<net::Net>, String> {
    with_core(|core| Ok(core.net.clone()))?
        .ok_or_else(|| "no radio available on this device".to_string())
}

/// Turn what `Net` reports into events Dart can act on, and persist what needs
/// persisting. Runs on the pump thread, never while the store lock is held.
fn on_net_event(net: &Arc<net::Net>, event: net::NetEvent) {
    match event {
        net::NetEvent::PeersChanged => {
            reach_for_queued_messages();
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
            // R0-F5: "queued messages deliver on reunion with no user action".
            // This is the reunion. Reported rather than propagated — a resend
            // that fails is a resend to try again on the next one, and taking
            // the session down over it would make the next one further away.
            //
            // The call itself is the one part of this with no test on it: a
            // `SessionOpened` is a transport event, and nothing reaches this
            // arm from the public API. What it calls is covered — see
            // `queued_for_resend_for_test` — so the untested step is the wiring
            // rather than the rules.
            if let Err(why) = resend_queued(&peer) {
                log::warn!("could not resend to {peer}: {why}");
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
        net::NetEvent::PairingSas { peer, sas } => {
            // Not logged with its contents. The SAS is the one value a relaying
            // attacker is trying to guess and the only secret the two humans
            // hold that the protocol does not, so writing it to a log — which
            // outlives the ceremony and leaves the device in a diagnostics
            // export — is the one place it must not go.
            log::info!("ceremony with {peer} reached the colours");
            emit(CoreEvent::PairingSas {
                device_id: peer,
                colours: sas
                    .colours
                    .iter()
                    .map(|c| SasColourDto {
                        name: c.name.to_string(),
                        rgb: c.rgb,
                    })
                    .collect(),
                word: sas.word.to_string(),
            });
        }
        net::NetEvent::PairingPeerConfirmed { peer } => {
            emit(CoreEvent::PairingPeerConfirmed { device_id: peer });
        }
        net::NetEvent::PairingCompleted {
            peer,
            persona_name,
            persona_colour,
            persona_version,
            l2_pub,
            l1_pub,
        } => {
            // Written down before it is announced. An event that arrives ahead
            // of the row sends the UI to a thread that does not exist yet, and
            // R0-F4 makes the thread part of what pairing *is* rather than a
            // consequence of it.
            match record_pairing(
                &peer,
                &persona_name,
                persona_colour,
                persona_version,
                &l2_pub,
                &l1_pub,
            ) {
                Ok(thread_id) => {
                    // The end marker for R0-F4's fifteen-second budget, which
                    // can only be measured on two real devices — nothing in a
                    // test harness has a radio or a person in it. Pairs with
                    // the "starting a ceremony" and "reached the colours" lines
                    // to give the two intervals that matter separately: what
                    // the protocol costs, and what the two humans cost.
                    //
                    // No SAS in it, for the reason above. `thread_id` is a
                    // local row number and names nobody.
                    log::info!("paired with {peer} on thread {thread_id}");
                    emit(CoreEvent::PairingCompleted {
                        device_id: peer,
                        thread_id,
                        name: persona_name,
                        colour: persona_colour,
                    });
                    // The nearby list carries a `paired` flag, and a pairing is
                    // exactly the event that changes it. Without this the badge
                    // waits for some unrelated peer movement to rebuild the
                    // list — on two phones the tile still read "nearby" a
                    // minute after pairing, and only a Discovery toggle brought
                    // it right. `SessionOpened` and `SessionClosed` refresh for
                    // the same reason; this was the one that did not.
                    if let Ok(devices) = nearby_devices() {
                        emit(CoreEvent::DiscoveryUpdated { devices });
                    }
                }
                Err(why) => {
                    // The ceremony succeeded and the store did not. Reported as
                    // a failed pairing because that is what it is from here:
                    // there is no thread to open and nothing to come back to.
                    log::error!("could not record the pairing with {peer}: {why}");
                    emit(CoreEvent::PairingFailed {
                        device_id: peer,
                        reason: "pairing could not be saved".into(),
                    });
                }
            }
        }
        net::NetEvent::PairingFailed { peer, why } => {
            emit(CoreEvent::PairingFailed {
                device_id: peer,
                reason: why,
            });
        }
        net::NetEvent::ChatReceived { peer, envelope } => {
            let _ = net;
            if let Ok(Some(event)) = store_incoming_chat(&peer, &envelope) {
                emit(event);
            }
        }
    }
}

/// The stored `(seq, msg_id)` of a thread's rows, in insertion order.
///
/// `pub` with a note, as [`open_store_for_test`]. Neither value reaches
/// [`ChatMessageDto`]: display order is the rowid, and `seq` is per-sender so
/// it is not a display key. But both are what a resend, an acknowledgement and
/// a gap will all be matched on, so "the row says what the wire said" needs to
/// be assertable from somewhere.
pub fn thread_rows_for_test(thread_id: i64) -> Result<Vec<(i64, String)>, String> {
    with_core(|core| {
        let mut msgs = core.store.messages_for_thread(thread_id)?;
        msgs.sort_by_key(|m| m.id);
        Ok(msgs
            .into_iter()
            .map(|m| (m.seq, hex::encode(&m.msg_id)))
            .collect())
    })
}

/// Deliver an inbound chat message as the network would, for the contract
/// tests.
///
/// `pub` with a note, following [`open_store_for_test`]: `#[cfg(test)]` items
/// are invisible to `tests/`, and the property worth testing here — that the
/// same envelope arriving twice is one message — cannot be reached through the
/// public API, which has no way to make a message arrive.
pub fn receive_chat_for_test(
    device_id: &str,
    envelope: &ChatEnvelope,
) -> Result<Option<CoreEvent>, String> {
    store_incoming_chat(device_id, envelope)
}

/// Write an inbound chat line and return the event announcing it, or `None` if
/// we already had it.
///
/// # The sender's numbers, not ours
///
/// Both identifiers come off the envelope. Inventing them here — which is what
/// this did — meant the same message arriving twice looked like two different
/// messages, so a resend after a severance the sender never saw acked put a
/// second copy on the screen. That is the case `seq` and `msg_id` exist for,
/// and the case the ratchet cannot catch, because a resend is genuinely fresh
/// ciphertext it has never seen.
fn store_incoming_chat(
    device_id: &str,
    envelope: &ChatEnvelope,
) -> Result<Option<CoreEvent>, String> {
    with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, device_id, now)?;
        // The peer chooses this number and `decode` accepts any `u64`, so the
        // conversion is checked and a message that will not fit is dropped.
        // Cast instead, anything above `i64::MAX` lands as a *negative* seq —
        // which sorts before every real message, and would let one frame from
        // a stranger reorder a conversation permanently.
        let Ok(seq) = i64::try_from(envelope.seq) else {
            log::warn!("dropping a chat message whose seq will not fit the store");
            return Ok(None);
        };
        // Where the thread had got to, restored from the store. A fresh
        // `Inbox` on every launch would forget everything already delivered,
        // so the first message after a restart would look new however many
        // times it had arrived before.
        let position = core.store.inbox_position(thread)?.unwrap_or_default();
        let mut inbox = Inbox::resumed(position.through, position.ahead);

        match inbox.receive(envelope.seq) {
            Delivery::Accepted => {}
            Delivery::Duplicate => {
                log::debug!("dropping a chat message we already have");
                return Ok(None);
            }
            Delivery::TooFarAhead => {
                // Refused rather than accepted with an enormous gap behind it.
                // Tracking that gap is memory a peer could ask for by picking a
                // number, and closing over it silently is the loss R0-F5 exists
                // to prevent — so the honest answer is to take neither.
                log::warn!("refusing a chat message too far ahead of the conversation");
                return Ok(None);
            }
        }

        let outcome = core.store.commit_received(
            // No ratchet on this path yet: an unpaired thread has none, and a
            // paired one will land with the ratchet that decrypted it.
            None,
            &InboxPosition {
                through: inbox.through(),
                ahead: inbox.ahead(),
            },
            &NewMessage {
                thread_id: thread,
                // As the sender numbered it. Per-sender numbering is what makes
                // this safe to store directly (tech spec §8).
                seq,
                msg_id: envelope.msg_id.to_vec(),
                body: envelope.body.clone(),
                direction: Direction::Incoming,
                state: MessageState::Delivered,
                created_at: now,
            },
            now,
        )?;
        // Both checks, and not the same check twice. The inbox refuses a `seq`
        // it has already seen; the store refuses a `msg_id` it already holds. A
        // sender that reused a number for different content would pass the
        // first, and one that resent after we lost our position would pass the
        // second — neither covers the other's case.
        if outcome == InsertOutcome::Duplicate {
            log::debug!("dropping a chat message the store already had");
            return Ok(None);
        }
        Ok(Some(CoreEvent::MessageReceived {
            thread_id: thread,
            msg_id: hex::encode(envelope.msg_id),
            text: String::from_utf8_lossy(&envelope.body).into_owned(),
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

/// Whether a completed ceremony stands behind this device (R0-F4).
///
/// Was hardcoded `false` until there was a ceremony to answer it — the honest
/// value while pairing did not exist, and a lie the moment it did.
///
/// Two ways to answer, and they are not equally good.
///
/// **With a live session** the peer's pseudonym has been proved by a handshake,
/// so the contact it resolves to is the right one and the answer is as sound as
/// anything here gets.
///
/// **Without one** all that exists is the sighting, and the first version of
/// this asked the session anyway. That is wrong in the case that matters most:
/// after pairing, walking away and coming back, there is no session until
/// something dials — so a paired friend's tile read "not paired", which is
/// precisely the state pairing exists to survive. The fallback matches the
/// Layer-2 key in the sighting's persona against paired contacts.
///
/// The limit, plainly: **a persona record is public and replayable**, so a
/// device broadcasting someone else's record gets their badge until a session
/// disproves it. That is the same evidence the name and colour on the tile
/// already come from — an unpaired tile is unauthenticated all the way through,
/// and this does not make it more so. It does mean nothing may *act* on this
/// flag; R0-F10's rule stands, that decisions key on the proved pseudonym.
///
/// A read failure reports "not paired": the alternative is propagating a store
/// error out of a list refresh, and a tile missing a badge beats no list.
/// A transport handle for this thread's peer, if they are in front of us.
///
/// Discovery off is `None` rather than a search of remembered sightings: off
/// means we are neither advertising nor scanning, so a peer id from before is a
/// guess about the present, and dialling it is exactly the traffic F3 says a
/// closed Discovery must not produce.
fn device_for_thread(core: &Core, thread: i64) -> Result<Option<String>, StoreError> {
    let Some(net) = core.net.as_ref() else {
        return Ok(None);
    };
    if !net.discovery().is_on() {
        return Ok(None);
    }
    for s in net.discovery().sightings() {
        if let Some(contact) = contact_for_sighting(core, &s.peer, s.persona.as_ref())? {
            if core.store.thread_for_contact(contact)? == Some(thread) {
                return Ok(Some(s.peer));
            }
        }
    }
    Ok(None)
}

/// Which contact a sighting belongs to, if we have one on file.
///
/// Two routes, in this order. A session pseudonym is proved, so it wins; the
/// persona a sighting carries is only claimed, and is consulted when no session
/// has answered — which is the ordinary case for someone who has just appeared.
/// Creates nothing: a stranger stays a stranger, and drawing the nearby list is
/// not the moment to start writing rows.
fn contact_for_sighting(
    core: &Core,
    device_id: &str,
    persona: Option<&VerifiedPersona>,
) -> Result<Option<i64>, StoreError> {
    if let Some(id) = contact_id_for_device(core, device_id)? {
        return Ok(Some(id));
    }
    let Some(persona) = persona else {
        return Ok(None);
    };
    Ok(core
        .store
        .paired_contact_by_l2(&persona.l2_pub.0)?
        .map(|c| c.id))
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
        if let Some(c) = core.store.contact_by_pseudonym(&real)? {
            return Ok(Some(c.id));
        }
    }
    Ok(core
        .store
        .contact_by_pseudonym(&fake::placeholder_pseudonym(device_id))?
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
        .contact_by_pseudonym(&fake::placeholder_pseudonym(device_id))?
        .map(|c| c.id)
    else {
        return Ok(());
    };
    match core.store.contact_by_pseudonym(&real)?.map(|c| c.id) {
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
    let key =
        pseudonym_of(core, device_id).unwrap_or_else(|| fake::placeholder_pseudonym(device_id));
    let (name, colour) = fake::peer(device_id)
        .map(|p| (p.name.to_owned(), p.colour))
        .unwrap_or_else(|| ("Unknown".into(), 0));
    core.store.add_contact(&NewContact {
        pseudonym: key,
        l2_pub: [0u8; 32], // placeholder — real Layer-2 arrives with pairing (T08–T10)
        name,
        colour,
        persona_version: 1,
        first_seen: now,
    })
}

fn ensure_thread(core: &Core, device_id: &str, now: i64) -> Result<i64, StoreError> {
    let contact_id = ensure_contact(core, device_id, now)?;
    match core.store.thread_for_contact(contact_id)? {
        Some(t) => Ok(t),
        None => core.store.create_thread(contact_id, now),
    }
}

fn persona_dto(p: &Persona, needs_name: bool) -> PersonaDto {
    PersonaDto {
        name: p.name.clone(),
        colour: p.colour,
        version: p.version,
        needs_name,
    }
}

/// Whether anyone has chosen this device's name.
///
/// The stored persona *is* the answer: nothing is written down until somebody
/// chooses one, so there is no separate flag able to disagree with it. A
/// persona too damaged to read also lands here, and asking again is the right
/// answer to that — the name genuinely is not known.
/// For the persistence tests; see [`open_store_for_test`].
pub fn needs_name_for_test(store: &Store) -> bool {
    needs_name(store)
}

/// Run a launch's identity load and report whether a name is still wanted.
///
/// The whole launch path, not its parts: whether the placeholder gets written
/// down is decided inside `load_identity`, and a test that opened a store and
/// asked `needs_name` never went near it. Mutation testing said exactly that —
/// adding a `store_persona` back into the first-launch branch broke nothing.
pub fn launch_needs_name_for_test(support_dir: String) -> Result<bool, String> {
    let (store, keystore) = open_store(support_dir)?;
    load_identity(&store, keystore.as_ref())?;
    Ok(needs_name(&store))
}

fn needs_name(store: &Store) -> bool {
    // Written as "not a readable persona" rather than "no persona", so a store
    // that cannot be read asks again. Only reachable when `settings_get` itself
    // fails, which no test here can arrange, so the mutation to the narrower
    // form survives — it is a difference in what happens when the database is
    // unreadable, and asking is the safer of the two answers.
    !matches!(stored_persona(store), Ok(Some(_)))
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
/// Delete a database whose key is gone.
///
/// Only reachable when no master is sealed and a database file exists — a
/// combination that means the ciphertext is provably unkeyable, so nothing is
/// being destroyed that was not already lost. That reasoning was always right
/// and the precondition was always wrong: with an in-memory keystore no master
/// was *ever* sealed, so this ran on every launch and threw away a database it
/// could have opened. It is loud now, because a line that deletes everything
/// should say so on the one occasion it is correct to.
/// Where the persona lives between launches.
const PERSONA_KEY: &str = "identity/persona/v1";

/// What a device is called before anyone has said otherwise.
///
/// A placeholder, and it has been shipping as the real thing: R0-F1 says the
/// person chooses a name, no step ever asks, so every install is called "Me"
/// and the pairing screen reads "Pairing with Me" — on the one screen whose job
/// is saying who is at the other end. The first-launch step is what retires
/// this; until then it is at least persisted rather than reinvented hourly.
const DEFAULT_NAME: &str = "Me";

/// Colours a device may be given before anyone picks one.
///
/// One fixed colour for every install made the nearby list a column of
/// identical dots, which is a small thing until two people are trying to work
/// out which entry is which. Drawn at random rather than derived from anything
/// about the device: a colour derived from a hardware identifier would be a
/// stable handle that survives the twelve-minute rotation R0-F2 exists to
/// enforce, and this is shown to strangers by design (R0-F2 again).
///
/// Five bits of entropy at most, so it is a convenience and never an identity.
/// The palette, with the words for it.
///
/// Named rather than a bare list of numbers because the row of swatches on the
/// first-launch screen has to be usable by somebody who cannot tell two of
/// these apart, which is roughly one man in twelve. The name is what a screen
/// reader says and what labels the swatch when the colour cannot.
const DEFAULT_COLOURS: [(&str, u32); 8] = [
    ("blue", 0x0044_88ff),
    ("coral", 0x00e0_5c4a),
    ("green", 0x0037_a86b),
    ("violet", 0x00b3_6ae2),
    ("amber", 0x00e0_9c3a),
    ("teal", 0x0027_9b9b),
    ("rose", 0x00d4_5c8a),
    ("slate", 0x006b_7280),
];

/// The palette offered on first launch. See [`DEFAULT_COLOURS`].
pub fn persona_colours() -> Vec<(&'static str, u32)> {
    DEFAULT_COLOURS.to_vec()
}

fn a_starting_colour() -> u32 {
    let pick = rng::random_array::<1>()[0] as usize;
    DEFAULT_COLOURS[pick % DEFAULT_COLOURS.len()].1
}

/// The identity this device sealed, or a fresh one on first launch.
///
/// # Why failing to seal is fatal
///
/// A device that cannot persist its identity is a device that is a different
/// person every launch — which is exactly the bug this replaces, and it is
/// invisible from the inside: pairing works, chat works, and then everything
/// silently belongs to somebody who no longer exists. Refusing to start says
/// so once, loudly, instead.
fn load_identity(store: &Store, keystore: &dyn Keystore) -> Result<Identity, String> {
    // A persona that failed to store is not a reason to throw away keys, so the
    // default stands in and only the name is lost.
    let persona = stored_persona(store)?.unwrap_or_else(|| Persona {
        name: DEFAULT_NAME.to_string(),
        colour: a_starting_colour(),
        version: 1,
    });

    if let Some(identity) =
        Identity::unsealed(keystore, persona.clone()).map_err(|e| e.to_string())?
    {
        return Ok(identity);
    }

    let identity = Identity::generate(persona.name, persona.colour);
    identity.seal_seeds(keystore).map_err(|e| {
        format!(
            "this device cannot store its identity, so it would be a new person every launch: {e}"
        )
    })?;
    // Deliberately *not* stored. The persona row is written only when somebody
    // chooses a name, which is what makes its absence mean "never chosen" — a
    // separate flag would be a second answer to the same question, and the two
    // would drift.
    log::info!("first launch: a new identity was generated and sealed");
    Ok(identity)
}

/// `[version:4][colour:4][name]`, big-endian.
///
/// Hand-rolled and local-only, like the ratchet's stored state and for the same
/// reason: a `.proto` would generate Dart types for something the UI has no
/// business reading. Not signed either — this sits inside the SQLCipher
/// database, which authenticates it already, and a second signature would need
/// the key it is stored alongside.
fn encode_persona(persona: &Persona) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + persona.name.len());
    out.extend_from_slice(&persona.version.to_be_bytes());
    out.extend_from_slice(&persona.colour.to_be_bytes());
    out.extend_from_slice(persona.name.as_bytes());
    out
}

/// For the persistence tests; see [`open_store_for_test`].
pub fn store_persona_for_test(store: &Store, persona: &Persona) -> Result<(), String> {
    store_persona(store, persona)
}

/// For the persistence tests; see [`open_store_for_test`].
pub fn stored_persona_for_test(store: &Store) -> Result<Option<Persona>, String> {
    stored_persona(store)
}

fn store_persona(store: &Store, persona: &Persona) -> Result<(), String> {
    store
        .settings_set(PERSONA_KEY, &encode_persona(persona))
        .map_err(stringify)
}

/// Read it back, refusing anything this build did not write.
///
/// Every invariant the writer holds is checked, because a sentence promising
/// that and eight bytes of length check is how a corrupt record becomes a live
/// persona: a version of zero is one this build never wrote (they start at 1
/// and only rise), a colour is 24 bits so its top byte is always clear, and a
/// name is bounded at [`MAX_PERSONA_NAME_LEN`].
///
/// All of it is `None` rather than an error, and that is the point of checking
/// so carefully: the keys are sealed separately, so a damaged persona costs a
/// name and not an identity — falling back to the default is a recoverable
/// afternoon, and refusing to start over a display name is not.
fn stored_persona(store: &Store) -> Result<Option<Persona>, String> {
    let Some(bytes) = store.settings_get(PERSONA_KEY).map_err(stringify)? else {
        return Ok(None);
    };
    let refuse = |why: &str| {
        log::warn!("stored persona ignored, falling back to the default: {why}");
        Ok(None)
    };
    // Length before anything else, the same order `envelope::decode` and
    // `verify_persona_record` use. Validating UTF-8 first would walk whatever
    // the row happens to contain before deciding it was too long to keep —
    // work a corrupt row gets to choose the size of, on the startup path.
    if bytes.len() < 8 {
        return refuse(&format!("{} bytes, too short to read", bytes.len()));
    }
    if bytes.len() > 8 + MAX_PERSONA_NAME_LEN {
        return refuse(&format!(
            "{} bytes, longer than a persona can be",
            bytes.len()
        ));
    }
    let version = u32::from_be_bytes(bytes[..4].try_into().expect("4 bytes"));
    let colour = u32::from_be_bytes(bytes[4..8].try_into().expect("4 bytes"));
    let Ok(name) = std::str::from_utf8(&bytes[8..]) else {
        return refuse("the name is not text");
    };
    if version == 0 {
        return refuse("version 0, which is never written");
    }
    if colour & !COLOUR_MASK != 0 {
        return refuse(&format!("colour {colour:#010x} is not 24-bit"));
    }
    Ok(Some(Persona {
        version,
        colour,
        name: name.to_string(),
    }))
}

fn reset_stale_db(db: &Path) -> Result<(), String> {
    log::warn!(
        "no sealed master and a database at {} — resetting it, as nothing can \
         decrypt it",
        db.display()
    );
    std::fs::remove_file(db).map_err(|_| "could not reset stale database".to_string())?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = db.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
    Ok(())
}
