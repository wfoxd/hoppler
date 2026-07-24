//! Core API v0 — sessions/threads area. Ping, Chat, and reading a thread.

use super::types::ChatMessageDto;

/// Ping a nearby device. Emits `Pinged`.
pub fn ping(device_id: String) -> Result<(), String> {
    crate::engine::ping(device_id)
}

/// Send a chat message to a device, returning the stored outgoing message. A
/// reply arrives asynchronously as a `message.received` event.
pub fn send_chat(device_id: String, text: String) -> Result<ChatMessageDto, String> {
    crate::engine::send_chat(device_id, text)
}

/// All messages on a thread, ordered as stored.
pub fn thread_messages(thread_id: i64) -> Result<Vec<ChatMessageDto>, String> {
    crate::engine::thread_messages(thread_id)
}
