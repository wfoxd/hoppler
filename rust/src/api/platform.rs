//! The host seam, as the app sees it (tech spec §2, G-3).
//!
//! Everything else in this API is the app asking the core to do something. This
//! is the one place it runs the other way: the core asks the *host* for things
//! only the host can do — drive a radio, hold a lock, watch an interface — and
//! the host reports back what it saw.
//!
//! The types here mirror [`crate::platform`] rather than re-exporting it, for
//! the same reason [`CoreEvent`](super::types::CoreEvent) mirrors `NetEvent`:
//! the app is allowed to depend on this surface and nothing behind it, and a
//! type that leaks through stops that being true the moment it changes shape.

use crate::frb_generated::StreamSink;
use crate::platform::{self, PlatformCommand, PlatformFact};
use crate::transport::ble::PlatformEvent;

/// Something the core needs the host to do.
///
/// The host is expected to attempt each one and report the outcome as a
/// [`HostFact`] — never by failing this call, which has already returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCommand {
    /// Name the radio. Arrives on every rotation, and while Discovery is off:
    /// a hidden node still dials paired peers and must introduce itself by this
    /// id rather than its hardware address (R0-F2).
    BleSetLocalId {
        local_id: String,
    },
    /// Advertise this payload, replacing any current advertisement.
    BleStartAdvertising {
        payload: Vec<u8>,
    },
    /// Stop advertising. A real radio stop, not a flag — R0-F2 is the whole
    /// point of the Discovery switch.
    BleStopAdvertising,
    BleStartScanning,
    BleStopScanning,
    /// Open a pipe. Acceptance only: report `BlePipeOpened` or `BlePipeFailed`.
    BleConnect {
        peer: String,
    },
    /// Write to an open pipe, in order, then report `BleWriteComplete` — which
    /// is what returns the core's send-window credit.
    BleSend {
        peer: String,
        bytes: Vec<u8>,
    },
    BleDisconnect {
        peer: String,
    },
    /// Release every radio resource. Facts arriving after this are tolerated.
    BleShutdown,
}

/// Something the host observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostFact {
    BlePeerFound {
        peer: String,
        payload: Vec<u8>,
    },
    BlePeerLost {
        peer: String,
    },
    BlePipeOpened {
        peer: String,
    },
    BlePipeFailed {
        peer: String,
        why: String,
    },
    BlePipeClosed {
        peer: String,
    },
    BleReceived {
        peer: String,
        bytes: Vec<u8>,
    },
    /// The radio became usable or unusable — Bluetooth toggled, permission
    /// revoked. Reported rather than inferred, so the UI can say why.
    BleAvailability {
        available: bool,
        reason: Option<String>,
    },
    /// Bytes the radio has finished with.
    BleWriteComplete {
        peer: String,
        bytes: u64,
    },
}

/// Subscribe to the host command stream. Call **before** `core_init`.
///
/// Commands issued before this lands are held rather than dropped, so a late
/// subscription costs latency and not correctness — but the core builds its
/// transports during `core_init`, so subscribing first is what keeps the very
/// first `BleSetLocalId` from waiting on the rest of startup.
pub fn platform_command_stream(sink: StreamSink<HostCommand>) {
    platform::bridge().attach(std::sync::Arc::new(move |command| {
        let _ = sink.add(HostCommand::from(command));
    }));
}

/// Report something the host observed.
pub fn platform_fact(fact: HostFact) {
    platform::bridge().deliver(fact.into());
}

impl From<PlatformCommand> for HostCommand {
    fn from(c: PlatformCommand) -> Self {
        match c {
            PlatformCommand::BleSetLocalId { local_id } => Self::BleSetLocalId { local_id },
            PlatformCommand::BleStartAdvertising { payload } => {
                Self::BleStartAdvertising { payload }
            }
            PlatformCommand::BleStopAdvertising => Self::BleStopAdvertising,
            PlatformCommand::BleStartScanning => Self::BleStartScanning,
            PlatformCommand::BleStopScanning => Self::BleStopScanning,
            PlatformCommand::BleConnect { peer } => Self::BleConnect { peer },
            PlatformCommand::BleSend { peer, bytes } => Self::BleSend { peer, bytes },
            PlatformCommand::BleDisconnect { peer } => Self::BleDisconnect { peer },
            PlatformCommand::BleShutdown => Self::BleShutdown,
        }
    }
}

