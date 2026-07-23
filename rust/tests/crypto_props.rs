//! Property tests and the envelope fuzz-lite pass (T04: "fuzz target for
//! envelope parsing runs in CI"). Proptest generates adversarial inputs and
//! treats any panic as a failure, so `decode` is exercised as a parser fuzz
//! within the normal `cargo test` budget.

use proptest::prelude::*;
use rust_lib_hoppler::crypto::{aead, dh, envelope, hash, kdf, sign, CryptoError};

proptest! {
    // ── Envelope: the fuzz surface ─────────────────────────────────────────

    #[test]
    fn envelope_decode_never_panics(wire in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = envelope::decode(&wire); // Ok or Err — never a panic
    }

    #[test]
    fn envelope_round_trips(frame_type in any::<u32>(), payload in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let wire = envelope::encode(frame_type, &payload);
        let decoded = envelope::decode(&wire).unwrap();
        prop_assert_eq!(decoded.frame_type, frame_type);
        prop_assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn envelope_nonzero_version_always_rejected(version in 1u8.., body in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut wire = vec![version];
        wire.extend(body);
        prop_assert_eq!(envelope::decode(&wire).unwrap_err(), CryptoError::UnsupportedWireVersion(version));
    }

    // ── Signatures ─────────────────────────────────────────────────────────

    #[test]
    fn sign_verify_round_trips(seed in any::<[u8; 32]>(), msg in proptest::collection::vec(any::<u8>(), 0..512)) {
        let kp = sign::SigningKeyPair::from_seed(&seed);
        let sig = kp.sign(&msg);
        prop_assert!(sign::verify(&kp.public(), &msg, &sig).is_ok());
    }

    #[test]
    fn signature_bound_to_message(seed in any::<[u8; 32]>(), msg in proptest::collection::vec(any::<u8>(), 1..512), flip in any::<usize>()) {
        let kp = sign::SigningKeyPair::from_seed(&seed);
        let sig = kp.sign(&msg);
        let mut other = msg.clone();
        let i = flip % other.len();
        other[i] ^= 0x01;
        prop_assert!(sign::verify(&kp.public(), &other, &sig).is_err());
    }

    // ── DH ─────────────────────────────────────────────────────────────────

    #[test]
    fn dh_commutes(a in any::<[u8; 32]>(), b in any::<[u8; 32]>()) {
        let sa = dh::DhSecret::from_bytes(&a);
        let sb = dh::DhSecret::from_bytes(&b);
        let ab = sa.diffie_hellman(&sb.public());
        let ba = sb.diffie_hellman(&sa.public());
        prop_assert_eq!(ab.as_bytes(), ba.as_bytes());
    }

    // ── AEAD ───────────────────────────────────────────────────────────────

    #[test]
    fn aead_round_trips(key in any::<[u8; 32]>(), nonce in any::<[u8; 24]>(), aad in proptest::collection::vec(any::<u8>(), 0..64), pt in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let key = aead::AeadKey::from_bytes(key);
        let sealed = aead::seal(&key, &nonce, &aad, &pt);
        prop_assert_eq!(sealed.len(), pt.len() + aead::TAG_LEN);
        let opened = aead::open(&key, &nonce, &aad, &sealed).unwrap();
        prop_assert_eq!(&*opened, &pt);
    }

    #[test]
    fn aead_single_bit_flip_fails(key in any::<[u8; 32]>(), nonce in any::<[u8; 24]>(), pt in proptest::collection::vec(any::<u8>(), 0..256), flip in any::<usize>()) {
        let key = aead::AeadKey::from_bytes(key);
        let mut sealed = aead::seal(&key, &nonce, b"", &pt);
        let i = flip % sealed.len();
        sealed[i] ^= 0x01;
        prop_assert!(aead::open(&key, &nonce, b"", &sealed).is_err());
    }

    // ── KDF / hash determinism ─────────────────────────────────────────────

    #[test]
    fn kdf_deterministic_and_input_sensitive(ikm in proptest::collection::vec(any::<u8>(), 1..64), salt in proptest::collection::vec(any::<u8>(), 0..32)) {
        let a = kdf::derive_32(&ikm, &salt, b"hoppler/prop/v1");
        let b = kdf::derive_32(&ikm, &salt, b"hoppler/prop/v1");
        prop_assert_eq!(*a, *b);
        let c = kdf::derive_32(&ikm, &salt, b"hoppler/prop/v2");
        prop_assert_ne!(*a, *c);
    }

    #[test]
    fn hash_deterministic(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        prop_assert_eq!(hash::hash(&data), hash::hash(&data));
    }
}
