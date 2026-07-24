//! Core API v0 — discovery area. Toggle discoverability and read who's nearby.
//! Toggling emits a `discovery.updated` event.

use super::types::NearbyDevice;

/// Turn Discovery on or off. Emits `discovery.updated`.
pub fn set_discovery(enabled: bool) -> Result<(), String> {
    crate::engine::set_discovery(enabled)
}

/// The devices currently visible (empty when Discovery is off).
pub fn nearby_devices() -> Result<Vec<NearbyDevice>, String> {
    crate::engine::nearby_devices()
}
