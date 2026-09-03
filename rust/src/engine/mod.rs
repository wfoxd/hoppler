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
    ChatMessageDto, CoreEvent, MessageStateDto, NearbyDevice, PersonaDto, SasColourDto,
    ThreadSummary,
};
use crate::block::{Admit, Blocklist, Handle};
use crate::crypto::rng;
use crate::discovery::{hint, Sighting};
use crate::frb_generated::StreamSink;
use crate::identity::filekeystore::FileKeystore;
use crate::identity::keystore::Keystore;
use crate::identity::{Identity, Persona, VerifiedPersona};
use crate::identity::{COLOUR_MASK, MAX_PERSONA_NAME_LEN};
use crate::pairing::invite::Invite;
use zeroize::Zeroizing;

use crate::session::chat::{ChatEnvelope, Delivery, Inbox, Outbox, MAX_UNACKED, MSG_ID_LEN};
use crate::session::ratchet::{self, Ratchet};
use crate::store::{
    Direction, InboxPosition, InsertOutcome, MessageState, NewContact, NewMessage, NewTransfer,
    Pairing, Store, StoreError, TransferState,
};

struct Core {
    store: Store,
    /// Shared with [`net::Net`] rather than owned: the persona endpoint must
    /// serve what a rename produced, not what existed at start-up.
    identity: Arc<Mutex<Identity>>,
    /// The radio plane. `None` when no transport could be built — the app still
    /// runs, reports itself unavailable, and does not pretend to be reachable.
    net: Option<Arc<net::Net>>,
    /// Who this device refuses to hear from (R0-F10). The same `Arc` `Net` and
    /// `Discovery` enforce, held here too because the nearby list is a surface
    /// that has to ask and is not on the radio plane — a paired contact draws a
    /// row off the disk with no transport in sight.
    blocked: Arc<Blocklist>,
}

static CORE: Mutex<Option<Core>> = Mutex::new(None);
static EVENT_SINK: Mutex<Option<StreamSink<CoreEvent>>> = Mutex::new(None);

/// Events kept for a test to read — see [`emit`]. Empty unless a test asked.
static UNHEARD: Mutex<Vec<CoreEvent>> = Mutex::new(Vec::new());

/// Whether to keep events nobody is listening for. Off in every build that
/// runs on a phone; see [`record_events_for_test`].
static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
        return;
    }
    // Nobody listening. Dropped, exactly as before — unless a test asked to see
    // what it would have sent.
    //
    // Gated on an explicit opt-in rather than on the sink being absent, and
    // review is why. The sink is attached by `core_event_stream`, which Dart
    // calls *after* `core_init` — so "no sink" is a real state on a phone
    // during startup, not just in a test. Keeping events there would have held
    // up to 256 of them for ever and never delivered one, which is a leak and
    // a silence at the same time.
    if !RECORDING.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut unheard = UNHEARD.lock().unwrap_or_else(|e| e.into_inner());
    // Bounded: a long test run must not accumulate for ever.
    if unheard.len() < 256 {
        unheard.push(event);
    }
}

