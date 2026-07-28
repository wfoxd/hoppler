//! The session layer (T10) — tech spec §5, R0-F5.
//!
//! A Noise IK handshake ([`handshake`]) followed by AEAD-framed traffic. See
//! that module for why the pattern changed from XX and what each static is.

pub mod frame;
pub mod handshake;
