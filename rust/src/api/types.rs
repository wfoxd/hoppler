//! Data types crossing the bridge (Core API v0). Plain, owned, frb-friendly —
//! no references, no crypto types (those stay behind the wrappers).

/// The local persona shown in Discovery and edited in the UI.
pub struct PersonaDto {
    pub name: String,
    /// Packed 0xRRGGBB.
    pub colour: u32,
    pub version: u32,
    /// Nobody has chosen this name yet — it is the placeholder the core made up.
    ///
    /// R0-F1 says the person chooses a display name on first launch, and until
    /// now no step ever asked, so every device shipped called "Me". The visible
    /// cost landed on the pairing screen, which read "Pairing with Me" on the
    /// one screen whose whole job is saying who is at the other end.
    ///
    /// Derived rather than stored as a flag: a persona is written down only
    /// when somebody chooses one, so "nothing stored" *is* "never chosen" and
    /// the two cannot disagree.
    pub needs_name: bool,
}

/// Someone the app can show: a device in range, or a person we have paired
/// with and are not currently in range of.
///
/// Both, because R0-F2 rotates transport ids every twelve minutes and R0-F4
/// says pairing survives that. A list built only from what the radio can see
/// drops a paired person the moment they stop advertising — the app forgetting
/// someone the user deliberately introduced it to.
pub struct NearbyDevice {
    /// Opaque transport handle, and `None` when they are not in range.
    ///
    /// The caller passes it back to `ping` / `send_chat` / `offer_drop`, so its
    /// absence *is* "cannot be reached right now" — one fact, not a presence
    /// flag alongside a handle that could disagree with it. Deliberately not a
    /// stand-in identity when they are away: an earlier attempt put a durable
    /// pseudonym in this field and made every transport-routed action on an
    /// away row unroutable, because Ping and Drop had nothing to dial.
    pub device_id: Option<String>,
    /// The conversation with this person, once there is one.
    ///
    /// What an away row is addressed by. A thread outlives every id the peer
    /// advertises under, so it is the handle that still means something when
    /// the radio can no longer see them.
    pub thread_id: Option<i64>,
    pub name: String,
    pub colour: u32,
    pub paired: bool,
}

/// One colour of a pairing SAS: what to paint, and what to call it.
///
/// Both, because a swatch cannot be compared over a phone call and cannot be
/// compared at all by a colour-blind person — and this is the step that decides
/// whether their pairing is safe.
pub struct SasColourDto {
    pub name: String,
    /// Packed 0xRRGGBB.
    pub rgb: u32,
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
    /// How far this one has got, for outgoing messages.
    ///
    /// Carried because the difference is real and was invisible: until the
    /// acknowledgement existed, `Sent` was terminal, so a message the other
    /// person is reading looked exactly like one they refused — and one they
    /// refused is a thing that happened, silently, on two phones.
    ///
    /// Meaningless on an incoming message, which is stored `Delivered` the
    /// moment it is written, so the screen reads this on its own lines only.
    pub state: MessageStateDto,
}

/// What a message's row is entitled to claim.
///
/// Deliberately three states rather than a `delivered: bool`. A flag collapses
/// "we have not sent it yet" and "we sent it and heard nothing" into one
/// negative, and those are the two the person on the other end of a queued
/// backlog most needs told apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageStateDto {
    /// Written, and still on this device. Nothing has left.
    Queued,
    /// The bytes left. Nobody has confirmed anything, and on an unpaired
    /// thread nobody ever will — there is no ratchet to seal an
    /// acknowledgement with, so this is as far as those rows go.
    Sent,
    /// The far side said it has it, sealed on a chain only they hold.
    Delivered,
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
    /// A message this device sent has got further along.
    ///
    /// The state is on the row the moment it changes, and until this existed
    /// nothing told the screen so — the conversation redrew when a message
    /// *arrived* and at no other time. A line sat at "not confirmed" while the
    /// store already said delivered, and only leaving and coming back put it
    /// right. Watching your own message is exactly when somebody is looking.
    MessageStateChanged {
        thread_id: i64,
        msg_id: String,
        state: MessageStateDto,
    },
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
    /// A pairing ceremony reached the point where two people compare colours.
    ///
    /// Nothing has been disclosed and nothing is written down. The only thing
    /// that may happen next is a person looking at a screen and deciding.
    ///
    /// The colours are a list rather than a fixed pair on purpose. How many
    /// there are is a security parameter — each one is four bits of what an
    /// active relay has to guess — and that number is still open. A UI written
    /// against a list survives the decision; one written against `first` and
    /// `second` does not.
    ///
    /// Order carries information: "teal then amber" is not "amber then teal",
    /// so it must be rendered as given and never sorted.
    ///
    /// Carried directly on the event rather than wrapped in a `Sas` struct,
    /// which is not tidiness — it is the only version with working equality in
    /// Dart. The generated enum variants compare list fields with
    /// `DeepCollectionEquality`, but a nested plain struct is compared with its
    /// own `==`, and the generator gives those reference equality on lists. So
    /// two identical SAS values would have compared unequal, silently, for as
    /// long as anyone cared to look.
    PairingSas {
        device_id: String,
        colours: Vec<SasColourDto>,
        word: String,
    },
    /// The other person confirmed.
    ///
    /// A cue, not a result: on its own it means nothing has crossed. Showing it
    /// as "paired" would be a lie the protocol is specifically built to avoid.
    PairingPeerConfirmed { device_id: String },
    /// Both people confirmed, both identities checked out, and the thread
    /// exists. This is a pairing (R0-F4).
    PairingCompleted {
        device_id: String,
        thread_id: i64,
        name: String,
        colour: u32,
    },
    /// A ceremony ended without pairing.
    ///
    /// One wording for every cause. A substituted code, a bad proof, a message
    /// out of order and a pipe that died are different things internally and
    /// the same thing to the person holding the phone — and telling them apart
    /// on screen would also tell a prober which of its guesses was closer.
    PairingFailed { device_id: String, reason: String },
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

/// One of the colours a person may pick for themselves (R0-F1).
///
/// Carries its name because a row of coloured circles is unusable to somebody
/// who cannot tell them apart, and roughly one man in twelve cannot tell at
/// least two of these apart. The name is what the screen reader reads and what
/// distinguishes the swatches when the colour does not.
pub struct PersonaColourDto {
    /// What to call it: "blue", "coral".
    pub name: String,
    /// Packed 0xRRGGBB, as [`PersonaDto::colour`].
    pub value: u32,
}