/// Keep what `emit` could not deliver, so a test can read it.
///
/// The sink is an `frb` type a test cannot build, so without this every `emit`
/// is invisible and "the screen is told" is a claim nothing can check. That is
/// how a conversation came to redraw on arrivals only: the rule was in a
/// comment and no test could reach it.
pub fn record_events_for_test() {
    RECORDING.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Stop keeping them, so a test can check the shape a phone actually runs in:
/// no sink yet, nobody recording, and nothing accumulating.
pub fn stop_recording_for_test() {
    RECORDING.store(false, std::sync::atomic::Ordering::Relaxed);
    drain_events_for_test();
}

/// Events emitted with nobody listening, oldest first, and cleared.
pub fn drain_events_for_test() -> Vec<CoreEvent> {
    std::mem::take(&mut *UNHEARD.lock().unwrap_or_else(|e| e.into_inner()))
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

    // Read before anything can answer a peer. This one line is the whole of
    // what makes a block outlive the process: the table has existed since the
    // first schema and nothing has ever loaded it.
    let blocked = Arc::new(Blocklist::loaded(
        store
            .list_blocks()
            .map_err(stringify)?
            .into_iter()
            .map(|b| b.handle),
    ));

    let net = transport.map(|t| {
        let net = Arc::new(net::Net::new(
            t,
            identity.clone(),
            blocked.clone(),
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
        blocked,
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
/// Holds a [`std::sync::Weak`] rather than an `Arc`: nothing shuts the engine
/// down today, but a second `init` replaces the core, and a clock still ticking
/// against the previous one would rotate ids on a transport no longer in use.
/// Losing the upgrade is how this thread learns it has been replaced.
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
        let known = Recogniser::read(core)?;
        let mut rows = Vec::new();
        let mut in_front_of_us = Vec::new();
        for s in sightings {
            let contact = contact_for_sighting(core, &s, &known)?;
            if let Some(id) = contact {
                // Recorded before the gate below can `continue`, on purpose: it
                // is what stops the loop over the disk drawing the same person
                // again.
                in_front_of_us.push(id);
            }
            // §12's fourth surface, and it asks about the *sighting* rather
            // than only about a contact. A blocked peer often resolves to
            // nobody: the block revoked their pairing, so the advert hint is
            // gone, and it tore down the session, so there is no proved
            // pseudonym either. Gating only on an attributed contact therefore
            // fails in exactly the case the block just created.
            if shut_out(core, contact, &s)? {
                continue;
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
                || shut_out_contact(core, contact.id)?
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

/// Whether a row about to be drawn belongs to somebody blocked (R0-F10).
///
/// Asks with everything the row could be identified by, because a block holds
/// every handle this device had for that person and any of them is enough —
/// see [`crate::block::Handle`]. In particular `contacts.pseudonym` is not
/// reliably a pseudonym: it holds whatever [`session_key_of`] supplied, which
/// is the peer's Layer-2 key on a session this device dialled, or a placeholder
/// derived from the rotating id when no session ever happened.
///
/// Takes the sighting as well as the contact, and works with either missing.
/// A blocked peer frequently resolves to no contact at all — blocking revoked
/// the pairing that carried the advert hint and tore down the session that
/// proved a pseudonym — so a gate that needed an attributed contact would fail
/// in precisely the state a block creates.
fn shut_out(core: &Core, contact: Option<i64>, seen: &Sighting) -> Result<bool, StoreError> {
    let mut handles = vec![fake::placeholder_pseudonym(&seen.peer)];
    if let Some(p) = seen.persona.as_ref() {
        handles.push(p.l2_pub.0);
    }
    if let Some(c) = match contact {
        Some(id) => core.store.contact_by_id(id)?,
        None => None,
    } {
        handles.push(c.pseudonym);
        handles.push(c.l2_pub);
    }
    Ok(core.blocked.ingress_gate(&handles) == Admit::Silence)
}

/// As [`shut_out`], for a contact drawn off the disk with no sighting at all.
fn shut_out_contact(core: &Core, contact: i64) -> Result<bool, StoreError> {
    let Some(c) = core.store.contact_by_id(contact)? else {
        return Ok(false);
    };
    Ok(core.blocked.ingress_gate(&[c.pseudonym, c.l2_pub]) == Admit::Silence)
}

/// Block a device (R0-F10).
///
/// Silent and local. Nothing goes to the person being blocked — not a frame,
/// not a close, not a persona — and after this returns every ingress in the app
/// refuses them; see [`crate::block`].
///
/// # What it binds to
///
/// **Every handle this device holds**, not the best one — see `handles_for`.
/// The Layer-1-derived pseudonym R0-F10 names is only learnable from a
/// handshake the *peer* opened, and which side dials is a comparison of two
/// rotating ids, so for roughly half of peers there is no pseudonym on file at
/// all and the strongest thing available is their Layer-2 persona key.
///
/// Recording only the strongest would leave a hole rather than merely a weaker
/// block: which handle a given surface can offer *also* depends on who dialled,
/// so a block holding one value is invisible to the surfaces that hold the
/// other. All of them are stored, all of them are enforced, and only the
/// pseudonym survives the peer regenerating their persona. T18b records why
/// offering the weaker ones beats refusing to block at all.
///
/// # Order
///
/// The list is updated before the session is torn down, and both after the
/// store has committed. The reverse order leaves a window in which the session
/// is gone, the list does not know yet, and a fresh handshake would be admitted.
pub fn block_device(device_id: String) -> Result<(), String> {
    // Contact, handles and write under **one** hold of the core lock. Deriving
    // the handles under one and revoking under another leaves a gap in which
    // the device's contact can move: `reconcile_contact` re-keys and merges
    // rows, and it runs from the session-open event on the pump thread. The
    // revocation would then name a different row than the handles came from —
    // a block that took a pairing away from the wrong person, or from nobody.
    let known = with_core(|core| {
        let contact = contact_id_for_device(core, &device_id)?;
        let handles = handles_for(core, &device_id, contact)?;
        if handles.is_empty() {
            return Ok(false);
        }
        core.store.block(&handles, contact, now_millis())?;
        // In force before anything else happens, including before the teardown
        // below can let a fresh handshake in.
        for (handle, _) in &handles {
            core.blocked.block(*handle);
        }
        Ok(true)
    })?;
    if !known {
        return Err("nothing known about that device to block".to_string());
    }

    // Outside the store closure: `close` takes the session table's lock, and
    // nothing in this crate holds the store lock across a network lock.
    if let Some(net) = with_core(|core| Ok(core.net.clone()))? {
        net.sessions().close(&device_id);
    }

    emit(CoreEvent::DiscoveryUpdated {
        devices: nearby_devices()?,
    });
    Ok(())
}

/// Every handle this device holds for a peer, each tagged with what it is.
///
/// **All of them, not the best one.** Which handle a surface can offer depends
/// on which side dialled — a session this device opened knows the peer's
/// Layer-2 key and not their pseudonym — and that flips every time the rotating
/// ids do. A block recording only the strongest value is therefore invisible to
/// half the surfaces that have to enforce it.
///
/// Empty only when the device id names nobody at all: no session, no sighting,
/// no contact row. There is then nothing to write that would mean anything, and
/// `block_device` refuses rather than storing a handle for a stranger it has
/// never seen.
///
/// `contact` is passed in rather than looked up here, so that the caller's
/// revocation and this handle set are derived from the same one — see
/// `block_device` on why a second lookup is a gap and not a tidy-up.
fn handles_for(
    core: &Core,
    device_id: &str,
    contact: Option<i64>,
) -> Result<Vec<([u8; 32], Handle)>, StoreError> {
    let mut out: Vec<([u8; 32], Handle)> = Vec::new();
    let mut add = |handle: [u8; 32], kind: Handle| {
        // The zero sentinel is `Request::UNKNOWN` and the `l2_pub` of a contact
        // whose persona was never fetched; `Blocklist::block` refuses it too,
        // and it must not reach the store either.
        if handle != [0u8; 32] && !out.iter().any(|(h, _)| h == &handle) {
            out.push((handle, kind));
        }
    };

    // The pseudonym, but only where the session really proves one — which is
    // only when the peer dialled. When this device dialled, the remote static
    // is the `session_pub` from their persona record, and recording that as a
    // pseudonym would credit the block with a durability it has not got.
    if let Some(net) = core.net.as_ref() {
        if let Some(p) = net.proved_pseudonym(device_id) {
            add(p, Handle::Pseudonym);
        }
        if let Some([_, l2_pub]) = net.session_handles(device_id) {
            add(l2_pub, Handle::PersonaKey);
        }
    }

    // Their Layer-2 key from a live sighting, and whatever the contact row
    // holds — which may be either kind, since `session_key_of` fed it.
    if let Some(p) = sighting_of(core, device_id).and_then(|s| s.persona) {
        add(p.l2_pub.0, Handle::PersonaKey);
    }
    if let Some(c) = match contact {
        Some(id) => core.store.contact_by_id(id)?,
        None => None,
    } {
        add(c.l2_pub, Handle::PersonaKey);
        // Weakest claim of the three: this column holds a proven pseudonym, a
        // Layer-2 key or a device-id placeholder depending on how the row was
        // made, and nothing on it says which.
        add(c.pseudonym, Handle::Device);
    }

    // The rotating id, hashed. Good until the next rotation and no longer,
    // which is sometimes the whole of what this device knows about them.
    if contact.is_some() || sighting_of(core, device_id).is_some() {
        add(fake::placeholder_pseudonym(device_id), Handle::Device);
    }
    Ok(out)
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

/// Why this thread cannot be written to, if it cannot.
///
/// `None` for the ordinary case — a stranger's thread, or the current one for a
/// paired contact. A thread belonging to a contact whose newest thread is some
/// other one is a finished conversation: pairing again opened its successor.
fn superseded(core: &Core, thread_id: i64) -> Result<Option<String>, StoreError> {
    let Some(contact) = core.store.contact_for_thread(thread_id)? else {
        return Ok(Some("that conversation is not on this device".into()));
    };
    if core.store.thread_for_contact(contact)? == Some(thread_id) {
        return Ok(None);
    }
    Ok(Some(
        "this conversation ended when you paired again — the newer one is open".into(),
    ))
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
    let (dto, envelope, wire, held) = with_core_mut(|core| {
        let now = now_millis();
        // A conversation that ended when the two of you paired again cannot be
        // written to, and says so rather than taking the message.
        //
        // Refused rather than held, which is the opposite of what every other
        // "cannot send right now" does here — because this one is never going
        // to become sendable. Held, it would sit `Queued` for ever behind a
        // peer who is perfectly reachable on the thread next to it. And it is
        // not only tidiness: a superseded thread has no ratchet,
        // `seal_for_thread` reads that as an unpaired stranger, and the body
        // would go out in the clear if anything ever did send it.
        //
        // Inside this lock, and immediately before the seal, because that is
        // the only place the answer stays true. Asked in `send_chat_to_thread`
        // it was two lock acquisitions earlier: a re-pairing arriving on the
        // net thread in between supersedes the thread and deletes its ratchet,
        // and the send that had just been told it was fine goes out in the
        // clear.
        if let Some(why) = superseded(core, thread)? {
            return Err(StoreError::Db(why));
        }
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
        // Sealed before the row is written, because the row is what makes the
        // advanced ratchet durable. See `commit_sent`: the counter must never
        // go backwards, and the only ordering that guarantees it is seal,
        // commit, send.
        // The envelope carries what the person typed, and the *whole* envelope
        // is what gets sealed — `seq` and `msg_id` included. Sealing only the
        // body left those two as the one part of a chat message the ratchet did
        // not cover, and put the receiver's decode ahead of its open: every
        // rejection that decode could make threw away a message that would have
        // opened, and the chain step the sender had already taken with it.
        let envelope = ChatEnvelope::new(seq_out, text.clone().into_bytes())
            .map_err(|e| StoreError::Db(e.to_string()))?;
        let sealed = seal_for_thread(core, thread, &envelope.encode())?;
        let held = matches!(sealed, Outgoing::NoChainYet);
        let (wire, advanced) = match sealed {
            Outgoing::Ready { body, ratchet } => (body, ratchet),
            // Nothing to send and nothing that could be sent. The envelope
            // above still exists, for the id it draws and the row below.
            Outgoing::NoChainYet => (Vec::new(), None),
        };
        // Everything below reads the envelope. There is deliberately no second
        // copy of the id: the row, the value handed back to the caller and the
        // bytes on the wire are three uses of one value rather than three
        // things that have to be kept in step. They cannot be checked against
        // each other from here — the engine is a singleton, so no test can
        // stand up a second one to watch the wire — so the guarantee has to be
        // that there is nothing to diverge.
        // The row keeps the *plaintext*: it is what the screen draws, and the
        // store is already encrypted at rest. The ciphertext is kept too, and
        // separately, but only while the message is `Queued` — see
        // `outbound_seals`. That is a reversal of what this comment used to
        // say: re-sealing at every reunion looked like the frugal choice until
        // it turned out to spend a message key each time, which is a budget
        // that cannot be refilled. A second copy of the *backlog* is a much
        // smaller thing than a second copy of every conversation, and it is
        // bounded by `MAX_UNACKED` and gone the moment the message goes.
        core.store.commit_sent(
            advanced.as_ref().map(|s| s.as_slice()),
            &NewMessage {
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
            },
            // What it will go out as, kept until it goes. Sealed once, so a
            // resend is the same bytes and costs no message key — see
            // `Store::commit_sent`. `None` on a thread whose chain has not
            // opened yet, where there was nothing to seal with; `resend_queued`
            // seals it when there is.
            (!held).then_some(wire.as_slice()),
            now,
        )?;
        let dto = ChatMessageDto {
            msg_id: hex::encode(envelope.msg_id),
            thread_id: thread,
            text: text.clone(),
            outgoing: true,
            created_at: now,
            // Queued until the send below says otherwise, which is what the row
            // says too — this is not a guess about what is about to happen.
            state: MessageStateDto::Queued,
        };

        Ok((dto, envelope, wire, held))
    })?;

    // Written down, and staying put. The other end of this thread has never
    // written to us, so this device has no chain to seal with yet — a window
    // that lasts from pairing until their opening arrives, and one somebody can
    // type into. `take_an_opening` sends the backlog the moment it closes.
    if held {
        log::info!("holding a message on thread {thread} until its other end opens");
        return Ok(dto);
    }

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
    let mut dto = dto;
    match net.send_chat(&device_id, wire, std::time::Instant::now()) {
        Ok(()) => {
            move_state(thread, &envelope.msg_id, MessageState::Sent)?;
            // And on the value handed back, which is the same fact and was two.
            // The row said `Sent` while the returned DTO still said `Queued`, so
            // a caller that drew what it was given — rather than re-reading the
            // thread — showed a message as waiting when the bytes had gone.
            // Every caller re-reads today, which is exactly why nothing caught
            // it and why it would have bitten the first one that did not.
            dto.state = MessageStateDto::Sent;
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
                state: state_dto(m.state),
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
    // Taken before the store lock, not inside it: `take_pairing_ratchet` reaches
    // into `Net`, and the engine's rule is that no `Net` call happens while the
    // core lock is held.
    let seed = require_net()
        .ok()
        .and_then(|net| net.take_pairing_ratchet(device_id));
    // Refused rather than written down without one, and this is a change of
    // mind: it used to pair anyway with a line in the log, on the grounds that
    // the identities crossed, both people confirmed, and a ratchet can be
    // rebuilt. That was true while nothing used the ratchet. Now a paired
    // thread without one is read by the send path as a thread that is not
    // paired, and answered with **plaintext on the wire** — so the cheaper
    // outcome is the honest one: nothing written, and two people who pair
    // again.
    //
    // No test reaches this and none can through the public API: `seed_ratchet`
    // fails only on a peer key that is not usable or on pairing with our own
    // identity, and nothing in `tests/` can produce either against a real
    // ceremony. Recorded so the surviving mutant reads as what it is — a guard
    // against a state the harness cannot build — rather than as a hole to close
    // by deleting the guard. Its twin, the store writing the two as one
    // transaction, *is* covered: see
    // `recording_a_pairing_writes_identity_persona_pairing_and_thread`.
    let Some(seed) = seed else {
        return Err(format!("no ratchet was seeded for {device_id}"));
    };
    with_core_mut(|core| {
        let now = now_millis();
        let contact = ensure_contact(core, device_id, now)?;
        // One call and one transaction, ratchet included. Written as two, a
        // failure between them leaves exactly the durable paired-without-a-
        // ratchet row above — and the caller reports the pairing as failed
        // while the row stays.
        core.store
            .record_pairing(contact, l2_pub, name, colour, version, l1_pub, &seed, now)
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
        .map(|(_, _, e)| (e.seq, hex::encode(e.msg_id)))
        .collect())
}

/// How many of a thread's outgoing messages are still waiting, for the contract
/// tests.
///
/// `pub` with a note, as [`open_store_for_test`]: the difference between a
/// message held and a message sent is invisible from the public API — the DTO
/// carries no state — and it is the whole of what F3 promises about a closed
/// Discovery.
pub fn message_state_for_test(msg_id: &[u8]) -> Result<Option<String>, String> {
    with_core(|core| Ok(core.store.message_state(msg_id)?.map(|s| format!("{s:?}"))))
}

/// How many of a thread's outgoing messages are still waiting, for the contract
/// tests.
pub fn queued_on_thread_for_test(thread_id: i64) -> Result<usize, String> {
    with_core(|core| {
        core.store
            .count_in_state(thread_id, Direction::Outgoing, MessageState::Queued)
    })
}

/// Whether a thread has a ratchet, and how many bytes of state — never the
/// state itself.
///
/// `pub` with a note, as [`open_store_for_test`]. The length rather than the
/// bytes on purpose: a test only needs to know a ratchet was seeded, and a
/// helper that handed back key material would put it one `assert_eq!` away
/// from a CI log. Review caught exactly that on the roots in #79.
pub fn ratchet_size_for_test(thread_id: i64) -> Result<Option<usize>, String> {
    with_core(|core| Ok(core.store.ratchet_state(thread_id)?.map(|s| s.len())))
}

/// A hash of a thread's ratchet state, for the contract tests.
///
/// A fingerprint rather than the state: a test needs to know the ratchet
/// *moved*, and comparing hashes answers that without a helper that hands key
/// material to an assertion. Same reason [`ratchet_size_for_test`] returns a
/// length — and size alone cannot see a turn, since the state keeps its shape.
pub fn ratchet_fingerprint_for_test(thread_id: i64) -> Result<Option<[u8; 32]>, String> {
    with_core(|core| {
        Ok(core
            .store
            .ratchet_state(thread_id)?
            .map(|s| crate::crypto::hash::hash(&s)))
    })
}

/// This device's Layer-1 public key, for the contract tests.
///
/// Which of two paired devices takes the ratchet's initiator role is settled by
/// comparing these — see `net::speaks_first` — and the keys are generated fresh
/// per run, so a test that cannot compute the answer covers one of the two
/// arrangements at random and calls it coverage. With this, a test can pick a
/// peer identity that puts this device on the side it means to exercise.
///
/// `pub` with the same note as [`open_store_for_test`]: it is only reachable
/// from `tests/`, and it hands back a public key that every ceremony this
/// device runs already puts on the wire.
pub fn layer1_public_for_test() -> Result<[u8; 32], String> {
    Ok(require_net()?.layer1_public().0)
}

/// Whether a thread's ratchet has a sending chain, for the contract tests.
///
/// The one thing a fingerprint cannot say. Which of the two roles a ceremony
/// hands this device is decided by comparing Layer-1 keys, which are freshly
/// generated per run — so a test that only works when this end happens to be
/// the initiator passes half the time, and that is how
/// `writing_on_a_paired_thread_turns_the_ratchet` behaved before there was an
/// opening frame. Asking directly is what makes both roles reachable.
///
/// A boolean, so nothing about the chain itself leaves the engine.
pub fn ratchet_can_send_for_test(thread_id: i64) -> Result<Option<bool>, String> {
    with_core(|core| {
        core.store
            .ratchet_state(thread_id)?
            .map(|s| {
                crate::session::ratchet::Ratchet::from_state(&s)
                    .map(|r| r.can_send())
                    .map_err(|e| StoreError::Db(format!("ratchet: {e}")))
            })
            .transpose()
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
) -> Result<Vec<(Vec<u8>, i64, ChatEnvelope)>, StoreError> {
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
                thread,
                ChatEnvelope {
                    seq,
                    msg_id,
                    // Plaintext, as stored, and usually not what goes out:
                    // `resend_queued` sends the bytes this message was sealed
                    // as when it was written. This is the fallback it seals
                    // from when there were none — a thread whose chain had not
                    // opened yet — and that path still has to make the turn
                    // durable before anything leaves.
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
    for (msg_id, thread, envelope) in pending {
        // The bytes this message was sealed as when it was written, not a fresh
        // sealing of the same words.
        //
        // Re-sealing drew a new message key on every attempt, so a frame the
        // transport accepted and then lost left the receiver's chain one behind
        // for good — and `walk` will not close a gap past `MAX_SKIP`, so a long
        // enough run of unlucky reunions ended the conversation with no way
        // back. Byte-identical, a resend costs no key: a receiver that never
        // got it opens it exactly as it would have the first time, and one that
        // did sees a replay and refuses it without spending anything either.
        //
        // Absent only where there was nothing to seal with when the row was
        // written — a paired thread whose chain had not opened yet. Sealed here
        // instead, and kept, so the attempt after this one is identical to this
        // one.
        let wire = with_core_mut(|core| {
            if let Some(wire) = core.store.seal_for(&msg_id)? {
                return Ok(Some(wire));
            }
            let Outgoing::Ready { body, ratchet } =
                seal_for_thread(core, thread, &envelope.encode())?
            else {
                return Ok(None);
            };
            // Both before the bytes leave, for the reason `commit_sent` gives:
            // a counter that rewinds reuses a nonce. And both in one write, for
            // a reason of their own — see `seal_queued`. Crashing between the
            // save and the send costs nothing now: the seal is on disk, so the
            // next reunion sends these same bytes rather than drawing another
            // key.
            core.store.seal_queued(
                thread,
                ratchet.as_ref().map(|s| s.as_slice()),
                &msg_id,
                &body,
                now_millis(),
            )?;
            Ok(Some(body))
        })?;
        // Still nothing to seal with. Left `Queued`, which is where it was, and
        // picked up by the next reunion or by the opening that closes the gap.
        let Some(wire) = wire else { continue };
        // One at a time, and the state moves per message: a send that fails
        // half way leaves the ones that got away marked `Sent` and the rest
        // still `Queued`, so the next reunion picks up exactly where this
        // stopped rather than starting again.
        // Flattened, unlike the first send: this runs because a session just
        // opened, so there is no out-of-range case left to tell apart — a
        // refusal here is a refusal. Stopping is right either way, because
        // order matters and a peer who has dropped will not take the next one.
        net.send_chat(device_id, wire, std::time::Instant::now())
            .map_err(|why| why.to_string())?;
        move_state(thread, &msg_id, MessageState::Sent)?;
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
            // And the same idea one layer down: a paired thread whose other end
            // has never been able to write is a conversation that only goes one
            // way, and a reunion is the chance to fix it. Does nothing on every
            // thread that is already two-way — see `offer_an_opening`.
            if let Ok(Some(thread)) = thread_for_device(peer.clone()) {
                offer_an_opening(thread, &peer);
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
        net::NetEvent::ChainOpening { peer, body } => {
            // No `CoreEvent`. Nothing was said, so there is nothing for the UI
            // to draw and nobody to notify — the whole effect is a ratchet that
            // has moved.
            match take_an_opening(&peer, &body) {
                // The chain is open now, so whatever was written while it was
                // not can go. The same rule as a reunion, reached by a
                // different route: `write_then_send` holds a message it cannot
                // seal rather than losing it, and this is the event that makes
                // it sealable.
                Ok(()) => {
                    if let Err(why) = resend_queued(&peer) {
                        log::warn!("could not send what was held for {peer}: {why}");
                    }
                }
                Err(why) => log::warn!("could not take an opening from {peer}: {why}"),
            }
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
                        // Cloned so the screen goes first: the opening below
                        // writes to the store and then to the wire, and the
                        // person who has just held two phones together should
                        // not wait on either.
                        device_id: peer.clone(),
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
                    // Last, and after the row exists: the opening turns the
                    // ratchet that `record_pairing` just wrote down, so it has
                    // to be able to read it back. Only one of the two sides
                    // sends one — whichever the ceremony made the initiator.
                    offer_an_opening(thread_id, &peer);
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
        net::NetEvent::SessionRefused {
            peer,
            their_version,
            our_version,
        } => {
            // Reported as a Ping failure because reaching someone is what the
            // user did, and this is the answer to it. A dedicated event would
            // be a second way to say "that did not work" for the UI to route,
            // and the reason string is where the difference actually lives.
            emit(CoreEvent::PingFailed {
                device_id: peer,
                reason: if their_version < our_version {
                    "their app is an older version — they need to update".to_string()
                } else {
                    "their app is a newer version — you need to update".to_string()
                },
            });
        }
        net::NetEvent::ChatReceived { peer, body } => {
            if let Ok(landed) = store_incoming_chat(&peer, &body) {
                // The ack goes out before the screen is told, because the
                // ratchet turn behind it is already on disk and the sender's
                // row is waiting on it. Failing to send leaves that row saying
                // `Sent`, which is the honest direction to fail in.
                if let Some(ack) = landed.ack {
                    if let Err(why) = net.send_ack(&peer, ack, std::time::Instant::now()) {
                        log::warn!("could not acknowledge a message: {why}");
                    }
                }
                if let Some(event) = landed.event {
                    emit(event);
                }
            }
        }
        net::NetEvent::ChatAcked { peer, body } => {
            let _ = net;
            if let Err(why) = mark_delivered(&peer, &body) {
                log::warn!("could not mark a message delivered: {why}");
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
pub fn receive_chat_for_test(device_id: &str, body: &[u8]) -> Result<Option<CoreEvent>, String> {
    store_incoming_chat(device_id, body).map(|landed| landed.event)
}

/// Why an inbound chat body was turned away, for the contract tests.
///
/// `Some` only where a body failed to open — the one refusal that has a shape
/// worth naming. Everything else this function can return `None` for was
/// refused for a reason the store already records.
pub fn refusal_for_test(device_id: &str, body: &[u8]) -> Result<Option<String>, String> {
    store_incoming_chat(device_id, body).map(|l| l.refused.map(str::to_owned))
}

/// Take an acknowledgement as if it had arrived, for the contract tests.
///
/// The *receiving* half, which is where the forgery check lives and where it
/// was missing: an unsealed body must not promote anything. Nothing on the
/// public API can drive this — an ack arrives as a transport event and reports
/// itself only by changing a row.
pub fn mark_delivered_for_test(device_id: &str, body: &[u8]) -> Result<(), String> {
    mark_delivered(device_id, body)
}

/// Whether receiving this body would send an acknowledgement back, for the
/// contract tests.
///
/// The *sending* half, and the decision rather than the bytes. Whether an ack
/// goes out is the whole of what [`crate::session::frame::FrameKind::Ack`]
/// promises and refuses — an unpaired thread must not produce one, because an
/// unsealed ack is a forgeable claim that a message arrived — and none of it is
/// visible through the public API, which reports what was received and never
/// what was said back.
pub fn acked_on_receipt_for_test(device_id: &str, body: &[u8]) -> Result<bool, String> {
    store_incoming_chat(device_id, body).map(|landed| landed.ack.is_some())
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
/// What arrived, and what to say back about it.
struct Landed {
    event: Option<CoreEvent>,
    /// Why a body was turned away, when one was. Never a reason to *keep* it —
    /// see [`why_refused`].
    ///
    /// **Logged, and not yet shown to anybody.** Review was right that the
    /// design's "the log and the screen can say so" is only half true here: the
    /// receive path uses `event` and `ack`, and this reaches a log line and the
    /// contract tests. Putting it on a screen needs a `CoreEvent` and a
    /// decision about where a dropped message should appear, which is a
    /// separate slice rather than something to bolt on quietly.
    refused: Option<&'static str>,
    /// A sealed `msg_id`, when this thread can seal one. `None` on an unpaired
    /// thread — see [`crate::session::frame::FrameKind::Ack`] for why that is a
    /// stated limit rather than a gap.
    ack: Option<Vec<u8>>,
}

fn store_incoming_chat(device_id: &str, body: &[u8]) -> Result<Landed, String> {
    with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, device_id, now)?;
        // Where the thread had got to, restored from the store. A fresh
        // `Inbox` on every launch would forget everything already delivered,
        // so the first message after a restart would look new however many
        // times it had arrived before.
        let position = core.store.inbox_position(thread)?.unwrap_or_default();
        let mut inbox = Inbox::resumed(position.through, position.ahead);

        // Opened first, before anything else has an opinion about this message.
        // A body that will not open leaves the ratchet exactly as it was —
        // `Ratchet::decrypt` spends nothing before the tag is checked — so a
        // forged body cannot burn a key the real message needs.
        //
        // Ahead of *every* other check, and that ordering is load-bearing.
        // Opening is what steps this end's chain, and each of the reasons below
        // to refuse a message is a reason the sender knows nothing about: it
        // stepped its own chain to send it. Refuse before opening and this end
        // falls one behind, permanently, because `walk` will not close a gap
        // past `MAX_SKIP`. A peer only has to be declined 257 times — resends
        // it thinks were lost, or numbers this store cannot hold — and the next
        // genuinely new message cannot be opened at all.
        let Incoming {
            plaintext,
            ratchet: advanced,
        } = match open_for_thread(core, thread, body) {
            Ok(opened) => opened,
            Err(why) => {
                // Not fatal to the session. A body we cannot open is a body we
                // cannot show, and storing ciphertext as though somebody had
                // written it would be worse than dropping it. Nothing moved, so
                // there is nothing to keep.
                //
                // Refused either way. What changes here is only what gets
                // *said*: every unopenable body used to produce one line, so a
                // message written before a pairing, a corrupt frame and a
                // forgery were indistinguishable in a log and invisible on a
                // screen.
                let refusal = why_refused(body);
                log::warn!("dropping a chat message that would not open: {refusal} ({why})");
                return Ok(Landed {
                    event: None,
                    ack: None,
                    refused: Some(refusal),
                });
            }
        };
        let advanced = advanced.as_ref().map(|s| s.as_slice());

        // Only now is there an envelope to speak of. Everything below this line
        // is a judgement about a message that has already been opened, which is
        // what lets each of those judgements be made without costing the chain
        // step the sender took to send it.
        let envelope = match ChatEnvelope::decode(&plaintext) {
            Ok(envelope) => envelope,
            Err(why) => {
                log::warn!("dropping an unreadable chat message: {why}");
                // Opened, so the turn happened and is kept — the same rule as
                // every other refusal below.
                if let Some(advanced) = advanced {
                    core.store.start_ratchet(thread, advanced, now)?;
                }
                return Ok(Landed {
                    event: None,
                    ack: None,
                    refused: None,
                });
            }
        };

        // Everything that says this message is not kept, in one place, because
        // the answer to all of them is the same: drop the message, keep the
        // turn.
        //
        // The peer chooses `seq` and `decode` accepts any `u64`. Cast rather
        // than converted, anything above `i64::MAX` lands as a *negative* seq —
        // which sorts before every real message, and would let one frame
        // reorder a conversation permanently.
        let kept: Result<i64, &str> = {
            match i64::try_from(envelope.seq) {
                Err(_) => Err("numbered past what the store can hold"),
                Ok(seq) => match inbox.receive(envelope.seq) {
                    Delivery::Accepted => Ok(seq),
                    Delivery::Duplicate => Err("one we already have"),
                    // Refused rather than accepted with an enormous gap behind
                    // it. Tracking that gap is memory a peer could ask for by
                    // picking a number, and closing over it silently is the
                    // loss R0-F5 exists to prevent — so take neither.
                    Delivery::TooFarAhead => Err("too far ahead of the conversation"),
                },
            }
        };
        let seq = match kept {
            Ok(seq) => seq,
            Err(why) => {
                log::debug!("dropping a chat message: {why}");
                // The message is not kept and the turn is. It opened, which is
                // the sender proving it holds this chain — not a header anyone
                // can write — and the key it spent is spent whatever the rest
                // of this decides about the number on the outside.
                if let Some(advanced) = advanced {
                    core.store.start_ratchet(thread, advanced, now)?;
                }
                // Not acknowledged. The chain moved, but nothing was stored,
                // and an ack asserts that this end *has* the message.
                return Ok(Landed {
                    event: None,
                    ack: None,
                    refused: None,
                });
            }
        };

        let outcome = core.store.commit_received(
            advanced,
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
            // Acked all the same. A duplicate means the sender never heard the
            // first one, so staying quiet leaves its row claiming less than the
            // truth for ever — and this end already has the message, which is
            // the whole of what an ack asserts.
            //
            // No test reaches this and the mutation to `ack: None` survives. On
            // a paired thread a byte-identical resend is refused by the ratchet
            // before the store is consulted at all — that is what makes a
            // resend cost no message key — so arriving here needs a repeat this
            // end can still open, which means a position lost and recovered.
            // The harness can produce neither. Recorded as a guard on a state
            // nothing here can build rather than left to read as dead code.
            return Ok(Landed {
                event: None,
                ack: seal_ack(core, thread, &envelope.msg_id, now)?,
                refused: None,
            });
        }
        let ack = seal_ack(core, thread, &envelope.msg_id, now)?;
        Ok(Landed {
            ack,
            refused: None,
            event: Some(CoreEvent::MessageReceived {
                thread_id: thread,
                msg_id: hex::encode(envelope.msg_id),
                // From the opened envelope, which on a paired thread is not what
                // arrived on the wire. The event is what the screen draws the
                // moment a message lands — built from the raw frame, a thread
                // already open showed a ratchet header and a block of ciphertext,
                // and only reading it again put the real line there.
                text: String::from_utf8_lossy(&envelope.body).into_owned(),
            }),
        })
    })
}

/// Why a body would not open, in words a person could be shown.
///
/// **A guess about the shape of some bytes, and never a reason to keep them.**
/// The body has already been refused by the time this runs; nothing here can
/// change that, and nothing here is trusted. That is what makes it safe to look
/// at bytes that failed a forgery check — the answer goes to a log and a
/// screen, not to the store.
///
/// A forged body would parse here too. It does not matter: a stranger who
/// wanted to be told "that looked like a message from before we paired" is
/// welcome to the sentence, because they had to write the bytes to earn it and
/// it says nothing they did not already know.
fn why_refused(body: &[u8]) -> &'static str {
    if ChatEnvelope::decode(body).is_ok() {
        // A whole envelope, in the clear, on a thread that now opens only
        // sealed ones. Almost always a message written before the two of you
        // paired: it was sealed with what the thread had at the time, which was
        // nothing.
        //
        // Hedged, because it is a guess read off bytes nobody vouched for and
        // the sentence may reach a person. "It was written before you paired"
        // states as fact what this can only recognise the shape of — and the
        // one reader who could be misled by the difference is the one holding
        // a message that never arrived.
        return "it looked like a message from before you paired";
    }
    if body.len() < ratchet::HEADER_LEN {
        // Too short to be a ratchet header, so there was never a sealed message
        // here to open.
        return "it was too short to be a message";
    }
    // A ratchet header this end cannot follow: a chain it does not have, a key
    // already spent, or bytes somebody made up.
    "it could not be opened on this conversation"
}

/// The thread a peer's traffic belongs to, without creating one.
///
/// The read-only counterpart of `ensure_thread`. An acknowledgement is about a
/// conversation that already exists by definition, so a lookup that opened one
/// would be inventing a conversation out of somebody else's bytes.
fn thread_for_peer(core: &Core, device_id: &str) -> Result<Option<i64>, StoreError> {
    let Some(contact) = contact_id_for_device(core, device_id)? else {
        return Ok(None);
    };
    core.store.thread_for_contact(contact)
}

/// Seal an acknowledgement for a message just stored, and make the turn durable
/// before the caller sends it.
///
/// Sealing spends a message key on this end's *sending* chain, so an ack costs
/// the same as a short chat line — a conversation where one person only listens
/// still turns its own chain once per message received. That is the price of
/// the ack being unforgeable, and it is paid in the one currency the ratchet
/// cannot refill, so it is worth saying out loud.
///
/// The advanced state is written here, before anything goes out, for the reason
/// `commit_sent` gives: a counter that rewinds reuses a nonce. Seal, commit,
/// send — in that order or not at all.
///
/// `None` on an unpaired thread. There is nothing to seal with, and an unsealed
/// ack would be a forgeable claim that a message arrived.
fn seal_ack(
    core: &mut Core,
    thread: i64,
    msg_id: &[u8; MSG_ID_LEN],
    now: i64,
) -> Result<Option<Vec<u8>>, StoreError> {
    match seal_for_thread(core, thread, msg_id)? {
        Outgoing::Ready {
            body,
            ratchet: Some(advanced),
        } => {
            core.store.start_ratchet(thread, &advanced, now)?;
            Ok(Some(body))
        }
        // Unpaired: nothing to seal with, and nothing to say.
        Outgoing::Ready { ratchet: None, .. } => Ok(None),
        // Paired, but this end cannot write yet — the chain opens when the
        // other side speaks first, which it just did, so this is a moment that
        // does not outlast the opening.
        Outgoing::NoChainYet => Ok(None),
    }
}

/// Mark the message an acknowledgement names as delivered.
///
/// Opening it advances this end's receiving chain, exactly as a chat line
/// would, and the advanced state has to land whether or not the row updates —
/// the chain moved either way.
fn mark_delivered(device_id: &str, body: &[u8]) -> Result<(), String> {
    let promoted = with_core_mut(|core| {
        let now = now_millis();
        let Some(thread) = thread_for_peer(core, device_id)? else {
            return Ok(None);
        };
        // Only a sealed one counts, and this is where that is enforced.
        //
        // `open_for_thread` hands back the raw bytes when a thread has no
        // ratchet, because an unpaired chat body legitimately arrives in the
        // clear. An acknowledgement never does: its whole value is that
        // producing one requires a chain only a completed ceremony discloses.
        // Without this guard, sixteen bytes from anybody in range promoted a
        // message to `Delivered` on an unpaired thread — the exact lie this
        // design refuses to permit, which was stated for the sending half and
        // never checked on the receiving one.
        let Incoming {
            plaintext,
            ratchet: advanced,
        } = match open_sealed_for_thread(core, thread, body) {
            Ok(opened) => opened,
            Err(why) => {
                log::warn!("dropping an acknowledgement that would not open: {why}");
                return Ok(None);
            }
        };
        if let Some(advanced) = advanced.as_ref() {
            core.store.start_ratchet(thread, advanced, now)?;
        }
        let Ok(msg_id) = <[u8; MSG_ID_LEN]>::try_from(plaintext.as_slice()) else {
            log::warn!("an acknowledgement did not name a message");
            return Ok(None);
        };
        // Only ever forwards. A row already `Delivered` stays there, and one
        // still `Queued` is not promoted by an ack for bytes that have not
        // left — which cannot happen, but the store is where that stays true.
        if core.store.message_state(&msg_id)? == Some(MessageState::Sent) {
            return Ok(Some((thread, msg_id)));
        }
        Ok(None)
    })?;
    // Outside the lock, for the reason `move_state` gives.
    let Some((thread, msg_id)) = promoted else {
        return Ok(());
    };
    move_state(thread, &msg_id, MessageState::Delivered)
}

/// Move a message's state and tell the screen.
///
/// One function because it is one fact. Written as a store call plus an
/// `emit` at each site, the two drifted immediately: every send updated the row
/// and none of them announced it, so a conversation showed "not confirmed" over
/// a message the store already had delivered.
///
/// The emit is outside the core lock deliberately. `emit` hands an event to
/// Dart, and Dart's handler calls back into the engine to re-read the thread —
/// so announcing while holding the lock invites the deadlock that the pump
/// thread's own rule already avoids.
fn move_state(thread: i64, msg_id: &[u8], state: MessageState) -> Result<(), String> {
    let changed = with_core(|core| core.store.set_message_state(msg_id, state))?;
    if changed {
        emit(CoreEvent::MessageStateChanged {
            thread_id: thread,
            msg_id: hex::encode(msg_id),
            state: state_dto(state),
        });
    }
    Ok(())
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
    // Whose conversation this is, asked once.
    //
    // This used to compare each sighting's contact against `thread_for_contact`
    // — the contact's *newest* thread — which since T12 is one of several a
    // person may have. The two spellings agree today, because `superseded`
    // refuses a write to any older thread and `nearby_devices` only ever offers
    // the newest, so no caller can reach the difference and the mutation
    // between them survives. Kept as the owner question anyway: it is one store
    // read instead of one per sighting, and it does not quietly depend on an
    // invariant enforced two modules away.
    let Some(owner) = core.store.contact_for_thread(thread)? else {
        return Ok(None);
    };
    let known = Recogniser::read(core)?;
    for s in net.discovery().sightings() {
        if contact_for_sighting(core, &s, &known)? == Some(owner) {
            return Ok(Some(s.peer));
        }
    }
    Ok(None)
}

/// What a thread's ratchet had to say about a body on its way out.
enum Outgoing {
    /// The bytes for the wire, and the ratchet state that has to be stored with
    /// the message that advanced it. `ratchet` is `None` on an unpaired thread,
    /// which has none.
    Ready {
        body: Vec<u8>,
        ratchet: Option<Zeroizing<Vec<u8>>>,
    },
    /// Paired, but this end has never been written to, so it has no sending
    /// chain to seal with. Nothing can go out until the other side opens it —
    /// see [`offer_an_opening`], which is what does.
    NoChainYet,
}

/// Seal a body with the thread's ratchet, if it has one.
///
/// An unpaired thread has no ratchet and its body travels as it always did,
/// inside the session's own encryption — R0-F5 lets strangers chat, and the
/// Double Ratchet belongs to a paired thread (tech spec §5).
///
/// The header travels in clear ahead of the body because the receiver needs it
/// to find the key; it is the AEAD's associated data, so the two cannot be
/// separated without the tag failing.
///
/// # Why a responder with no chain is not an error
///
/// It is a window measured in the milliseconds between two devices finishing
/// the same ceremony, and it is a window somebody can type into. Reported as an
/// error, the message they wrote is gone — `write_then_send` seals before it
/// writes the row, so there would be no row — and what they would have to go on
/// is "message could not be decrypted", which is both frightening and about
/// something else. Held instead, which is what R0-F5 already does for every
/// other reason a message cannot leave right now.
fn seal_for_thread(core: &Core, thread: i64, plaintext: &[u8]) -> Result<Outgoing, StoreError> {
    let Some(state) = core.store.ratchet_state(thread)? else {
        return Ok(Outgoing::Ready {
            body: plaintext.to_vec(),
            ratchet: None,
        });
    };
    let mut ratchet =
        Ratchet::from_state(&state).map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    if !ratchet.can_send() {
        return Ok(Outgoing::NoChainYet);
    }
    let (header, sealed) = ratchet
        .encrypt(plaintext)
        .map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    let mut body = header.to_bytes().to_vec();
    body.extend_from_slice(&sealed);
    Ok(Outgoing::Ready {
        body,
        ratchet: Some(ratchet.to_state()),
    })
}

/// Speak first on a paired thread, so the other end can speak at all.
///
/// A `Ratchet::responder` has no sending chain until it has received
/// something, so the side the ceremony names as responder pairs and then finds
/// it cannot write. The side that *can* write settles it by sending one
/// [`FrameKind::Opening`](crate::session::frame::FrameKind::Opening): a ratchet
/// header and a sealed empty body, which turns their ratchet and gives them
/// both chains.
///
/// The two other ways out were considered and are in the T12 notes. Holding a
/// responder's messages `Queued` until the initiator writes makes "you paired,
/// you typed, nothing left" a normal state with nothing on screen to explain
/// it. Seeding both sides so either can start departs from the Signal
/// handshake this ratchet follows, in the one file where following it is what
/// stands in for the review nobody has done.
///
/// Best effort by design, and it repairs itself. This runs at pairing and
/// again whenever a session opens, for as long as the thread's ratchet says
/// one is owed ([`Ratchet::owes_an_opening`]). An opening lost to a pipe that
/// closed at the wrong moment would otherwise leave that thread one-way for
/// good, and the only way back would be pairing again — two people and a code.
///
/// The reunion call is the part with no test on it, the same gap and the same
/// reason as `resend_queued` in the arm beside it: a `SessionOpened` is a
/// transport event and nothing in the public API produces one. What it decides
/// is covered — `only_the_side_that_can_write_first_owes_an_opening` — so what
/// is untested is the wiring rather than the rule.
fn offer_an_opening(thread: i64, device_id: &str) {
    let body = match with_core_mut(|core| opening_for_thread(core, thread)) {
        Ok(Some(body)) => body,
        // Nothing owed: an unpaired thread, a responder with nothing to say
        // yet, or a peer who has already spoken.
        Ok(None) => return,
        Err(why) => {
            log::warn!("could not open the chain on thread {thread}: {why}");
            return;
        }
    };
    let Ok(net) = require_net() else { return };
    match net.send_opening(device_id, body, std::time::Instant::now()) {
        // Not "sent to <device>": this is the one line that would say a paired
        // thread and a rotating id belong together, and it is written on the
        // device that already knows.
        Ok(()) => log::info!("opened the chain on thread {thread}"),
        // The message key is spent either way — it was persisted before the
        // bytes left, for the reason `commit_sent` gives — and the next session
        // tries again.
        Err(why) => log::info!("could not open the chain on thread {thread} yet: {why}"),
    }
}

/// The bytes of an opening, if this thread owes one, with the turn made durable.
///
/// Persisted before it is returned and therefore before it is sent, which is
/// the same rule as [`Store::commit_sent`](crate::store::Store::commit_sent)
/// and for the same reason: `nonce_for` derives from the message number, so a
/// counter that rewound would reuse a key *and* a nonce. Crashing between the
/// write and the send costs one message key, which the receiver's ratchet steps
/// over without noticing.
fn opening_for_thread(core: &Core, thread: i64) -> Result<Option<Vec<u8>>, StoreError> {
    let Some(state) = core.store.ratchet_state(thread)? else {
        return Ok(None);
    };
    let mut ratchet =
        Ratchet::from_state(&state).map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    if !ratchet.owes_an_opening() {
        return Ok(None);
    }
    let (header, sealed) = ratchet
        .encrypt(&[])
        .map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    core.store
        .start_ratchet(thread, &ratchet.to_state(), now_millis())?;
    let mut body = header.to_bytes().to_vec();
    body.extend_from_slice(&sealed);
    Ok(Some(body))
}

/// Take the opening a peer sent: turn the ratchet, write down the turn, keep
/// nothing else.
///
/// The receiving half of [`offer_an_opening`], and the half that has to be
/// trusted not to store anything. It is written so that trust is structural
/// rather than remembered — there is no `seq`, no `msg_id` and no
/// [`NewMessage`] anywhere below, so there is nothing this could write a row
/// with even if a later edit wanted to. `an_opening_leaves_no_message_behind`
/// is what says so out loud.
fn take_an_opening(device_id: &str, body: &[u8]) -> Result<(), String> {
    with_core_mut(|core| {
        let Some(contact) = contact_id_for_device(core, device_id)? else {
            return Ok(());
        };
        let Some(thread) = core.store.thread_for_contact(contact)? else {
            return Ok(());
        };
        // Opened through the same door as a chat body, so the two cannot drift
        // apart about what a sealed body looks like.
        let Incoming {
            plaintext,
            ratchet: advanced,
        } = open_for_thread(core, thread, body)?;
        let Some(advanced) = advanced else {
            // No ratchet on this thread, so `open_for_thread` handed the bytes
            // straight back. Nothing turned and nothing to write.
            log::warn!("an opening arrived for a thread with no ratchet");
            return Ok(());
        };
        if !plaintext.is_empty() {
            // Dropped, not shown and not stored — it has nowhere to go from
            // here. Logged because a peer putting something in a frame defined
            // to carry nothing is worth being able to see.
            log::warn!("an opening arrived carrying {} bytes", plaintext.len());
        }
        core.store.start_ratchet(thread, &advanced, now_millis())?;
        Ok(())
    })
}

/// What a thread's ratchet made of a body that arrived.
///
/// A struct rather than the pair it used to be, because the pair was two
/// options deep and read as neither of the two things it meant.
struct Incoming {
    plaintext: Vec<u8>,
    /// The state to store with the message that advanced it, or `None` on an
    /// unpaired thread, which has no ratchet to advance.
    ratchet: Option<Zeroizing<Vec<u8>>>,
}

/// Open a body with the thread's ratchet, if it has one.
///
/// The mirror of [`seal_for_thread`], and it decides the same way: a thread
/// with a ratchet expects sealed bodies, one without expects plain. Both sides
/// gain their ratchet in the same act — the ceremony — so the two ends agree
/// without anything on the wire saying which it is.
fn open_for_thread(core: &Core, thread: i64, body: &[u8]) -> Result<Incoming, StoreError> {
    let Some(state) = core.store.ratchet_state(thread)? else {
        return Ok(Incoming {
            plaintext: body.to_vec(),
            ratchet: None,
        });
    };
    let mut ratchet =
        Ratchet::from_state(&state).map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    let header_bytes: [u8; ratchet::HEADER_LEN] = body
        .get(..ratchet::HEADER_LEN)
        .and_then(|h| h.try_into().ok())
        .ok_or_else(|| StoreError::Db("a sealed message with no header".into()))?;
    let plaintext = ratchet
        .decrypt(
            ratchet::Header::from_bytes(&header_bytes),
            &body[ratchet::HEADER_LEN..],
        )
        .map_err(|e| StoreError::Db(format!("ratchet: {e}")))?;
    Ok(Incoming {
        plaintext: plaintext.to_vec(),
        ratchet: Some(ratchet.to_state()),
    })
}

/// Open something that is only ever sealed.
///
/// [`open_for_thread`] falls back to treating a body as plaintext when the
/// thread has no ratchet, which is right for a chat line — R0-F5 lets strangers
/// talk, and a stranger's thread has nothing to seal with. It is wrong for
/// anything whose meaning depends on having been sealed, because the fallback
/// turns "nobody could have forged this" into "anybody could".
///
/// Refusing here rather than at each caller: a guard that has to be remembered
/// is one that will be forgotten, and the thing forgotten is a forgery check.
fn open_sealed_for_thread(core: &Core, thread: i64, body: &[u8]) -> Result<Incoming, StoreError> {
    if core.store.ratchet_state(thread)?.is_none() {
        return Err(StoreError::Db(
            "a thread with no ratchet cannot have sealed anything".into(),
        ));
    }
    open_for_thread(core, thread, body)
}

/// Which contact a device belongs to, if we have one on file.
///
/// **Four routes, strongest claim first**, and every caller uses all four.
/// Splitting them was the bug this order exists to prevent: the nearby list
/// recognised a friend by her advert hint while every *send* went through a
/// lookup that did not know about hints, fell through to the device id, and
/// minted a stranger for somebody the screen had just named.
///
/// 1. **A pseudonym the peer has proved** in a session. Nothing outranks it.
/// 2. **The advert hint** (T09a). Cryptographic, and the only route that works
///    with nobody connected — forging one means guessing eight bytes keyed on a
///    Layer-1 key you would have had to attend a ceremony to learn.
/// 3. **A claimed persona.** Only claimed, so it sits below the two above, but
///    it is a Layer-2 key and matches paired contacts only.
///
///    **No test reaches this route, and deleting it passes the suite.** It
///    fires only for a device whose persona we have fetched while holding no
///    session and matching no hint — a pipe that opened far enough to answer
///    the persona endpoint and then went away, against a peer advertising
///    nothing we can read. The harness can build the parts and not the
///    sequence. Recorded so the surviving mutant reads as what it is rather
///    than as a hole to close by deleting the route: it predates this
///    ordering, and dropping it would silently narrow who can be recognised.
/// 4. **The rotating device id.** Last, and it matters that it is last. It
///    rotates every twelve minutes and anybody may present one, so a row
///    remembered under it must not outrank a friend recognised by a key. Before
///    this it came *second*, which is how a stray row minted under a friend's
///    old id went on displacing her for as long as that id lived.
///
/// Creates nothing: a stranger stays a stranger, and drawing the nearby list is
/// not the moment to start writing rows.
fn contact_for(
    core: &Core,
    device_id: &str,
    hint: Option<&[u8; hint::HINT_LEN]>,
    persona: Option<&VerifiedPersona>,
    known: &Recogniser,
) -> Result<Option<i64>, StoreError> {
    if let Some(id) = proved_contact(core, device_id)? {
        return Ok(Some(id));
    }
    if let Some(id) = known.whose(device_id, hint) {
        return Ok(Some(id));
    }
    if let Some(persona) = persona {
        if let Some(c) = core.store.paired_contact_by_l2(&persona.l2_pub.0)? {
            return Ok(Some(c.id));
        }
    }
    contact_by_device_id(core, device_id)
}

/// [`contact_for`] with what a sighting already carries.
///
/// `known` is read once for a whole list and handed down — see [`Recogniser`].
fn contact_for_sighting(
    core: &Core,
    sighting: &Sighting,
    known: &Recogniser,
) -> Result<Option<i64>, StoreError> {
    contact_for(
        core,
        &sighting.peer,
        sighting.hint.as_ref(),
        sighting.persona.as_ref(),
        known,
    )
}

/// [`contact_for`] with what the current sighting of this device carries.
///
/// The read-only twin of [`ensure_contact`], for callers holding only a device
/// id — every send and every receive. It reaches for the sighting rather than
/// taking a narrower set of routes, because the two answering differently is
/// exactly what put a friend's name on one screen and a stranger's row behind
/// every message to her.
fn contact_id_for_device(core: &Core, device_id: &str) -> Result<Option<i64>, StoreError> {
    let seen = sighting_of(core, device_id);
    let known = Recogniser::read(core)?;
    contact_for(
        core,
        device_id,
        seen.as_ref().and_then(|s| s.hint.as_ref()),
        seen.as_ref().and_then(|s| s.persona.as_ref()),
        &known,
    )
}

/// A contact keyed on an identity the peer has actually proved.
fn proved_contact(core: &Core, device_id: &str) -> Result<Option<i64>, StoreError> {
    let Some(real) = session_key_of(core, device_id) else {
        return Ok(None);
    };
    Ok(core.store.contact_by_pseudonym(&real)?.map(|c| c.id))
}

/// A contact remembered under this rotating id, and nothing stronger.
fn contact_by_device_id(core: &Core, device_id: &str) -> Result<Option<i64>, StoreError> {
    Ok(core
        .store
        .contact_by_pseudonym(&fake::placeholder_pseudonym(device_id))?
        .map(|c| c.id))
}

/// What we can currently see of this device, if anything.
fn sighting_of(core: &Core, device_id: &str) -> Option<Sighting> {
    core.net
        .as_ref()?
        .discovery()
        .sightings()
        .into_iter()
        .find(|s| s.peer == device_id)
}

/// The Layer-1 keys an advert hint can be tried against, and the moment the
/// whole list is being drawn at.
///
/// Built once per list and reused for every row, for two reasons that pull the
/// same way. A hint is not a lookup key — every pairing has to be tried, since
/// only somebody holding the key can recognise what it generated, which is the
/// property that makes the hint safe to broadcast at all — so doing it per
/// sighting reads the whole table once per device in range. And the epoch is a
/// division of *now*: a list built across a twelve-minute boundary would judge
/// its first rows against one epoch and its last against another, so a friend
/// could be recognised in one row of a redraw and not the next.
///
/// One read, one clock, one answer for the whole screen.
struct Recogniser {
    pairings: Vec<Pairing>,
    now_ms: i64,
}

impl Recogniser {
    fn read(core: &Core) -> Result<Self, StoreError> {
        Ok(Self {
            pairings: core.store.pairings()?,
            now_ms: now_millis(),
        })
    }

    /// The paired contact whose Layer-1 key generated this hint, if one did.
    fn whose(&self, device_id: &str, hint: Option<&[u8; hint::HINT_LEN]>) -> Option<i64> {
        let hint = hint?;
        self.pairings
            .iter()
            .find(|p| hint::written_by(&p.l1_pub, device_id, hint, self.now_ms))
            .map(|p| p.contact_id)
    }
}

/// The remote static a live session's handshake proved, if there is one.
///
/// Called `pseudonym_of` until T18b, and the rename is the point. In Noise IK
/// the remote static is whatever the other side presented: their pseudonym
/// toward us when they dialled, and the `session_pub` from their persona record
/// when we dialled. Which role we take is a comparison of two rotating ids, so
/// for one person it changes every twelve minutes.
///
/// Contacts are keyed on this, and stay resolvable anyway — a row keyed under
/// one of the two values is still found by `contact_for`'s persona route. What
/// it *does* mean is that `contacts.pseudonym` may hold either kind, so nothing
/// deciding about a person may treat this as the one true handle. The block
/// list asks with all of them for exactly this reason.
fn session_key_of(core: &Core, device_id: &str) -> Option<[u8; 32]> {
    core.net
        .as_ref()
        .and_then(|net| net.remote_static(device_id))
        .map(|p| p.0)
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
    let Some(real) = session_key_of(core, device_id) else {
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
        session_key_of(core, device_id).unwrap_or_else(|| fake::placeholder_pseudonym(device_id));
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

fn state_dto(state: MessageState) -> MessageStateDto {
    match state {
        MessageState::Queued => MessageStateDto::Queued,
        MessageState::Sent => MessageStateDto::Sent,
        MessageState::Delivered => MessageStateDto::Delivered,
    }
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

/// The palette offered on first launch. See `DEFAULT_COLOURS`.
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
