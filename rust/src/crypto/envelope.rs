//! The versioned wire envelope (tech spec §2).
//!
//! Wire format: `[1 version byte][protobuf Envelope]`. The version byte lives
//! outside the protobuf so an unknown version is rejected before any parsing.

use prost::Message;

use super::CryptoError;
use crate::proto::v0::Envelope;

/// The only wire version this build speaks.
pub const WIRE_VERSION_V0: u8 = 0x00;

/// Hard cap on a single envelope's wire size. Envelopes carry control-plane
/// frames (Ping, Chat, transfer negotiation) — all small; bulk data never
/// travels as one envelope. This bounds decode memory to the actual input
/// (prost already rejects a length field that exceeds the buffer, so this is
/// defense-in-depth for genuinely large input). A transport MAY impose a
/// lower cap; it must never need a higher one at this ring.
pub const MAX_WIRE_LEN: usize = 1 << 20; // 1 MiB

/// A parsed envelope.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodedEnvelope {
    pub frame_type: u32,
    pub payload: Vec<u8>,
}

/// Assemble the wire bytes for a frame.
pub fn encode(frame_type: u32, payload: &[u8]) -> Vec<u8> {
    let body = Envelope {
        frame_type,
        payload: payload.to_vec(),
    };
    let mut wire = Vec::with_capacity(1 + body.encoded_len());
    wire.push(WIRE_VERSION_V0);
    body.encode(&mut wire)
        .expect("encoding into a Vec cannot fail");
    wire
}

/// Parse wire bytes. Strict: empty input, unknown version byte, and malformed
/// protobuf are all distinct, non-panicking failures.
pub fn decode(wire: &[u8]) -> Result<DecodedEnvelope, CryptoError> {
    if wire.len() > MAX_WIRE_LEN {
        return Err(CryptoError::MalformedEnvelope);
    }
    let (&version, body) = wire.split_first().ok_or(CryptoError::EmptyWire)?;
    if version != WIRE_VERSION_V0 {
        return Err(CryptoError::UnsupportedWireVersion(version));
    }
    let envelope = Envelope::decode(body).map_err(|_| CryptoError::MalformedEnvelope)?;
    Ok(DecodedEnvelope {
        frame_type: envelope.frame_type,
        payload: envelope.payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let wire = encode(7, b"ping");
        assert_eq!(wire[0], WIRE_VERSION_V0);
        let decoded = decode(&wire).unwrap();
        assert_eq!(decoded.frame_type, 7);
        assert_eq!(decoded.payload, b"ping");
    }

    #[test]
    fn golden_wire_bytes() {
        // Pins the exact on-wire layout so a field-number or version-byte
        // regression is caught here rather than as a cross-language break with
        // the generated Dart side. Layout: [version][field1 tag=0x08][varint 7]
        // [field2 tag=0x12][len 0x04]["ping"].
        assert_eq!(
            encode(7, b"ping"),
            [0x00, 0x08, 0x07, 0x12, 0x04, b'p', b'i', b'n', b'g']
        );
    }

    #[test]
    fn empty_wire_rejected() {
        assert_eq!(decode(&[]), Err(CryptoError::EmptyWire));
    }

    #[test]
    fn oversized_wire_rejected() {
        let too_big = vec![WIRE_VERSION_V0; MAX_WIRE_LEN + 1];
        assert_eq!(decode(&too_big), Err(CryptoError::MalformedEnvelope));
        // A frame right at the cap is still parsed (not rejected on size).
        let at_cap = encode(1, &vec![0u8; MAX_WIRE_LEN - 16]);
        assert!(at_cap.len() <= MAX_WIRE_LEN && decode(&at_cap).is_ok());
    }

    #[test]
    fn unknown_version_rejected_before_parsing() {
        let mut wire = encode(1, b"x");
        wire[0] = 0x01;
        assert_eq!(
            decode(&wire),
            Err(CryptoError::UnsupportedWireVersion(0x01))
        );
        wire[0] = 0xff;
        assert_eq!(
            decode(&wire),
            Err(CryptoError::UnsupportedWireVersion(0xff))
        );
    }

    #[test]
    fn version_byte_alone_is_an_empty_envelope() {
        // A bare version byte parses as the default (empty) message — legal protobuf.
        let decoded = decode(&[WIRE_VERSION_V0]).unwrap();
        assert_eq!(decoded.frame_type, 0);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn garbage_body_rejected() {
        // 0xff opens a field with the maximum tag — invalid as a lone byte sequence.
        assert_eq!(
            decode(&[WIRE_VERSION_V0, 0xff, 0xff, 0xff]),
            Err(CryptoError::MalformedEnvelope)
        );
    }
}
