//! The event bus: a single ordered stream of [`CoreEvent`]s to Dart.

use super::types::CoreEvent;
use crate::frb_generated::StreamSink;

/// Subscribe to the core event stream. Call once at startup; the core pushes
/// every `discovery.updated` / `message.received` / `transfer.*` event here.
pub fn core_event_stream(sink: StreamSink<CoreEvent>) {
    crate::engine::set_event_sink(sink);
}
