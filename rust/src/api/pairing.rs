//! Core API v0 — pairing area (R0-F4, tech spec §6).
//!
//! The in-person trust ceremony: one person shows a code, the other scans it,
//! both compare two colours and a word, both confirm. Out the other side is a
//! contact with a real Layer-1 identity and a persistent thread — the Circle
//! seed.
//!
//! Everything here is asynchronous in the way that matters: these calls start
//! or stop things, and what came of them arrives on the event stream as
//! `PairingSas`, `PairingPeerConfirmed`, `PairingCompleted` or `PairingFailed`.
//! A caller that treats `Ok` as "paired" has misread it — `Ok` means the
//! ceremony was accepted, and the only thing that pairs two people is two
//! people confirming.

/// Mint a code and start showing it. Returns the text to render as a QR code
/// (and, later, to write to an NFC tag — the payload is identical).
///
/// Every call mints a fresh one, so a code that has been dismissed cannot be
/// used from a photograph.
pub fn pairing_invite() -> Result<String, String> {
    crate::engine::pairing_invite()
}

/// Take the code off the screen.
///
/// A ceremony already under way is unaffected: the other person is looking at
/// colours, waiting, and putting this phone down is not a decision about them.
pub fn stop_showing_invite() -> Result<(), String> {
    crate::engine::stop_showing_invite()
}

/// Begin pairing from a code the camera read. Returns the device id the
/// ceremony is with, which is what the events coming back are keyed on.
///
/// `Err` means the code could not be read or names a device that is not
/// nearby. Everything after that — including the code being someone else's —
/// arrives as `PairingFailed`.
pub fn begin_pairing(code: String) -> Result<String, String> {
    crate::engine::begin_pairing(code)
}

/// This person confirmed the colours match.
///
/// Never enough on its own: nothing crosses until the other side confirms too
/// (R0-F4). The UI should say "waiting" after this, not "paired".
pub fn confirm_pairing(device_id: String) -> Result<(), String> {
    crate::engine::confirm_pairing(device_id)
}

/// Abandon a ceremony — the colours did not match, the screen closed, the other
/// person walked off. Nothing was written, so there is nothing to undo, and the
/// peer is told nothing.
pub fn cancel_pairing(device_id: String) -> Result<(), String> {
    crate::engine::cancel_pairing(device_id)
}
