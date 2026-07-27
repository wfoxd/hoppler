//! An in-process transport: a shared "airspace" several nodes join, discover
//! each other in, and exchange bytes over — with no sockets and no radios.
//!
//! This is not a mock of the trait; it is a real implementation of it, which is
//! what makes it useful: the session layer (T10) can drive Noise handshakes
//! over it in a unit test and get the same semantics a radio gives — ordered,
//! reliable, *unframed* bytes delivered on a transport thread.
//!
//! Two details exist specifically to stop callers developing habits a radio
//! will punish:
//! - Events are delivered on a per-node thread, never on the caller's (contract
//!   rule 1), so code written here doesn't assume synchronous callbacks.
//! - [`LoopbackNet::with_max_chunk`] splits sends, because a rung is free to
//!   fragment and BLE's MTU will. Code that assumes one `send` yields one
//!   `Received` breaks on a real radio; here it breaks in a unit test instead.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use super::{EventSink, PeerId, Transport, TransportError, TransportEvent, TransportLimits};

/// Loopback carries anything; the cap only exists so the value is finite.
const MAX_ADVERTISING_PAYLOAD: usize = 64 * 1024;

struct Node {
    /// Delivery queue for this node's sink, drained by its own thread.
    tx: Option<Sender<TransportEvent>>,
    advertising: Option<Vec<u8>>,
    scanning: bool,
    pipes: Vec<PeerId>,
}

struct NetState {
    nodes: HashMap<PeerId, Node>,
    max_chunk: Option<usize>,
}

/// A shared airspace. Clone it to hand the same airspace to several nodes.
#[derive(Clone)]
pub struct LoopbackNet {
    state: Arc<Mutex<NetState>>,
}

