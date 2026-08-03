//! The platform seam: things only the host can do, and things only the host
//! can see.
//!
//! # Why this is not BLE-shaped
//!
//! Three separate needs have now wanted a way out to the host, and each one
//! arrived looking like a one-off:
//!
//! | Need | Host API | How it was met |
//! |---|---|---|
//! | Receive multicast | `WifiManager.MulticastLock` | hand-wired in `MainActivity` |
//! | Notice a network change | `ConnectivityManager` | attempted, then abandoned |
//! | Drive the BLE radio | the whole BLE stack | unbuilt — this module |
//!
//! Building the third as a BLE bridge would mean building the first two again
//! later, differently. So the seam carries *commands* and *facts*, and BLE is
//! merely the first thing to use it.
//!
//! # Direction, and why commands do not wait
//!
//! `flutter_rust_bridge` calls run Dart→Rust. This seam runs the other way, so
//! a command is enqueued for the host and **returns immediately** — it never
//! blocks waiting for the host to act.
//!
//! That is not a compromise, it is the contract the transport already states.
//! `connect` is acceptance rather than delivery (T08 rule 2), `send` is
//! acknowledged separately by `on_write_complete`, and a radio that cannot
//! advertise reports it as an availability fact. Everything the caller needs to
//! learn arrives as a fact, not as a return value.
//!
//! Blocking would also be the third instance of a defect this project has
//! already paid for twice: the LAN and BLE rungs both deadlocked by doing work
//! while holding a lock the callee needed. A platform call that waits on Dart,
//! made from a thread holding a pipe lock, is that bug with a longer fuse.

pub mod ble;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::transport::ble::{BleIngress, PlatformEvent};
use crate::transport::PeerId;

/// How many commands may pile up before a host attaches.
///
/// A handful is normal — the core builds its transports before Dart has
/// subscribed. Hundreds means the host never attached at all, and dropping
/// loudly beats growing without bound.
const MAX_QUEUED: usize = 256;

/// Something only the host can do.
///
/// Flat rather than nested by subsystem: a variant is cheaper to add than a
/// hierarchy is to change, and the host dispatches on it either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformCommand {
    /// Tell the radio how we are named. Sent on every rotation, and while
    /// Discovery is off — a hidden node still dials paired peers and must
    /// introduce itself by the core's id rather than its MAC (R0-F2).
    BleSetLocalId {
        local_id: String,
    },
    BleStartAdvertising {
        payload: Vec<u8>,
    },
    BleStopAdvertising,
    BleStartScanning,
    BleStopScanning,
    BleConnect {
        peer: PeerId,
    },
    BleSend {
        peer: PeerId,
        bytes: Vec<u8>,
    },
    BleDisconnect {
        peer: PeerId,
    },
    BleShutdown,
}

/// Something the host observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformFact {
    Ble(PlatformEvent),
    /// Bytes the radio has finished with, releasing send-window credit.
    BleWriteComplete {
        peer: PeerId,
        bytes: u64,
    },
}

type Outbound = Arc<dyn Fn(PlatformCommand) + Send + Sync>;

/// Carries commands to the host and facts back.
#[derive(Default)]
pub struct PlatformBridge {
    inner: Mutex<Bridge>,
    ble: Mutex<Option<BleIngress>>,
}

#[derive(Default)]
struct Bridge {
    outbound: Option<Outbound>,
    /// Commands issued before a host attached. Drained in order on attach, so
    /// the first thing the radio hears is still `BleSetLocalId` rather than
    /// whatever happened to be sent after Dart woke up.
    queued: VecDeque<PlatformCommand>,
}

