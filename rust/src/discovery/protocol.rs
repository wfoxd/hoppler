//! The persona endpoint wire format (T09) — tech spec §4.
//!
//! Two frames and one deliberate absence.
//!
//! ```text
//!   requester → responder   [version:1][pseudonym:32]        33 bytes, fixed
//!   responder → requester   [version:1][len:2][record...]    on disclosure
//!   responder → requester   (nothing at all)                 on refusal
//! ```
//!
//! # Requester-first, and why the shape carries the security property
//!
//! The requester speaks first, presenting its pseudonym toward us. Everything
//! that follows from that ordering is a property we get for free rather than
//! one we have to remember to enforce: rate limiting happens before any
//! disclosure, and a blocked requester is refused before we have said anything
//! at all.
//!
//! # The null response is *silence*
//!
//! R0-F10 requires that a blocked device observe nothing distinguishable from
//! the responder simply having Discovery off. The cheapest way to guarantee
//! that is to make refusal indistinguishable from refusal — the same zero
//! bytes, on both paths, with no branch that could ever diverge.
//!
//! A fixed non-empty "unavailable" frame would satisfy F10 too, but it tells
//! anyone who asks that a Hoppler device is here and chose not to answer.
//! Silence tells them nothing they did not already know from the connection
//! itself. This also matches T10, which requires zero response frames to a
//! blocked initiator.
//!
//! The consequence is deliberate: a requester cannot tell "off" from "blocked"
//! from "gone", and neither can the UI. That is the requirement, not a gap.

use crate::crypto::dh;

/// Wire version. Bumped if any frame's shape changes; a mismatched version is
/// refused rather than guessed at.
pub const VERSION: u8 = 1;

/// Length of a pseudonym on the wire (an X25519 public key).
pub const PSEUDONYM_LEN: usize = 32;

/// `[version][pseudonym]`.
pub const REQUEST_LEN: usize = 1 + PSEUDONYM_LEN;

/// Ceiling on a persona record we will emit or accept. Matches the identity
/// layer's own bound so a record that round-trips here cannot be refused there.
pub const MAX_RECORD_LEN: usize = 4096;

/// A requester asking for our persona.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The requester's pseudonym toward us.
    ///
    /// **Claimed, not proven.** Nothing in this frame demonstrates that the
    /// sender holds the matching secret, so a blocklist consulted here is
    /// advisory: an evader can present any 32 bytes. T10 closes this by making
    /// the pseudonym the initiator's Noise static, which the handshake proves.
    /// Until then, treat a match as "this is who they say they are".
    pub pseudonym: [u8; PSEUDONYM_LEN],
}

impl Request {
    /// All-zero: "I have never seen your persona, so I cannot derive one yet."
    ///
    /// A pseudonym is derived toward the *responder's* Layer-2 key, which a
    /// first-time requester has not learned — that is what it is asking for.
    /// The zero pseudonym names that case explicitly instead of leaving the
    /// responder to guess at 32 arbitrary bytes.
    pub const UNKNOWN: [u8; PSEUDONYM_LEN] = [0u8; PSEUDONYM_LEN];

    pub fn new(pseudonym: [u8; PSEUDONYM_LEN]) -> Self {
        Self { pseudonym }
    }

    /// A first-contact request, before the responder's persona is known.
    pub fn first_contact() -> Self {
        Self::new(Self::UNKNOWN)
    }

    pub fn from_public(key: &dh::DhPublic) -> Self {
        Self::new(key.0)
    }

    pub fn is_first_contact(&self) -> bool {
        self.pseudonym == Self::UNKNOWN
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(REQUEST_LEN);
        out.push(VERSION);
        out.extend_from_slice(&self.pseudonym);
        out
    }

    /// Decode exactly one request. Rejects anything that is not the exact
    /// length — a stream is framed by fixed size here, so a short read is not a
    /// request yet and a long one is not a request at all.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != REQUEST_LEN {
            return Err(ProtocolError::Malformed);
        }
        if bytes[0] != VERSION {
            return Err(ProtocolError::Version(bytes[0]));
        }
        let mut pseudonym = [0u8; PSEUDONYM_LEN];
        pseudonym.copy_from_slice(&bytes[1..]);
        Ok(Self { pseudonym })
    }
}

/// What the responder sends back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Our signed persona record.
    Persona(Vec<u8>),
    /// Nothing. Discovery is off, the requester is blocked, or it asked too
    /// often — the three are indistinguishable by construction, which is R0-F10
    /// falling out of the protocol shape rather than being enforced on top.
    Silence,
}

impl Response {
    /// Encode for the wire. [`Response::Silence`] is the empty slice, and every
    /// refusal path must funnel through it rather than growing its own reply.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Response::Silence => Vec::new(),
            // A record too large for the length prefix would be truncated by
            // the cast below and put a corrupt frame on the wire. `Response` is
            // public, so this is reachable from outside this module — refuse
            // through the same silence every other refusal uses rather than
            // emitting something a peer would mis-parse.
            Response::Persona(record) if record.len() > MAX_RECORD_LEN => Vec::new(),
            Response::Persona(record) => {
                let mut out = Vec::with_capacity(3 + record.len());
                out.push(VERSION);
                out.extend_from_slice(&(record.len() as u16).to_be_bytes());
                out.extend_from_slice(record);
                out
            }
        }
    }

    /// Decode a response the requester has read. `Ok(None)` means "not a whole
    /// frame yet" — the transport does not preserve boundaries (T08 contract
    /// rule 3), so a caller feeds this a growing buffer.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, ProtocolError> {
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes[0] != VERSION {
            return Err(ProtocolError::Version(bytes[0]));
        }
        if bytes.len() < 3 {
            return Ok(None);
        }
        let len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if len > MAX_RECORD_LEN {
            return Err(ProtocolError::Malformed);
        }
        if bytes.len() < 3 + len {
            return Ok(None);
        }
        Ok(Some(Response::Persona(bytes[3..3 + len].to_vec())))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    /// Not a frame we can parse.
    Malformed,
    /// A version we do not speak.
    Version(u8),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Malformed => write!(f, "malformed discovery frame"),
            ProtocolError::Version(v) => write!(f, "unsupported discovery wire version {v}"),
        }
    }
}

impl std::error::Error for ProtocolError {}