impl Default for LoopbackNet {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopbackNet {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(NetState {
                nodes: HashMap::new(),
                max_chunk: None,
            })),
        }
    }

    /// An airspace that splits every send into chunks of at most `n` bytes —
    /// the deliberate analogue of an MTU, so callers cannot rely on message
    /// boundaries the contract doesn't promise.
    pub fn with_max_chunk(n: usize) -> Self {
        let net = Self::new();
        net.lock().max_chunk = Some(n.max(1));
        net
    }

    /// Join the airspace as `id`, delivering events to `sink`.
    pub fn join(&self, id: impl Into<PeerId>, sink: EventSink) -> LoopbackTransport {
        let id = id.into();
        let (tx, rx) = channel::<TransportEvent>();
        // The sink is revocable: dropping the sender alone is not enough,
        // because a Receiver keeps draining whatever was already queued — so
        // events could still land after shutdown returned (contract rule 6).
        let sink = Arc::new(RwLock::new(Some(sink)));
        let delivery_sink = Arc::clone(&sink);
        // Deliver on our own thread: no Transport method may call the sink
        // before it returns (contract rule 1).
        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                if let Some(s) = delivery_sink
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    s(event);
                }
            }
        });
        self.lock().nodes.insert(
            id.clone(),
            Node {
                tx: Some(tx),
                advertising: None,
                scanning: false,
                pipes: Vec::new(),
            },
        );
        LoopbackTransport {
            id: Mutex::new(id),
            net: self.clone(),
            sink,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl NetState {
    /// Queue an event for `peer`. Queuing under the lock is safe — the sink
    /// runs on the node's own thread, not here.
    fn post(&self, peer: &str, event: TransportEvent) {
        if let Some(tx) = self.nodes.get(peer).and_then(|n| n.tx.as_ref()) {
            let _ = tx.send(event);
        }
    }

    fn scanners_other_than(&self, me: &str) -> Vec<PeerId> {
        self.nodes
            .iter()
            .filter(|(id, n)| n.scanning && id.as_str() != me)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// One node's view of the airspace.
pub struct LoopbackTransport {
    /// Mutable so the node can rotate how it appears (tech spec §4).
    id: Mutex<PeerId>,
    net: LoopbackNet,
    /// Cleared under the write lock by `shutdown`, so an event already queued
    /// cannot still be delivered afterwards.
    sink: Arc<RwLock<Option<EventSink>>>,
}

impl LoopbackTransport {
    pub fn id(&self) -> PeerId {
        self.id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Transport for LoopbackTransport {
    fn name(&self) -> &'static str {
        "loopback"
    }

    fn limits(&self) -> TransportLimits {
        TransportLimits {
            max_advertising_payload: MAX_ADVERTISING_PAYLOAD,
            preferred_write_size: self.net.lock().max_chunk.unwrap_or(16 * 1024),
        }
    }

    fn set_local_id(&self, new_id: &str) -> Result<(), TransportError> {
        let mut id = self.id.lock().unwrap_or_else(|e| e.into_inner());
        if *id == new_id {
            return Ok(());
        }
        let mut state = self.net.lock();
        // Renaming while connected would leave peers holding pipes under an id
        // that no longer exists, and would rename a peer under an open pipe
        // (contract rule 4). The core rotates when idle.
        if state.nodes.get(&*id).is_some_and(|n| !n.pipes.is_empty()) {
            return Err(TransportError::Unavailable(
                "cannot rotate the local id while pipes are open".into(),
            ));
        }
        let Some(node) = state.nodes.remove(&*id) else {
            return Err(TransportError::Unavailable("node has shut down".into()));
        };
        // Scanners see the old name go and the new one arrive — which is the
        // point: an observer must not be able to link the two.
        let was_advertising = node.advertising.clone();
        for w in state.scanners_other_than(&id) {
            state.post(&w, TransportEvent::PeerLost { peer: id.clone() });
        }
        state.nodes.insert(new_id.to_string(), node);
        if let Some(payload) = was_advertising {
            for w in state.scanners_other_than(new_id) {
                state.post(
                    &w,
                    TransportEvent::PeerFound {
                        peer: new_id.to_string(),
                        payload: payload.clone(),
                    },
                );
            }
        }
        *id = new_id.to_string();
        Ok(())
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        if payload.len() > MAX_ADVERTISING_PAYLOAD {
            return Err(TransportError::PayloadTooLarge {
                max: MAX_ADVERTISING_PAYLOAD,
            });
        }
        let me = self.id();
        let mut state = self.net.lock();
        let Some(node) = state.nodes.get_mut(&me) else {
            return Err(TransportError::Unavailable("node has shut down".into()));
        };
        node.advertising = Some(payload.clone());
        for w in state.scanners_other_than(&me) {
            state.post(
                &w,
                TransportEvent::PeerFound {
                    peer: me.clone(),
                    payload: payload.clone(),
                },
            );
        }
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        let me = self.id();
        let mut state = self.net.lock();
        let was = state.nodes.get_mut(&me).and_then(|n| n.advertising.take());
        if was.is_some() {
            for w in state.scanners_other_than(&me) {
                state.post(&w, TransportEvent::PeerLost { peer: me.clone() });
            }
        }
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        let me = self.id();
        let mut state = self.net.lock();
        let Some(node) = state.nodes.get_mut(&me) else {
            return Err(TransportError::Unavailable("node has shut down".into()));
        };
        node.scanning = true;
        // Immediately see everyone already advertising.
        let seen: Vec<(PeerId, Vec<u8>)> = state
            .nodes
            .iter()
            .filter(|(id, _)| id.as_str() != me)
            .filter_map(|(id, n)| n.advertising.clone().map(|p| (id.clone(), p)))
            .collect();
        for (peer, payload) in seen {
            state.post(&me, TransportEvent::PeerFound { peer, payload });
        }
        Ok(())
    }

    fn stop_scanning(&self) -> Result<(), TransportError> {
        let me = self.id();
        if let Some(node) = self.net.lock().nodes.get_mut(&me) {
            node.scanning = false;
        }
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        let me = self.id();
        let mut state = self.net.lock();
        if !state.nodes.contains_key(peer) {
            state.post(
                &me,
                TransportEvent::PipeFailed {
                    peer: peer.to_string(),
                    why: "no such peer".into(),
                },
            );
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        for (a, b) in [
            (me.clone(), peer.to_string()),
            (peer.to_string(), me.clone()),
        ] {
            if let Some(node) = state.nodes.get_mut(&a) {
                if !node.pipes.contains(&b) {
                    node.pipes.push(b);
                }
            }
        }
        // Always emitted, even if the pipe was already open (contract rule 2).
        state.post(
            &me,
            TransportEvent::PipeOpened {
                peer: peer.to_string(),
            },
        );
        state.post(peer, TransportEvent::PipeOpened { peer: me.clone() });
        Ok(())
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        let me = self.id();
        // The whole send happens under one lock, so concurrent sends to the
        // same peer can never interleave their chunks (contract rule 3).
        let state = self.net.lock();
        let open = state
            .nodes
            .get(&me)
            .is_some_and(|n| n.pipes.iter().any(|p| p == peer));
        if !open {
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        let chunk = state.max_chunk.unwrap_or(usize::MAX).max(1);
        for part in bytes.chunks(chunk) {
            state.post(
                peer,
                TransportEvent::Received {
                    peer: me.clone(),
                    bytes: part.to_vec(),
                },
            );
        }
        Ok(())
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        let me = self.id();
        let mut state = self.net.lock();
        let mut closed = false;
        for (a, b) in [
            (me.clone(), peer.to_string()),
            (peer.to_string(), me.clone()),
        ] {
            if let Some(node) = state.nodes.get_mut(&a) {
                let before = node.pipes.len();
                node.pipes.retain(|p| p != &b);
                closed |= node.pipes.len() != before;
            }
        }
        if closed {
            state.post(
                &me,
                TransportEvent::PipeClosed {
                    peer: peer.to_string(),
                },
            );
            state.post(peer, TransportEvent::PipeClosed { peer: me.clone() });
        }
        Ok(())
    }

    fn peers(&self) -> Vec<PeerId> {
        let me = self.id();
        let state = self.net.lock();
        if !state.nodes.get(&me).is_some_and(|n| n.scanning) {
            return Vec::new();
        }
        state
            .nodes
            .iter()
            .filter(|(id, n)| id.as_str() != me && n.advertising.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn pipes(&self) -> Vec<PeerId> {
        let me = self.id();
        self.net
            .lock()
            .nodes
            .get(&me)
            .map(|n| n.pipes.clone())
            .unwrap_or_default()
    }

    fn shutdown(&self) {
        let me = self.id();
        let peers = self.pipes();
        for p in peers {
            let _ = self.disconnect(&p);
        }
        let _ = self.stop_advertising();
        let _ = self.stop_scanning();
        // Revoke first, then drop the sender: anything still queued finds no
        // sink, so nothing escapes after we return (contract rule 6).
        *self.sink.write().unwrap_or_else(|e| e.into_inner()) = None;
        self.net.lock().nodes.remove(&me);
    }
}

impl Drop for LoopbackTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}
