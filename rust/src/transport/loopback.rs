//! An in-process transport: a shared "airspace" several nodes join, discover
//! each other in, and exchange bytes over — with no sockets, no radios, and no
//! timing nondeterminism.
//!
//! This is not a mock of the trait; it is a real implementation of it, which is
//! what makes it useful: the session layer (T10) can drive Noise handshakes and
//! framing over it in a unit test and get the same semantics a radio gives —
//! ordered, reliable, unframed bytes — deterministically and in microseconds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{EventSink, PeerId, Transport, TransportError, TransportEvent};

#[derive(Default)]
struct Node {
    sink: Option<Arc<EventSink>>,
    advertising: Option<Vec<u8>>,
    scanning: bool,
    pipes: Vec<PeerId>,
}

#[derive(Default)]
struct NetState {
    nodes: HashMap<PeerId, Node>,
}

/// A shared airspace. Clone it to hand the same airspace to several nodes.
#[derive(Clone, Default)]
pub struct LoopbackNet {
    state: Arc<Mutex<NetState>>,
}

impl LoopbackNet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Join the airspace as `id`, delivering events to `sink`.
    pub fn join(&self, id: impl Into<PeerId>, sink: EventSink) -> LoopbackTransport {
        let id = id.into();
        let mut state = self.lock();
        state.nodes.entry(id.clone()).or_default().sink = Some(Arc::new(sink));
        drop(state);
        LoopbackTransport {
            id,
            net: self.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One node's view of the airspace.
pub struct LoopbackTransport {
    id: PeerId,
    net: LoopbackNet,
}

impl LoopbackTransport {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Queue a delivery to `peer`, to be dispatched *after* the net lock is
    /// released — a transport must never call a sink while holding a lock.
    fn collect(state: &NetState, peer: &str, event: TransportEvent) -> Option<Delivery> {
        state
            .nodes
            .get(peer)
            .and_then(|n| n.sink.clone())
            .map(|sink| Delivery { sink, event })
    }
}

struct Delivery {
    sink: Arc<EventSink>,
    event: TransportEvent,
}

fn dispatch(deliveries: Vec<Delivery>) {
    for d in deliveries {
        (d.sink)(d.event);
    }
}

impl Transport for LoopbackTransport {
    fn name(&self) -> &'static str {
        "loopback"
    }

    fn start_advertising(&self, payload: Vec<u8>) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        state.nodes.entry(self.id.clone()).or_default().advertising = Some(payload.clone());

        // Everyone currently scanning sees us (a re-advertise refreshes them).
        let watchers: Vec<PeerId> = state
            .nodes
            .iter()
            .filter(|(id, n)| n.scanning && *id != &self.id)
            .map(|(id, _)| id.clone())
            .collect();
        let deliveries: Vec<Delivery> = watchers
            .iter()
            .filter_map(|w| {
                LoopbackTransport::collect(
                    &state,
                    w,
                    TransportEvent::PeerFound {
                        peer: self.id.clone(),
                        payload: payload.clone(),
                    },
                )
            })
            .collect();
        drop(state);
        dispatch(deliveries);
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        state.nodes.entry(self.id.clone()).or_default().advertising = None;

        let watchers: Vec<PeerId> = state
            .nodes
            .iter()
            .filter(|(id, n)| n.scanning && *id != &self.id)
            .map(|(id, _)| id.clone())
            .collect();
        let deliveries: Vec<Delivery> = watchers
            .iter()
            .filter_map(|w| {
                LoopbackTransport::collect(
                    &state,
                    w,
                    TransportEvent::PeerLost {
                        peer: self.id.clone(),
                    },
                )
            })
            .collect();
        drop(state);
        dispatch(deliveries);
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        state.nodes.entry(self.id.clone()).or_default().scanning = true;

        // Immediately see everyone already advertising.
        let seen: Vec<(PeerId, Vec<u8>)> = state
            .nodes
            .iter()
            .filter(|(id, _)| *id != &self.id)
            .filter_map(|(id, n)| n.advertising.clone().map(|p| (id.clone(), p)))
            .collect();
        let deliveries: Vec<Delivery> = seen
            .into_iter()
            .filter_map(|(peer, payload)| {
                LoopbackTransport::collect(
                    &state,
                    &self.id,
                    TransportEvent::PeerFound { peer, payload },
                )
            })
            .collect();
        drop(state);
        dispatch(deliveries);
        Ok(())
    }

    fn stop_scanning(&self) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        state.nodes.entry(self.id.clone()).or_default().scanning = false;
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        if !state.nodes.contains_key(peer) {
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        for (a, b) in [
            (self.id.clone(), peer.to_string()),
            (peer.to_string(), self.id.clone()),
        ] {
            let node = state.nodes.entry(a).or_default();
            if !node.pipes.contains(&b) {
                node.pipes.push(b);
            }
        }
        // Both ends learn the pipe is open.
        let deliveries: Vec<Delivery> = [self.id.as_str(), peer]
            .iter()
            .filter_map(|who| {
                let other = if *who == self.id {
                    peer
                } else {
                    self.id.as_str()
                };
                LoopbackTransport::collect(
                    &state,
                    who,
                    TransportEvent::PipeOpened {
                        peer: other.to_string(),
                    },
                )
            })
            .collect();
        drop(state);
        dispatch(deliveries);
        Ok(())
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        let state = self.net.lock();
        let open = state
            .nodes
            .get(&self.id)
            .is_some_and(|n| n.pipes.iter().any(|p| p == peer));
        if !open {
            return Err(TransportError::NoSuchPeer(peer.to_string()));
        }
        let delivery = LoopbackTransport::collect(
            &state,
            peer,
            TransportEvent::Received {
                peer: self.id.clone(),
                bytes: bytes.to_vec(),
            },
        );
        drop(state);
        dispatch(delivery.into_iter().collect());
        Ok(())
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        let mut state = self.net.lock();
        let mut closed = false;
        for (a, b) in [
            (self.id.clone(), peer.to_string()),
            (peer.to_string(), self.id.clone()),
        ] {
            if let Some(node) = state.nodes.get_mut(&a) {
                let before = node.pipes.len();
                node.pipes.retain(|p| p != &b);
                closed |= node.pipes.len() != before;
            }
        }
        let deliveries: Vec<Delivery> = if closed {
            [self.id.as_str(), peer]
                .iter()
                .filter_map(|who| {
                    let other = if *who == self.id {
                        peer
                    } else {
                        self.id.as_str()
                    };
                    LoopbackTransport::collect(
                        &state,
                        who,
                        TransportEvent::PipeClosed {
                            peer: other.to_string(),
                        },
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        drop(state);
        dispatch(deliveries);
        Ok(())
    }

    fn shutdown(&self) {
        let _ = self.stop_advertising();
        let _ = self.stop_scanning();
        let peers: Vec<PeerId> = {
            let state = self.net.lock();
            state
                .nodes
                .get(&self.id)
                .map(|n| n.pipes.clone())
                .unwrap_or_default()
        };
        for p in peers {
            let _ = self.disconnect(&p);
        }
        self.net.lock().nodes.remove(&self.id);
    }
}

impl Drop for LoopbackTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}
