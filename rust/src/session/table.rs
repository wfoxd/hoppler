//! The session table (T10) — lifecycle for established Noise sessions.
//!
//! One entry per counterparty, holding the transport keys, who they were proven
//! to be, and a reassembler for the byte stream. Everything above this layer
//! deals in [`Frame`]s and never sees a nonce.
//!
//! Memory a peer can make us hold is bounded by the framing rather than by a
//! budget. The length prefix is two bytes, so an *incomplete* message is under
//! 65537 bytes and completed ones are drained immediately — a peer that
//! dribbles for ever still holds under 64 KiB of ours. (Mid-push the buffer can
//! transiently carry a finished message plus the start of the next; the bound
//! is on what remains after draining, which is what actually accumulates.)
//!
//! # Idle sessions are dropped, not kept warm
//!
//! A session costs keys in memory and a peer that believes it can still reach
//! us. Ring 0 keeps no ratchet state (tech spec §5: the thread dies with the
//! session unless pairing upgrades it), so an idle session is pure liability —
//! there is nothing to resume and nothing lost by rebuilding it. [`Self::sweep`]
//! drops them on the caller's clock rather than a timer of its own, so a test
//! can age a session without waiting.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use snow::TransportState;

use super::frame::{self, Frame, FrameError, Reassembler};
use super::handshake::Established;
use crate::crypto::dh;
use crate::identity::VerifiedPersona;
use crate::transport::PeerId;

/// Bytes the AEAD adds to a Noise transport message. Named rather than a
/// round-number slack: an over-allocation that happens to be big enough hides
/// what the real relationship is, and the next person changing the cipher suite
/// has nothing to check against.
const AEAD_TAG_LEN: usize = 16;

/// How long a session may sit unused before it is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, PartialEq, Eq)]
pub enum SessionError {
    /// No session with that peer.
    Unknown(PeerId),
    /// The frame or stream was not usable.
    Frame(FrameError),
    /// The AEAD refused: wrong keys, tampering, or messages out of order.
    /// Deliberately coarse — a caller learns the session is dead, not why.
    Crypto,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Unknown(p) => write!(f, "no session with {p}"),
            SessionError::Frame(e) => write!(f, "{e}"),
            SessionError::Crypto => write!(f, "session cryptography failed"),
        }
    }
}

impl std::error::Error for SessionError {}

struct Session {
    keys: TransportState,
    persona: VerifiedPersona,
    pseudonym: dh::DhPublic,
    inbound: Reassembler,
    last_active: Instant,
}

/// Live sessions, keyed by the transport's peer id.
#[derive(Default)]
pub struct SessionTable {
    sessions: Mutex<HashMap<PeerId, Session>>,
}

