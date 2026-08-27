//! The protocol version — what two builds must agree on to talk at all.
//!
//! # Why this exists
//!
//! Ring 0 changed the *meaning* of bytes that kept the same shape. A build
//! running #80 has a seeded ratchet it never uses; from #81 every chat body is
//! sealed and the ratchet turns on each message. The frames are the same
//! length, the same kind, and the same layout. Put the two builds in a room and
//! a paired conversation corrupts in both directions: the old side stores the
//! new side's ratchet header and ciphertext *as the message text*, and the new
//! side drops the old side's plaintext as an AEAD failure. Neither notices.
//!
//! Silent corruption is the worst available outcome — worse than not
//! connecting, and far worse than an error. So two builds that do not agree on
//! this number refuse each other, and say so.
//!
//! # What it is not
//!
//! Not a negotiation. There is one version, and a peer either speaks it or is
//! turned away; nothing degrades to an older dialect. Ring 0 is pre-release and
//! nobody is owed compatibility yet, so the whole cost of this decision is that
//! two phones must be flashed together — which they already are.
//!
//! Not [`crate::crypto::envelope::WIRE_VERSION_V0`]. That byte guards a
//! `[version][protobuf]` framing that **nothing on any live path uses** — the
//! module has no caller outside its own tests. It was cited as the reason this
//! gate could wait, and it could not have refused anybody.
//!
//! # Where it is enforced
//!
//! Both places that establish durable state with a peer, and by construction
//! rather than by a caller remembering:
//!
//! - **The session**, in the Noise IK intro payload. It is checked inside
//!   `session::handshake::decode_intro`, which a handshake cannot complete
//!   without calling, so no frame can be delivered ahead of the check.
//!   The payload is encrypted to the responder's static, so the number is not
//!   visible to anyone watching the air.
//! - **The ceremony**, folded into the Noise XX prologue beside the invite
//!   nonce. A mismatch means nothing decrypts, which is the correct outcome for
//!   a pairing: it writes contact rows, a Layer-1 key and a ratchet seed, and
//!   two builds that disagree about the wire would be writing each other
//!   durable state neither can read.
//!
//! # When to change it
//!
//! Whenever the meaning or layout of anything inside a session or a ceremony
//! moves, including when the bytes keep their shape. Adding a frame *kind* is
//! not a bump — an unknown kind already refuses on its own. Changing what an
//! existing kind means is exactly what this is for.

/// The only protocol version this build speaks.
///
/// **1, not 0, on purpose.** Builds before this gate existed sent an intro
/// beginning `[record_len:2]`, and a persona record is a couple of hundred
/// bytes, so that first byte was always zero. Reserving 0 for "a build from
/// before there was a version" means those peers are diagnosed rather than
/// merely malformed — this build reads a 0 and can say *older version*
/// instead of *something is wrong with that message*.
pub const PROTOCOL_VERSION: u8 = 2;

/// The version a build that predates this gate appears to send.
pub const VERSION_BEFORE_THE_GATE: u8 = 0;
