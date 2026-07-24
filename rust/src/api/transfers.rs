//! Core API v0 — transfers area. Offer a Drop; progress and completion arrive
//! as `transfer.progress` / `transfer.completed` events.

/// Offer a file (Drop) to a device. Returns the transfer id; progress and
/// completion are delivered over the event stream.
pub fn offer_drop(device_id: String, name: String, size: u64) -> Result<String, String> {
    crate::engine::offer_drop(device_id, name, size)
}
