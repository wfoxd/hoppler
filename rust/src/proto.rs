//! Generated protobuf types (see ../proto/, built by build.rs via prost).

pub mod v0 {
    include!(concat!(env!("OUT_DIR"), "/hoppler.v0.rs"));
}

#[cfg(test)]
mod tests {
    use super::v0::PingFrame;
    use prost::Message;

    // Golden bytes shared with the Dart test (test/proto_test.dart):
    // field 1, varint 42 -> [0x08, 0x2A]
    #[test]
    fn ping_frame_round_trip_matches_golden_bytes() {
        let frame = PingFrame { nonce: 42 };
        let bytes = frame.encode_to_vec();
        assert_eq!(bytes, vec![0x08, 0x2A]);

        let decoded = PingFrame::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.nonce, 42);
    }
}
