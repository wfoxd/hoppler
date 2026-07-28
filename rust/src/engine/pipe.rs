//! Pipe framing and demultiplexing (T10) — what actually rides a byte pipe.
//!
//! ```text
//!   [channel:1][len:2][payload]
//! ```
//!
//! # Why this exists
//!
//! A pipe carries two protocols: the discovery persona exchange and the
//! session plane (handshake, then encrypted frames). Before this, they shared
//! a raw stream with nothing to tell them apart, and the receiver had to guess
//! by inspection. That cannot be made to work:
//!
//! A discovery request is `[version:1][pseudonym:32]`, and *any* 32 bytes are a
//! structurally valid pseudonym — so the only real filter is a first byte of
//! `1`. Session ciphertext is uniformly distributed, so roughly one 33-byte
//! block in 256 parses as a valid request. When one did, the responder replied
//! with a persona record in plaintext, straight into the peer's session stream,
//! and the session died. Not a rare path: near-certain given enough traffic.
//!
//! The second half of the same gap: without a length, a reader cannot know
//! where a handshake message ends. The transport may split or merge freely
//! (T08 contract rule 3) and BLE certainly will, so handshakes that survived on
//! a boundary-preserving loopback rung would have broken on a radio.
//!
//! One byte of channel and two of length remove both problems. Nothing is
//! inferred from content.

/// The discovery persona exchange (T09).
pub const CHANNEL_DISCOVERY: u8 = 0;
/// The session plane: Noise handshake messages, then transport messages (T10).
pub const CHANNEL_SESSION: u8 = 1;

/// Header ahead of every payload.
pub const HEADER_LEN: usize = 3;

/// Largest payload a pipe frame may carry.
///
/// A Noise message tops out at 65535, which is also what the length field can
/// express — so this is the format's own ceiling rather than a policy choice.
pub const MAX_PAYLOAD: usize = u16::MAX as usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeFrame {
    pub channel: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PipeError {
    /// A payload too large for the length field.
    TooLarge,
    /// A frame that cannot be part of any legitimate stream.
    Malformed,
}

impl std::fmt::Display for PipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipeError::TooLarge => write!(f, "pipe payload exceeds {MAX_PAYLOAD} bytes"),
            PipeError::Malformed => write!(f, "malformed pipe frame"),
        }
    }
}

impl std::error::Error for PipeError {}

/// Wrap a payload for the pipe.
pub fn encode(channel: u8, payload: &[u8]) -> Result<Vec<u8>, PipeError> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return Err(PipeError::TooLarge);
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(channel);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Accumulates a peer's byte stream and yields whole frames.
///
/// Memory is bounded by the framing rather than a budget: an incomplete frame
/// is under 65538 bytes and complete ones are taken immediately, so a peer that
/// dribbles for ever holds under 64 KiB.
#[derive(Default)]
pub struct PipeReader {
    buf: Vec<u8>,
}

impl PipeReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, if one has arrived.
    ///
    /// `Err` means the stream is unusable. There is no resynchronisation marker
    /// to recover to, so a caller should drop the pipe rather than guess — a
    /// zero-length frame in particular would otherwise spin a caller's loop.
    pub fn next_frame(&mut self) -> Result<Option<PipeFrame>, PipeError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let channel = self.buf[0];
        let len = u16::from_be_bytes([self.buf[1], self.buf[2]]) as usize;
        if len == 0 {
            return Err(PipeError::Malformed);
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(PipeFrame { channel, payload }))
    }

    /// Bytes held pending more input.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_on_either_channel() {
        for channel in [CHANNEL_DISCOVERY, CHANNEL_SESSION] {
            let mut r = PipeReader::new();
            r.push(&encode(channel, b"payload").unwrap());
            assert_eq!(
                r.next_frame().unwrap(),
                Some(PipeFrame {
                    channel,
                    payload: b"payload".to_vec()
                })
            );
        }
    }

    #[test]
    fn channels_stay_separate_however_the_stream_is_chopped() {
        // The whole point: a reader must never have to infer a channel from
        // content. Interleaved traffic on one pipe, at every chunk size.
        let stream: Vec<u8> = [
            encode(CHANNEL_SESSION, &[0xaa; 40]).unwrap(),
            encode(CHANNEL_DISCOVERY, &[0x01; 33]).unwrap(),
            encode(CHANNEL_SESSION, &[0xbb; 5]).unwrap(),
        ]
        .concat();

        for chunk in [1usize, 2, 7, 33, 4096] {
            let mut r = PipeReader::new();
            let mut got = Vec::new();
            for piece in stream.chunks(chunk) {
                r.push(piece);
                while let Some(f) = r.next_frame().unwrap() {
                    got.push(f);
                }
            }
            let channels: Vec<u8> = got.iter().map(|f| f.channel).collect();
            assert_eq!(
                channels,
                vec![CHANNEL_SESSION, CHANNEL_DISCOVERY, CHANNEL_SESSION],
                "chunk size {chunk}"
            );
            assert_eq!(got[1].payload, vec![0x01; 33]);
            assert_eq!(r.buffered(), 0, "chunk size {chunk}: leftovers");
        }
    }

    /// The exact confusion this framing exists to prevent: session ciphertext
    /// that happens to begin with the discovery version byte.
    #[test]
    fn ciphertext_shaped_like_a_discovery_request_stays_on_its_own_channel() {
        let mut looks_like_a_request = vec![1u8]; // discovery VERSION
        looks_like_a_request.extend_from_slice(&[0x42; 32]);

        let mut r = PipeReader::new();
        r.push(&encode(CHANNEL_SESSION, &looks_like_a_request).unwrap());
        let frame = r.next_frame().unwrap().unwrap();
        assert_eq!(
            frame.channel, CHANNEL_SESSION,
            "content that parses as a discovery request was routed by its \
             bytes rather than by its channel"
        );
    }

    #[test]
    fn a_zero_length_frame_is_an_error_not_an_infinite_loop() {
        let mut r = PipeReader::new();
        r.push(&[CHANNEL_SESSION, 0, 0, 9]);
        assert_eq!(r.next_frame(), Err(PipeError::Malformed));
    }

    #[test]
    fn an_empty_payload_cannot_be_encoded() {
        assert_eq!(encode(CHANNEL_SESSION, &[]), Err(PipeError::TooLarge));
    }

    #[test]
    fn a_dribbling_peer_stays_under_the_framing_ceiling() {
        const CEILING: usize = HEADER_LEN + MAX_PAYLOAD;
        let mut r = PipeReader::new();
        r.push(&[CHANNEL_SESSION, 0xff, 0xff]);
        let mut completed = false;
        for _ in 0..200 {
            r.push(&[0u8; 1024]);
            while r.next_frame().unwrap().is_some() {
                completed = true;
            }
            assert!(r.buffered() < CEILING, "held {} bytes", r.buffered());
            if completed {
                // Stop at the frame boundary: pushing on would run into the
                // *next* frame's header, which is a different test.
                break;
            }
        }
        assert!(
            completed,
            "the frame never completed, so nothing was bounded"
        );
    }
}
