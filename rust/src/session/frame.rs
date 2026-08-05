//! In-session framing (T10) — what rides an established Noise session.
//!
//! ```text
//!   plaintext frame   [kind:1][len:2][payload]
//!   on the wire       Noise transport message, length-prefixed for the pipe
//! ```
//!
//! Two layers of length, for two different reasons. The inner `len` is the
//! frame's own, so one Noise message can carry exactly one frame and a short
//! read is detectable. The outer prefix exists because the transport does not
//! preserve message boundaries (T08 contract rule 3) — without it a reader
//! cannot tell where one ciphertext ends and the next begins, and a Noise
//! message fed a byte short fails its tag rather than waiting for more.
//!
//! # Multiplexing
//!
//! One session carries Ping, Chat and Drop control. The `kind` byte is what
//! lets a reader route without decrypting speculatively — and it is inside the
//! ciphertext, so an observer cannot tell a Drop offer from a chat line by
//! watching frame types go past.

/// What a frame carries. One session multiplexes all of them.
///
/// `#[repr(u8)]` because these discriminants *are* the wire: [`Frame::encode`]
/// writes one with `as u8`, which truncates silently. A future variant numbered
/// past 255 would otherwise compile, go out as some other kind's byte, and be
/// decoded by the peer as that kind. The repr turns that into a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// R0-F3. A nudge.
    Ping = 1,
    /// R0-F5. A chat line.
    Chat = 2,
    /// R0-F6 control plane — offers, accepts, progress. Bulk bytes ride a
    /// separate rung (T15/T16), not this session.
    DropControl = 3,
    /// R0-F3. The answer to a [`FrameKind::Ping`], and the only thing that
    /// makes one delivered rather than merely sent.
    ///
    /// A separate kind rather than a Ping sent back: two devices answering each
    /// other with the same frame is a loop with no idle state, and a receiver
    /// cannot tell the nudge from the answer. A Pong is never answered.
    Pong = 4,
}

impl FrameKind {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(FrameKind::Ping),
            2 => Some(FrameKind::Chat),
            3 => Some(FrameKind::DropControl),
            4 => Some(FrameKind::Pong),
            _ => None,
        }
    }
}

/// Largest single frame payload.
///
/// Sized well under Noise's 65535-byte message ceiling, leaving room for the
/// 3-byte header and the 16-byte AEAD tag. Bulk transfer does not come through
/// here — a Drop's bytes ride the fast rung — so this bounds control traffic,
/// where a large frame is a bug or an attack rather than a legitimate need.
pub const MAX_FRAME_PAYLOAD: usize = 16 * 1024;

/// Header bytes ahead of the payload.
pub const HEADER_LEN: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Longer than [`MAX_FRAME_PAYLOAD`].
    TooLarge,
    /// Not a frame we can parse: bad length, unknown kind, trailing bytes.
    Malformed,
    /// A kind this build does not know. Distinct from `Malformed` so a caller
    /// can ignore it and keep the session, which is what forward compatibility
    /// requires — a peer on a newer build must not tear our session down.
    UnknownKind(u8),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooLarge => write!(f, "frame payload exceeds {MAX_FRAME_PAYLOAD} bytes"),
            FrameError::Malformed => write!(f, "malformed session frame"),
            FrameError::UnknownKind(k) => write!(f, "unknown frame kind {k}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    pub fn new(kind: FrameKind, payload: impl Into<Vec<u8>>) -> Result<Self, FrameError> {
        let payload = payload.into();
        if payload.len() > MAX_FRAME_PAYLOAD {
            return Err(FrameError::TooLarge);
        }
        Ok(Self { kind, payload })
    }

    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.payload.len() > MAX_FRAME_PAYLOAD {
            return Err(FrameError::TooLarge);
        }
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(self.kind as u8);
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decode exactly one frame from a decrypted Noise message.
    ///
    /// Trailing bytes are an error rather than ignored: one Noise message
    /// carries one frame, so anything left over means the sender and this
    /// reader disagree about the format, and continuing on a guess is how a
    /// desync becomes silent corruption.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() < HEADER_LEN {
            return Err(FrameError::Malformed);
        }
        let len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if len > MAX_FRAME_PAYLOAD {
            return Err(FrameError::TooLarge);
        }
        if bytes.len() != HEADER_LEN + len {
            return Err(FrameError::Malformed);
        }
        let kind = FrameKind::from_byte(bytes[0]).ok_or(FrameError::UnknownKind(bytes[0]))?;
        Ok(Self {
            kind,
            payload: bytes[HEADER_LEN..].to_vec(),
        })
    }
}

/// Split a byte stream into whole Noise messages.
///
/// The transport merges and splits freely, so a reader accumulates and asks
/// this what is complete. Returns `None` while a message is still arriving.
pub struct Reassembler {
    buf: Vec<u8>,
}

/// Length prefix ahead of each Noise message on the pipe.
pub const LENGTH_PREFIX: usize = 2;

impl Default for Reassembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Reassembler {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete Noise message, if one has fully arrived.
    ///
    /// `Err` means the stream is unusable — a declared length beyond what any
    /// legitimate sender produces. The caller should drop the session rather
    /// than resynchronise, because there is no framing marker to resynchronise
    /// *to*, and guessing turns a corrupt stream into a plausible one.
    pub fn next_message(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buf.len() < LENGTH_PREFIX {
            return Ok(None);
        }
        let len = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
        if len == 0 {
            return Err(FrameError::Malformed);
        }
        if self.buf.len() < LENGTH_PREFIX + len {
            return Ok(None);
        }
        let message = self.buf[LENGTH_PREFIX..LENGTH_PREFIX + len].to_vec();
        self.buf.drain(..LENGTH_PREFIX + len);
        Ok(Some(message))
    }

    /// Bytes held pending more input. Exposed so a caller can bound how much a
    /// peer may make it hold.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// Add the pipe-level length prefix to a Noise message.
pub fn prefix(message: &[u8]) -> Result<Vec<u8>, FrameError> {
    if message.is_empty() || message.len() > u16::MAX as usize {
        return Err(FrameError::TooLarge);
    }
    let mut out = Vec::with_capacity(LENGTH_PREFIX + message.len());
    out.extend_from_slice(&(message.len() as u16).to_be_bytes());
    out.extend_from_slice(message);
    Ok(out)
}
