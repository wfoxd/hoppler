//! Data types crossing the bridge (Core API v0). Plain, owned, frb-friendly —
//! no references, no crypto types (those stay behind the wrappers).

/// The local persona shown in Discovery and edited in the UI.
pub struct PersonaDto {
    pub name: String,
    /// Packed 0xRRGGBB.
    pub colour: u32,
    pub version: u32,
}

/// A device visible in Discovery.
pub struct NearbyDevice {
    /// Opaque stable handle for this device (the caller passes it back to
    /// `ping` / `send_chat` / `offer_drop`).
    pub device_id: String,
    pub name: String,
    pub colour: u32,
    pub paired: bool,
}

/// A conversation, for the UI's thread list.
pub struct ThreadSummary {
    pub thread_id: i64,
    pub name: String,
    pub colour: u32,
}

/// A stored chat message, as the UI sees it.
pub struct ChatMessageDto {
    pub msg_id: String,
    pub thread_id: i64,
    pub text: String,
    pub outgoing: bool,
    pub created_at: i64,
}

/// An event pushed from the core to Dart over the single event stream. Variants
/// mirror the tech-spec event names (`discovery.updated`, `message.received`,
/// `transfer.progress`, `transfer.completed`, …).
pub enum CoreEvent {
    /// The nearby-device list changed (Discovery toggled or peers moved).
    DiscoveryUpdated { devices: Vec<NearbyDevice> },
    /// A device pinged us (or our ping was acknowledged).
    Pinged { device_id: String, name: String },
    /// A Ping could not be delivered — the pipe never opened.
    ///
    /// Only ever raised for a peer we failed to *reach*. A blocked peer accepts
    /// the connection and then goes quiet during the handshake, so it produces
    /// no event at all — which is what keeps "blocked" indistinguishable from
    /// "not there" (R0-F10).
    PingFailed { device_id: String, reason: String },
    /// A message arrived on a thread.
    MessageReceived {
        thread_id: i64,
        msg_id: String,
        text: String,
    },
    /// Progress on an in-flight transfer.
    TransferProgress {
        transfer_id: String,
        received: u64,
        total: u64,
    },
    /// A transfer finished (or failed).
    TransferCompleted { transfer_id: String, success: bool },
}
