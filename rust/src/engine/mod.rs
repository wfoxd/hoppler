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

pub mod fake;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::types::{ChatMessageDto, CoreEvent, NearbyDevice, PersonaDto};
use crate::crypto::rng;
use crate::frb_generated::StreamSink;
use crate::identity::keystore::SoftwareKeystore;
use crate::identity::{Identity, Persona};
use crate::store::{
    Direction, MessageState, NewContact, NewMessage, NewTransfer, Store, StoreError, TransferState,
};

struct Core {
    store: Store,
    identity: Identity,
    discovery_on: bool,
}

static CORE: Mutex<Option<Core>> = Mutex::new(None);
static EVENT_SINK: Mutex<Option<StreamSink<CoreEvent>>> = Mutex::new(None);

// ── event bus ───────────────────────────────────────────────────────────────

/// Register the Dart event sink. Replaces any previous one.
pub fn set_event_sink(sink: StreamSink<CoreEvent>) {
    *EVENT_SINK.lock().expect("event sink lock") = Some(sink);
}

/// Emit an event to Dart, if a sink is registered.
pub fn emit(event: CoreEvent) {
    if let Some(sink) = EVENT_SINK.lock().expect("event sink lock").as_ref() {
        let _ = sink.add(event);
    }
}

// ── lifecycle ─────────────────────────────────────────────────────────────────

/// Initialise the core at `support_dir`, returning the local persona.
pub fn init(support_dir: String) -> Result<PersonaDto, String> {
    let dir = PathBuf::from(support_dir);
    let db = dir.join("hoppler.db");
    let files = dir.join("files");
    let keystore = Arc::new(SoftwareKeystore::new());

    // The dev keystore is fresh each launch, so a DB from a prior session can't
    // be reopened — start clean if the old master is gone.
    let store = match Store::open(keystore.clone(), &db, &files) {
        Ok(s) => s,
        Err(_) => {
            let _ = std::fs::remove_file(&db);
            Store::open(keystore, &db, &files).map_err(stringify)?
        }
    };

    let identity = Identity::generate("Me", 0x0044_88ff);
    let dto = persona_dto(identity.persona());
    *CORE.lock().map_err(|_| "core lock".to_string())? = Some(Core {
        store,
        identity,
        discovery_on: false,
    });
    Ok(dto)
}

// ── identity ──────────────────────────────────────────────────────────────────

pub fn current_persona() -> Result<PersonaDto, String> {
    with_core(|core| Ok(persona_dto(core.identity.persona())))
}

pub fn update_persona(name: String, colour: u32) -> Result<PersonaDto, String> {
    with_core_mut(|core| {
        core.identity.update_persona(name, colour);
        let persona = core.identity.persona().clone();
        core.store
            .settings_set("persona.name", persona.name.as_bytes())?;
        Ok(persona_dto(&persona))
    })
}

// ── discovery ─────────────────────────────────────────────────────────────────

pub fn set_discovery(enabled: bool) -> Result<(), String> {
    with_core_mut(|core| {
        core.discovery_on = enabled;
        Ok(())
    })?;
    let devices = if enabled { fake::nearby() } else { Vec::new() };
    emit(CoreEvent::DiscoveryUpdated { devices });
    Ok(())
}

pub fn nearby_devices() -> Result<Vec<NearbyDevice>, String> {
    with_core(|core| {
        Ok(if core.discovery_on {
            fake::nearby()
        } else {
            Vec::new()
        })
    })
}

// ── messaging ─────────────────────────────────────────────────────────────────

pub fn ping(device_id: String) -> Result<(), String> {
    let name = fake::peer(&device_id)
        .map(|p| p.name.to_owned())
        .unwrap_or_else(|| "Unknown".into());
    emit(CoreEvent::Pinged { device_id, name });
    Ok(())
}

/// Send a chat message: writes the outgoing row, then (fake) synchronously
/// receives a canned reply and emits `MessageReceived`. Returns the outgoing
/// message as stored.
pub fn send_chat(device_id: String, text: String) -> Result<ChatMessageDto, String> {
    with_core_mut(|core| {
        let now = now_millis();
        let thread = ensure_thread(core, &device_id, now)?;

        let (out_bytes, out_hex) = new_msg_id();
        let seq = core.store.next_seq(thread, Direction::Outgoing)?;
        core.store.add_message(&NewMessage {
            thread_id: thread,
            seq,
            msg_id: out_bytes,
            body: text.clone().into_bytes(),
            direction: Direction::Outgoing,
            state: MessageState::Sent,
            created_at: now,
        })?;
        let dto = ChatMessageDto {
            msg_id: out_hex,
            thread_id: thread,
            text: text.clone(),
            outgoing: true,
            created_at: now,
        };

        // Fake peer replies.
        let reply = fake::canned_reply(&text);
        let (rep_bytes, rep_hex) = new_msg_id();
        let rseq = core.store.next_seq(thread, Direction::Incoming)?;
        core.store.add_message(&NewMessage {
            thread_id: thread,
            seq: rseq,
            msg_id: rep_bytes,
            body: reply.clone().into_bytes(),
            direction: Direction::Incoming,
            state: MessageState::Delivered,
            created_at: now + 1,
        })?;
        emit(CoreEvent::MessageReceived {
            thread_id: thread,
            msg_id: rep_hex,
            text: reply,
        });
        Ok(dto)
    })
}

pub fn thread_messages(thread_id: i64) -> Result<Vec<ChatMessageDto>, String> {
    with_core(|core| {
        let msgs = core.store.messages_for_thread(thread_id)?;
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

// ── transfers ─────────────────────────────────────────────────────────────────

/// Offer a Drop: records a transfer row and (fake) emits progress then
/// completion synchronously. Returns the transfer id.
pub fn offer_drop(_device_id: String, name: String, size: u64) -> Result<String, String> {
    let transfer_id = with_core_mut(|core| {
        let id = core.store.add_transfer(&NewTransfer {
            thread_id: None,
            direction: Direction::Outgoing,
            name,
            size: size as i64,
            mime: "application/octet-stream".into(),
            state: TransferState::Complete,
            root_hash: [0u8; 32],
            chunk_bitmap: Vec::new(),
            created_at: now_millis(),
        })?;
        Ok(format!("xfer-{id}"))
    })?;

    let total = size.max(1);
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

/// Find or create the contact + thread for a (fake) device.
fn ensure_thread(core: &Core, device_id: &str, now: i64) -> Result<i64, StoreError> {
    let l1 = fake::fake_l1_pub(device_id);
    let contact_id = match core.store.contact_by_l1(&l1)? {
        Some(c) => c.id,
        None => {
            let (name, colour) = fake::peer(device_id)
                .map(|p| (p.name.to_owned(), p.colour))
                .unwrap_or_else(|| ("Unknown".into(), 0));
            core.store.add_contact(&NewContact {
                l1_pub: l1,
                l2_pub: [0u8; 32],
                name,
                colour,
                persona_version: 1,
                paired_at: now,
            })?
        }
    };
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
