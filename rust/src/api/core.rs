//! Core API v0 — lifecycle and version. See `docs/CORE_API.md` for the
//! versioning contract (breaking changes bump the version).

use super::types::PersonaDto;

#[flutter_rust_bridge::frb(sync)]
pub fn core_version() -> String {
    format!("libhoppler {}", env!("CARGO_PKG_VERSION"))
}

/// The core API contract version. Breaking changes to any `crate::api` surface
/// bump this (and `docs/CORE_API.md`).
#[flutter_rust_bridge::frb(sync)]
pub fn api_version() -> String {
    "v0".to_string()
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Which radio the core should run on.
///
/// One rung, not the tech spec §9 ladder — running several at once is T15's
/// job. The choice exists because the BLE acceptance needs the radio in
/// isolation: with LAN also running, a peer found over Wi-Fi looks exactly like
/// one found over the air and the run proves nothing about the radio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioChoice {
    /// mDNS and TCP. The default, and what every hardware run so far used.
    Lan,
    /// The BLE rung, driven through the host seam. A host dispatcher must be
    /// attached before `core_init` or every command simply queues.
    Ble,
}

/// Initialise the core at `support_dir` (the app's private data directory).
/// Returns the local persona. Call once at startup, before other API calls.
pub fn core_init(support_dir: String, radio: RadioChoice) -> Result<PersonaDto, String> {
    crate::engine::init_on(
        support_dir,
        match radio {
            RadioChoice::Lan => crate::engine::Radio::Lan,
            RadioChoice::Ble => crate::engine::Radio::Ble,
        },
    )
}
