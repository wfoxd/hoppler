//! The pairing ceremony (T11; tech spec §6, requirement R0-F4).
//!
//! Pairing is the one moment Layer-1 identities cross. Everything else in
//! Ring 0 — Discovery, Ping, stranger chat — runs on Layer-2 personas and
//! per-counterparty pseudonyms; a completed ceremony is what turns two
//! strangers into a persistent thread, the Circle seed.
//!
//! This module currently holds the two *formats* the ceremony is built on, and
//! nothing that talks:
//!
//! - [`invite`] — what the QR code and the NFC tap carry, and how to read one.
//! - [`sas`] — the short authentication string the two humans compare, derived
//!   from the finished handshake's transcript.
//!
//! Both are pure functions over bytes, which is deliberate: they are the parts
//! a wrong implementation gets wrong *silently*. A ceremony that never runs is
//! obvious; a ceremony whose SAS is derived from the wrong material still shows
//! two matching colours and still lets both people press confirm, while
//! authenticating nothing. So they are pinned by golden vectors here, before
//! any code depends on them.
//!
//! The handshake between them — Noise XX bound to the invite's nonce — the
//! Layer-1 exchange it carries, and the state machine that requires both
//! confirmations are still to come.

pub mod ceremony;
pub mod invite;
pub mod sas;
mod words;