impl From<HostFact> for PlatformFact {
    fn from(f: HostFact) -> Self {
        match f {
            HostFact::BlePeerFound { peer, payload } => {
                Self::Ble(PlatformEvent::PeerFound { peer, payload })
            }
            HostFact::BlePeerLost { peer } => Self::Ble(PlatformEvent::PeerLost { peer }),
            HostFact::BlePipeOpened { peer } => Self::Ble(PlatformEvent::PipeOpened { peer }),
            HostFact::BlePipeFailed { peer, why } => {
                Self::Ble(PlatformEvent::PipeFailed { peer, why })
            }
            HostFact::BlePipeClosed { peer } => Self::Ble(PlatformEvent::PipeClosed { peer }),
            HostFact::BleReceived { peer, bytes } => {
                Self::Ble(PlatformEvent::Received { peer, bytes })
            }
            HostFact::BleAvailability { available, reason } => {
                Self::Ble(PlatformEvent::Availability { available, reason })
            }
            HostFact::BleWriteComplete { peer, bytes } => Self::BleWriteComplete { peer, bytes },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command must survive the crossing. A variant that translates to
    /// the wrong thing — or silently to nothing — presents as a radio that
    /// ignores one instruction, which from the outside is two phones that
    /// cannot see each other and no error on either.
    #[test]
    fn commands_cross_the_boundary_unchanged() {
        let cases = vec![
            PlatformCommand::BleSetLocalId {
                local_id: "id".into(),
            },
            PlatformCommand::BleStartAdvertising {
                payload: vec![1, 2, 3],
            },
            PlatformCommand::BleStopAdvertising,
            PlatformCommand::BleStartScanning,
            PlatformCommand::BleStopScanning,
            PlatformCommand::BleConnect { peer: "p".into() },
            PlatformCommand::BleSend {
                peer: "p".into(),
                bytes: vec![9],
            },
            PlatformCommand::BleDisconnect { peer: "p".into() },
            PlatformCommand::BleShutdown,
        ];
        for case in cases {
            let crossed = HostCommand::from(case.clone());
            // Round-tripping would only prove the two enums agree with each
            // other, so compare against the shape the host actually receives.
            let expected = match &case {
                PlatformCommand::BleSetLocalId { local_id } => HostCommand::BleSetLocalId {
                    local_id: local_id.clone(),
                },
                PlatformCommand::BleStartAdvertising { payload } => {
                    HostCommand::BleStartAdvertising {
                        payload: payload.clone(),
                    }
                }
                PlatformCommand::BleStopAdvertising => HostCommand::BleStopAdvertising,
                PlatformCommand::BleStartScanning => HostCommand::BleStartScanning,
                PlatformCommand::BleStopScanning => HostCommand::BleStopScanning,
                PlatformCommand::BleConnect { peer } => {
                    HostCommand::BleConnect { peer: peer.clone() }
                }
                PlatformCommand::BleSend { peer, bytes } => HostCommand::BleSend {
                    peer: peer.clone(),
                    bytes: bytes.clone(),
                },
                PlatformCommand::BleDisconnect { peer } => {
                    HostCommand::BleDisconnect { peer: peer.clone() }
                }
                PlatformCommand::BleShutdown => HostCommand::BleShutdown,
            };
            assert_eq!(crossed, expected, "command changed crossing the boundary");
        }
    }

    /// A fact translated to the wrong event is worse than a dropped one: a
    /// `PipeFailed` arriving as `PipeClosed` tells the core a pipe it never had
    /// has ended, which is the contract violation T08 rule 2 exists to forbid.
    #[test]
    fn facts_cross_the_boundary_unchanged() {
        assert_eq!(
            PlatformFact::from(HostFact::BlePipeFailed {
                peer: "p".into(),
                why: "no".into()
            }),
            PlatformFact::Ble(PlatformEvent::PipeFailed {
                peer: "p".into(),
                why: "no".into()
            })
        );
        assert_eq!(
            PlatformFact::from(HostFact::BlePipeClosed { peer: "p".into() }),
            PlatformFact::Ble(PlatformEvent::PipeClosed { peer: "p".into() })
        );
        assert_eq!(
            PlatformFact::from(HostFact::BleAvailability {
                available: false,
                reason: Some("off".into())
            }),
            PlatformFact::Ble(PlatformEvent::Availability {
                available: false,
                reason: Some("off".into())
            })
        );
        assert_eq!(
            PlatformFact::from(HostFact::BleWriteComplete {
                peer: "p".into(),
                bytes: 7
            }),
            PlatformFact::BleWriteComplete {
                peer: "p".into(),
                bytes: 7
            }
        );
        assert_eq!(
            PlatformFact::from(HostFact::BleReceived {
                peer: "p".into(),
                bytes: vec![1, 2]
            }),
            PlatformFact::Ble(PlatformEvent::Received {
                peer: "p".into(),
                bytes: vec![1, 2]
            })
        );
    }
}
