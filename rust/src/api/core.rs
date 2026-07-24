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

/// Initialise the core at `support_dir` (the app's private data directory).
/// Returns the local persona. Call once at startup, before other API calls.
pub fn core_init(support_dir: String) -> Result<PersonaDto, String> {
    crate::engine::init(support_dir)
}
