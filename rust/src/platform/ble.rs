//! The BLE rung's half of the platform seam.
//!
//! [`BleTransport`](crate::transport::ble::BleTransport) holds the whole
//! transport contract in Rust and expects six commands out and seven facts
//! back. This turns those commands into [`PlatformCommand`]s; the facts come
//! back through [`PlatformBridge::deliver`].
//!
//! There is deliberately no logic here. Anything that can be decided in Rust
//! already was — duplicate opens, closes for pipes that never opened, silence
//! after shutdown, id validation, backpressure — so this file is a translation
//! and nothing else. Somewhere for a decision to hide is exactly what makes a
//! radio adapter hard to test.

use std::sync::Arc;

use super::{PlatformBridge, PlatformCommand};
use crate::transport::ble::BlePlatform;
use crate::transport::TransportError;

/// A radio driven by whatever host is attached to the bridge.
pub struct HostBleRadio {
    bridge: Arc<PlatformBridge>,
}

impl HostBleRadio {
    pub fn new(bridge: Arc<PlatformBridge>) -> Self {
        Self { bridge }
    }
}

/// Every method returns `Ok` once the command is handed over, and none waits
/// for the radio.
///
/// This is the contract the rung already publishes rather than a shortcut.
/// `connect` reports acceptance, with success arriving as `PipeOpened` and
/// failure as `PipeFailed` (T08 rule 2). `send` is acknowledged by
/// `on_write_complete`, which is what releases window credit. A radio that
/// cannot advertise says so as an availability fact, because "Bluetooth is
/// off" is a state the UI has to show anyway (R0-F2) and not an error belonging
/// to one call.
///
/// So there is nothing a return value could usefully carry that a fact does not
/// already carry better — and waiting for one would mean blocking a transport
/// thread on the host, which is how both the LAN and BLE rungs deadlocked
/// before.
impl BlePlatform for HostBleRadio {
    fn set_local_id(&self, local_id: &str) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleSetLocalId {
            local_id: local_id.to_string(),
        });
        Ok(())
    }

    fn start_advertising(&self, payload: &[u8]) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleStartAdvertising {
            payload: payload.to_vec(),
        });
        Ok(())
    }

    fn stop_advertising(&self) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleStopAdvertising);
        Ok(())
    }

    fn start_scanning(&self) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleStartScanning);
        Ok(())
    }

    fn stop_scanning(&self) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleStopScanning);
        Ok(())
    }

    fn connect(&self, peer: &str) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleConnect {
            peer: peer.to_string(),
        });
        Ok(())
    }

    fn send(&self, peer: &str, bytes: &[u8]) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleSend {
            peer: peer.to_string(),
            bytes: bytes.to_vec(),
        });
        Ok(())
    }

    fn disconnect(&self, peer: &str) -> Result<(), TransportError> {
        self.bridge.send(PlatformCommand::BleDisconnect {
            peer: peer.to_string(),
        });
        Ok(())
    }

    fn shutdown(&self) {
        self.bridge.send(PlatformCommand::BleShutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn radio() -> (HostBleRadio, Arc<Mutex<Vec<PlatformCommand>>>) {
        let bridge = Arc::new(PlatformBridge::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        bridge.attach(Arc::new(move |c| sink_seen.lock().unwrap().push(c)));
        (HostBleRadio::new(bridge), seen)
    }

    /// Every method must reach the host. A translation layer that quietly drops
    /// one is invisible: the rung reports success, the radio does nothing, and
    /// the symptom is two phones that cannot see each other with no error on
    /// either — the failure mode this whole module exists to make debuggable.
    #[test]
    fn every_command_reaches_the_host() {
        let (radio, seen) = radio();
        radio.set_local_id("abc").unwrap();
        radio.start_advertising(&[1, 2]).unwrap();
        radio.stop_advertising().unwrap();
        radio.start_scanning().unwrap();
        radio.stop_scanning().unwrap();
        radio.connect("peer").unwrap();
        radio.send("peer", &[3]).unwrap();
        radio.disconnect("peer").unwrap();
        radio.shutdown();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                PlatformCommand::BleSetLocalId {
                    local_id: "abc".into()
                },
                PlatformCommand::BleStartAdvertising {
                    payload: vec![1, 2]
                },
                PlatformCommand::BleStopAdvertising,
                PlatformCommand::BleStartScanning,
                PlatformCommand::BleStopScanning,
                PlatformCommand::BleConnect {
                    peer: "peer".into()
                },
                PlatformCommand::BleSend {
                    peer: "peer".into(),
                    bytes: vec![3]
                },
                PlatformCommand::BleDisconnect {
                    peer: "peer".into()
                },
                PlatformCommand::BleShutdown,
            ]
        );
    }

    /// A payload must cross intact. Truncation here would present as a peer
    /// whose persona never resolves, which reads as a discovery bug.
    #[test]
    fn a_payload_crosses_byte_for_byte() {
        let (radio, seen) = radio();
        let payload: Vec<u8> = (0u8..=255).collect();
        radio.start_advertising(&payload).unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![PlatformCommand::BleStartAdvertising { payload }]
        );
    }
}
