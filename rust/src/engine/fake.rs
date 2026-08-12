//! The fake network — scripted peers and canned transfers so all UI and module
//! work can proceed (and be contract-tested) before the radio plane (T08–T10)
//! exists. This is the UI team's daily environment, not throwaway scaffolding.
//!
//! Everything here is deterministic and synchronous: operations emit their
//! events in a burst rather than over wall-clock time, so contract tests never
//! race. The real transports will replace this behind the same `crate::api`
//! surface without the UI noticing.

use crate::api::types::NearbyDevice;

/// A scripted peer.
pub struct FakePeer {
    pub device_id: &'static str,
    pub name: &'static str,
    pub colour: u32,
    pub paired: bool,
}

/// The peers Discovery "sees" when it is open.
pub const PEERS: &[FakePeer] = &[
    FakePeer {
        device_id: "fake-sam",
        name: "Sam",
        colour: 0x00e0_5c5c,
        paired: true,
    },
    FakePeer {
        device_id: "fake-mai",
        name: "Mai",
        colour: 0x0044_88ff,
        paired: false,
    },
    FakePeer {
        device_id: "fake-kit",
        name: "Kit",
        colour: 0x0033_bb77,
        paired: false,
    },
];

pub fn nearby() -> Vec<NearbyDevice> {
    PEERS
        .iter()
        .map(|p| NearbyDevice {
            device_id: p.device_id.to_owned(),
            name: p.name.to_owned(),
            colour: p.colour,
            paired: p.paired,
        })
        .collect()
}

pub fn peer(device_id: &str) -> Option<&'static FakePeer> {
    PEERS.iter().find(|p| p.device_id == device_id)
}

/// A canned reply a peer "sends back" to our chat message.
pub fn canned_reply(text: &str) -> String {
    format!("(echo) {text}")
}

/// A stand-in pseudonym for a peer we have not yet held a session with, so the
/// store has a stable key to hang a contact on.
///
/// Not a pseudonym anyone proved: it is a hash of the rotating device id, which
/// R0-F2 changes every twelve minutes. That is why `reconcile_contact` exists —
/// a row keyed here is moved onto the real, handshake-proved pseudonym as soon
/// as there is one. It was called `fake_l1_pub` while `contacts` had a column
/// called `l1_pub`; both names were wrong in the same way.
pub fn placeholder_pseudonym(device_id: &str) -> [u8; 32] {
    crate::crypto::hash::hash(device_id.as_bytes())
}
