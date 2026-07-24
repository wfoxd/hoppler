//! Core API v0 — sessions/threads area. Ping, Chat, and reading threads.

use super::types::{ChatMessageDto, ThreadSummary};

/// Ping a nearby device. Emits `Pinged`.
pub fn ping(device_id: String) -> Result<(), String> {
    crate::engine::ping(device_id)
}

/// Send a chat message to a device, returning the stored outgoing message. The
/// reply arrives later as a `message.received` event (not before this returns).
pub fn send_chat(device_id: String, text: String) -> Result<ChatMessageDto, String> {
    crate::engine::send_chat(device_id, text)
}

/// All messages on a thread, in chronological display order.
pub fn thread_messages(thread_id: i64) -> Result<Vec<ChatMessageDto>, String> {
    crate::engine::thread_messages(thread_id)
}

/// The existing thread with a device, if any (open a conversation without
/// sending first).
pub fn thread_for_device(device_id: String) -> Result<Option<i64>, String> {
    crate::engine::thread_for_device(device_id)
}

/// All conversations, for the thread list.
pub fn list_threads() -> Result<Vec<ThreadSummary>, String> {
    crate::engine::list_threads()
}
