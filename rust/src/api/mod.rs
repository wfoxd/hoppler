//! Core API v0 — the versioned, module-facing bridge surface (G-3, tech spec
//! §2). Everything user-facing consumes *only* this; the engine and internal
//! modules stay behind it. Six areas: core · identity · discovery · sessions
//! (messaging) · transfers · events. See `docs/CORE_API.md` for the contract.

pub mod core;
pub mod discovery;
pub mod events;
pub mod identity;
pub mod messaging;
pub mod transfers;
pub mod types;
