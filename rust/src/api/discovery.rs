//! Core API v0 — discovery area. Toggle discoverability and read who's nearby.
//! Toggling emits a `discovery.updated` event.

use super::types::{BlockedPersonDto, NearbyDevice};

/// Turn Discovery on or off. Emits `discovery.updated`.
pub fn set_discovery(enabled: bool) -> Result<(), String> {
    crate::engine::set_discovery(enabled)
}

/// The devices currently visible (empty when Discovery is off).
pub fn nearby_devices() -> Result<Vec<NearbyDevice>, String> {
    crate::engine::nearby_devices()
}

/// Block a device (R0-F10). Silent and local: nothing reaches the person
/// blocked, and being blocked is indistinguishable from this device having
/// Discovery closed.
///
/// Revokes any pairing with them, tears down any live session, and takes them
/// off the nearby list. Emits `discovery.updated`.
pub fn block_device(device_id: String) -> Result<(), String> {
    crate::engine::block_device(device_id)
}

/// Block the person a conversation is with (R0-F10).
///
/// The way to block somebody who is out of range: a paired friend has a thread
/// whether the radio can see them or not, and no `device_id` while they are
/// away.
pub fn block_thread(thread_id: i64) -> Result<(), String> {
    crate::engine::block_thread(thread_id)
}

/// Lift a block. Restores stranger-level status and **not** the pairing it
/// revoked (R0-F10).
pub fn unblock_person(contact_id: i64) -> Result<(), String> {
    crate::engine::unblock_person(contact_id)
}

/// Everyone this device is currently refusing.
pub fn blocked_people() -> Result<Vec<BlockedPersonDto>, String> {
    crate::engine::blocked_people()
}