impl SessionTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completed handshake.
    ///
    /// Replaces any existing session for that peer. A second handshake means
    /// the peer rebuilt — after a severed link, or a rotation — and the old
    /// keys are already useless to them; keeping the old entry would leave us
    /// encrypting to a session they have forgotten.
    pub fn open(&self, peer: &str, established: Established, now: Instant) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                peer.to_string(),
                Session {
                    keys: established.transport,
                    persona: established.persona,
                    pseudonym: established.remote_static,
                    inbound: Reassembler::new(),
                    last_active: now,
                },
            );
    }

    pub fn close(&self, peer: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(peer)
            .is_some()
    }

    pub fn is_open(&self, peer: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(peer)
    }

    pub fn peers(&self) -> Vec<PeerId> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// The persona proven during this session's handshake.
    pub fn persona(&self, peer: &str) -> Option<VerifiedPersona> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .map(|s| s.persona.clone())
    }

    /// The pseudonym proven during the handshake — what a block binds to.
    pub fn pseudonym(&self, peer: &str) -> Option<dh::DhPublic> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .map(|s| s.pseudonym)
    }

    /// Encrypt a frame, ready to hand to the transport.
    pub fn seal(&self, peer: &str, frame: &Frame, now: Instant) -> Result<Vec<u8>, SessionError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let session = sessions
            .get_mut(peer)
            .ok_or_else(|| SessionError::Unknown(peer.to_string()))?;

        let plain = frame.encode().map_err(SessionError::Frame)?;
        let mut buf = vec![0u8; plain.len() + AEAD_TAG_LEN];
        let n = session
            .keys
            .write_message(&plain, &mut buf)
            .map_err(|_| SessionError::Crypto)?;
        buf.truncate(n);
        session.last_active = now;
        frame::prefix(&buf).map_err(SessionError::Frame)
    }

    /// Feed bytes from the transport and take whatever whole frames emerge.
    ///
    /// An error here means the session is finished: the stream has no
    /// resynchronisation marker, so there is nothing to recover to, and a
    /// caller that kept going would be interpreting noise. The entry is dropped
    /// before returning so a caller cannot keep using it by mistake.
    pub fn open_frames(
        &self,
        peer: &str,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Vec<Frame>, SessionError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let session = sessions
            .get_mut(peer)
            .ok_or_else(|| SessionError::Unknown(peer.to_string()))?;

        // No separate reassembly budget: the 2-byte length prefix bounds an
        // incomplete message, and the loop below drains every complete one, so
        // a peer that dribbles for ever still holds under 64 KiB of ours. A
        // larger explicit budget was written here first and could never fire —
        // a guard that cannot trigger is not defence in depth, it is a comment
        // that looks like code.
        session.inbound.push(bytes);

        let mut frames = Vec::new();
        loop {
            let message = match session.inbound.next_message() {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    sessions.remove(peer);
                    return Err(SessionError::Frame(e));
                }
            };
            // Plaintext is the message minus its tag, so the message's own
            // length is always enough room.
            let mut buf = vec![0u8; message.len()];
            let n = match session.keys.read_message(&message, &mut buf) {
                Ok(n) => n,
                Err(_) => {
                    sessions.remove(peer);
                    return Err(SessionError::Crypto);
                }
            };
            match Frame::decode(&buf[..n]) {
                Ok(f) => frames.push(f),
                // A kind this build does not know is skipped, not fatal: a peer
                // on a newer build must not be able to end our session by
                // sending a frame type we have not heard of.
                Err(FrameError::UnknownKind(_)) => continue,
                Err(e) => {
                    sessions.remove(peer);
                    return Err(SessionError::Frame(e));
                }
            }
        }
        session.last_active = now;
        Ok(frames)
    }

    /// Drop sessions idle beyond [`IDLE_TIMEOUT`]. Returns those dropped, so a
    /// caller can tell the UI the thread is gone rather than leaving it looking
    /// live.
    pub fn sweep(&self, now: Instant) -> Vec<PeerId> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let expired: Vec<PeerId> = sessions
            .iter()
            // Saturating: `now` comes from the caller, and a clock that went
            // backwards should read as "not idle yet" rather than as an
            // arithmetic edge case. `duration_since` already saturates on
            // current Rust; saying so explicitly means the guarantee does not
            // rest on which std you happen to build against.
            .filter(|(_, s)| now.saturating_duration_since(s.last_active) >= IDLE_TIMEOUT)
            .map(|(p, _)| p.clone())
            .collect();
        for peer in &expired {
            sessions.remove(peer);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{self, Identity};
    use crate::session::frame::FrameKind;
    use crate::session::handshake::{Initiator, Responder};

    /// In-crate because it needs to seal a frame kind `Frame::encode` cannot
    /// produce. Writing it from `tests/session.rs` would have meant exposing a
    /// raw-seal path on the public API purely for a test — the same trade
    /// refused earlier for `DhSecret::expose_secret`.
    #[test]
    fn an_unknown_frame_kind_is_skipped_and_the_session_survives() {
        let alice = Identity::generate("Alice", 1);
        let bob = Identity::generate("Bob", 2);
        let bob_persona = identity::verify_persona_record(&bob.persona_record()).unwrap();
        let now = Instant::now();

        let (initiator, msg1) = Initiator::start(&alice, &bob_persona).unwrap();
        let (bob_side, msg2) = Responder::read_first(&bob, &msg1)
            .unwrap()
            .accept(&bob)
            .unwrap();
        let a = SessionTable::new();
        let b = SessionTable::new();
        a.open("bob", initiator.finish(&msg2).unwrap(), now);
        b.open("alice", bob_side, now);

        // A frame from a build that knows a kind this one does not.
        let mut wire = {
            let mut sessions = a.sessions.lock().unwrap();
            let session = sessions.get_mut("bob").unwrap();
            let plain = [99u8, 0, 1, 0xaa]; // kind 99, one payload byte
            let mut buf = vec![0u8; 128];
            let n = session.keys.write_message(&plain, &mut buf).unwrap();
            buf.truncate(n);
            frame::prefix(&buf).unwrap()
        };
        // Followed by one this build does understand.
        let known = Frame::new(FrameKind::Chat, b"still here".to_vec()).unwrap();
        wire.extend_from_slice(&a.seal("bob", &known, now).unwrap());

        let got = b.open_frames("alice", &wire, now).unwrap();
        assert_eq!(
            got,
            vec![known],
            "an unknown kind should be skipped, leaving the known frame"
        );
        assert!(
            b.is_open("alice"),
            "a peer on a newer build ended our session by naming a frame kind \
             we have not heard of"
        );
    }
}