impl PlatformBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand a command to the host, or hold it until one attaches.
    pub fn send(&self, command: PlatformCommand) {
        let mut bridge = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match bridge.outbound.clone() {
            // Emitted under the guard on purpose. The sink only enqueues onto
            // Dart's event loop, so this does not block, and holding the lock
            // is what stops a concurrent `attach` from draining the backlog
            // *after* this command has already gone direct — which would
            // deliver `BleStartAdvertising` before the `BleSetLocalId` it
            // depends on. Ordering matters more here than lock granularity.
            Some(sink) => sink(command),
            None => {
                if bridge.queued.len() >= MAX_QUEUED {
                    log::warn!(
                        "platform command dropped: nothing has attached after {MAX_QUEUED} commands"
                    );
                    return;
                }
                bridge.queued.push_back(command);
            }
        }
    }

    /// Attach the host, and give it everything issued while it was absent.
    pub fn attach(&self, sink: Outbound) {
        let mut bridge = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let backlog = std::mem::take(&mut bridge.queued);
        bridge.outbound = Some(sink.clone());
        // Still under the guard, for the ordering reason in `send`.
        for command in backlog {
            sink(command);
        }
    }

    /// Forget the host. Commands queue again rather than vanishing.
    pub fn detach(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbound = None;
    }

    /// Route BLE facts to this rung.
    pub fn attach_ble(&self, ingress: BleIngress) {
        *self.ble.lock().unwrap_or_else(|e| e.into_inner()) = Some(ingress);
    }

    /// Deliver a fact from the host.
    ///
    /// A fact for a rung that no longer exists is dropped rather than being an
    /// error: the radio is asynchronous and the host may still be reporting on
    /// work issued before a shutdown. T08 rule 6 requires that to be tolerated.
    pub fn deliver(&self, fact: PlatformFact) {
        // Cloned out so the ingress is called with no lock held — it reaches
        // the transport, which takes its own.
        let ingress = self
            .ble
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .cloned();
        let Some(ingress) = ingress else { return };
        match fact {
            PlatformFact::Ble(event) => ingress.on_platform_event(event),
            PlatformFact::BleWriteComplete { peer, bytes } => {
                ingress.on_write_complete(&peer, bytes as usize)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> (Outbound, Arc<Mutex<Vec<PlatformCommand>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let sink: Outbound = Arc::new(move |c| {
            sink_seen.lock().unwrap().push(c);
        });
        (sink, seen)
    }

    fn id(n: &str) -> PlatformCommand {
        PlatformCommand::BleSetLocalId {
            local_id: n.to_string(),
        }
    }

    /// The core builds its transports before Dart has subscribed, so the first
    /// commands are always issued into nothing. Dropping them would leave the
    /// radio permanently unnamed — advertising under a MAC or not at all — with
    /// no error anywhere to explain it.
    #[test]
    fn commands_issued_before_a_host_attaches_are_not_lost() {
        let bridge = PlatformBridge::new();
        bridge.send(id("first"));
        bridge.send(PlatformCommand::BleStartScanning);

        let (sink, seen) = recorder();
        bridge.attach(sink);

        assert_eq!(
            *seen.lock().unwrap(),
            vec![id("first"), PlatformCommand::BleStartScanning]
        );
    }

    /// Order is the whole point of holding the backlog: advertising under an id
    /// the radio has not been told about is how a peer ends up announced by its
    /// MAC.
    #[test]
    fn a_backlog_is_delivered_before_anything_issued_after_it() {
        let bridge = PlatformBridge::new();
        bridge.send(id("early"));

        let (sink, seen) = recorder();
        bridge.attach(sink);
        bridge.send(id("late"));

        assert_eq!(*seen.lock().unwrap(), vec![id("early"), id("late")]);
    }

    #[test]
    fn a_host_that_never_attaches_does_not_grow_the_queue_without_bound() {
        let bridge = PlatformBridge::new();
        for _ in 0..MAX_QUEUED * 2 {
            bridge.send(PlatformCommand::BleStartScanning);
        }

        let (sink, seen) = recorder();
        bridge.attach(sink);

        assert_eq!(seen.lock().unwrap().len(), MAX_QUEUED);
    }

    /// Detaching is not shutting down: the host going away — a hot restart in
    /// development, an isolate torn down — must not silently discard the work
    /// the core keeps doing.
    #[test]
    fn commands_queue_again_once_the_host_detaches() {
        let bridge = PlatformBridge::new();
        let (sink, seen) = recorder();
        bridge.attach(sink);
        bridge.detach();
        bridge.send(id("while-away"));
        assert!(seen.lock().unwrap().is_empty());

        let (sink2, seen2) = recorder();
        bridge.attach(sink2);
        assert_eq!(*seen2.lock().unwrap(), vec![id("while-away")]);
    }

    /// The radio is asynchronous and reports on work issued before a shutdown,
    /// so a fact arriving with no rung attached is expected rather than
    /// exceptional (T08 rule 6).
    #[test]
    fn a_fact_with_no_rung_attached_is_dropped_rather_than_panicking() {
        let bridge = PlatformBridge::new();
        bridge.deliver(PlatformFact::Ble(PlatformEvent::PipeClosed {
            peer: "gone".into(),
        }));
    }
}
