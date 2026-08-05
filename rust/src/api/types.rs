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
    /// A device pinged us.
    Pinged { device_id: String, name: String },
    /// A ping we sent was answered.
    ///
    /// Separate from the "pinged" event, which is someone nudging us. While the
    /// two were one event, a tap looked answered only if the other person
    /// happened to nudge back — so an ordinary ping always timed out, and an
    /// unrelated incoming one was mistaken for the answer.
    ///
    /// Named in prose rather than as a Rust path, because this comment is
    /// copied verbatim into the generated Dart, where such a link points at
    /// nothing.
    PingAcked { device_id: String },
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
    /// The radio became usable or unusable.
    ///
    /// A device list cannot carry this on its own: an empty list is what both
    /// "the radio is off" and "nobody is nearby" look like, and R0-F2 turns on
    /// the user being able to tell those apart. `reason` is absent when the
    /// radio is fine, and a sentence fit to show when it is not — written by
    /// the rung, because only the rung knows.
    ///
    /// `available` is the source of truth; `reason` is decoration on top of it.
    /// A report can be unavailable with nothing to say, so anything deciding
    /// from the presence of a reason gets the state backwards.
    ///
    /// Kept free of Rust names and Rust paths on purpose: this comment is
    /// copied verbatim into the generated Dart, where `None` and
    /// `CoreEvent::DiscoveryUpdated` mean nothing to the reader.
    RadioChanged {
        available: bool,
        reason: Option<String>,
    },
}
