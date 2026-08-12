//! The pairing invite — what the QR code shows and the NFC tap carries
//! (tech spec §6, R0-F4).
//!
//! # What it is for, and what it is not
//!
//! An invite is an **address plus a nonce**. It says which Layer-2 key to open
//! a ceremony toward, roughly where to find it, and which ceremony this is. It
//! is not a credential and proves nothing on its own: it is unsigned, and
//! signing it would not help. A signature would show the shower holds `l2_pub`,
//! which the ceremony's handshake proves anyway; and an attacker who replaces
//! the whole code replaces `l2_pub` with their own and signs it perfectly well.
//!
//! What actually authenticates the pairing is the physical channel — a person
//! holding a screen up in front of another person — and the SAS both humans
//! check at the end ([`super::sas`]). This type carries no security claim.
//!
//! # Privacy cost, stated plainly
//!
//! A displayed invite pairs a Layer-2 key with the rung id the device is
//! advertising under *at that moment*. Anyone who photographs the screen can
//! link the two until the next rotation, which R0-F2 otherwise prevents. That
//! is inherent to showing a code to a person in the room, it is bounded by the
//! rotation period, and it is a deliberate trade — but it is not nothing, and
//! it is the reason an invite should be shown for as long as it takes to scan
//! and no longer.
//!
//! Note where the bound comes from: it is the *shower's* own state that has to
//! expire, not anything in these bytes. An expiry carried inside the payload
//! would be a claim the reader cannot check — the scanner has no clock in
//! common with the shower and no reason to trust one it is handed — so this
//! type has no timestamp field. Freshness is the ceremony's job, and it holds
//! the nonce locally to do it.
//!
//! # Why this text encoding
//!
//! The wire form is `HOPPLER://PAIR/<BASE32>`, and every part of that is
//! chosen for the scanner:
//!
//! - **Uppercase, base32, no padding.** QR's alphanumeric mode covers `A–Z`,
//!   `0–9` and a handful of punctuation including `:` and `/`, and stores
//!   5.5 bits per character against byte mode's 8. The RFC 4648 base32 alphabet
//!   (`A–Z`, `2–7`) sits entirely inside it, and so does an uppercased URI
//!   scheme — schemes are case-insensitive (RFC 3986 §3.1), so this is a normal
//!   URI written in the case that keeps the whole string in the denser mode.
//!   The `=` padding character is *not* in that set, which is the second reason
//!   there is none. Reading accepts either case; only writing is uppercase.
//! - **A URI, not bare base32.** It gives the scanner an unmistakable prefix to
//!   reject a stray QR code on sight, it is a valid NDEF URI record for the NFC
//!   path, and `hoppler://pair/…` is an ordinary Android intent filter.
//! - **The version byte is inside the base32, not in the prefix.** Same
//!   discipline as [`crate::crypto::envelope`]: one byte ahead of the protobuf,
//!   so an unknown version is refused before anything is parsed. It costs one
//!   base32 decode to reach, which is cheaper than the alternative of a scheme
//!   per version.

use prost::Message;

use crate::crypto::{rng, sign};
use crate::proto::v0::PairingInvite;

/// The scheme and path an invite is written under. Uppercase for QR's
/// alphanumeric mode; parsing is case-insensitive.
pub const URI_PREFIX: &str = "HOPPLER://PAIR/";

/// Payload version, ahead of the protobuf bytes.
pub const INVITE_VERSION: u8 = 0x01;

/// Length of the ceremony nonce. Full 32 bytes rather than something shorter
/// that would still be "enough": it is the handshake prologue, so its only cost
/// is roughly 52 characters of QR, and there is no reason to shave a binding.
pub const CEREMONY_NONCE_LEN: usize = 32;

/// Cap on the rung hint, in bytes. It holds a node id (16 hex characters
/// today); this is slack, and mostly a bound on what a hostile code can make us
/// carry around.
pub const MAX_BLE_HINT_LEN: usize = 64;

/// Cap on an invite URI we will even attempt to parse. A camera decodes
/// whatever is in front of it, so this is untrusted input arriving at the size
/// the attacker picks.
pub const MAX_INVITE_URI_LEN: usize = 1024;

/// Why an invite could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// The text is not a Hoppler pairing URI at all — the common case, since a
    /// camera sees every QR code in front of it and most are not ours.
    NotAnInvite,
    /// It is one of ours, but from a version this build does not speak.
    UnsupportedVersion(u8),
    /// Longer than [`MAX_INVITE_URI_LEN`].
    TooLong,
    /// The prefix matched but the rest did not survive decoding: bad base32, a
    /// protobuf that would not parse, or a field of the wrong length.
    Malformed,
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InviteError::NotAnInvite => write!(f, "not a Hoppler pairing code"),
            InviteError::UnsupportedVersion(v) => {
                write!(f, "pairing code version {v:#04x} is newer than this app")
            }
            InviteError::TooLong => write!(f, "pairing code too long"),
            InviteError::Malformed => write!(f, "malformed pairing code"),
        }
    }
}

impl std::error::Error for InviteError {}

/// A pairing invite, either one we are showing or one we have just read.
///
/// Deliberately not `Copy` and deliberately owning its hint: an invite outlives
/// the scan that produced it, and the ceremony holds one while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The Layer-2 key the ceremony is opened toward.
    pub l2_pub: sign::PublicKey,
    /// Where the shower was advertising when the code was made. A hint only —
    /// it rotates, so it goes stale, and a scanner that cannot reach it falls
    /// back to Discovery rather than failing.
    pub ble_hint: String,
    /// Binds one ceremony to one code.
    pub nonce: [u8; CEREMONY_NONCE_LEN],
}

impl Invite {
    /// Mint a fresh invite to show. The nonce comes from the OS CSPRNG and is
    /// new every call — a code shown twice is two ceremonies, never one
    /// replayed.
    pub fn fresh(l2_pub: sign::PublicKey, ble_hint: impl Into<String>) -> Self {
        Self {
            l2_pub,
            ble_hint: truncate_hint(ble_hint.into()),
            nonce: rng::random_array::<CEREMONY_NONCE_LEN>(),
        }
    }

    /// Build from known parts, for a caller supplying its own nonce (the
    /// ceremony, which must keep it to check the handshake against).
    pub fn from_parts(
        l2_pub: sign::PublicKey,
        ble_hint: impl Into<String>,
        nonce: [u8; CEREMONY_NONCE_LEN],
    ) -> Self {
        Self {
            l2_pub,
            ble_hint: truncate_hint(ble_hint.into()),
            nonce,
        }
    }

    /// The text to put in a QR code or an NDEF URI record.
    pub fn to_uri(&self) -> String {
        let payload = PairingInvite {
            l2_pub: self.l2_pub.0.to_vec(),
            ble_hint: self.ble_hint.clone(),
            ceremony_nonce: self.nonce.to_vec(),
        };
        let mut bytes = Vec::with_capacity(1 + payload.encoded_len());
        bytes.push(INVITE_VERSION);
        payload
            .encode(&mut bytes)
            .expect("encoding into a Vec cannot fail");
        let mut uri = String::with_capacity(URI_PREFIX.len() + bytes.len().div_ceil(5) * 8);
        uri.push_str(URI_PREFIX);
        base32_encode_into(&bytes, &mut uri);
        uri
    }

    /// Read whatever the camera decoded.
    ///
    /// Every failure mode here is reachable from a stranger holding up a
    /// screen, so the order matters: length, then prefix, then version, then
    /// content. Nothing allocates on the strength of a field the code itself
    /// declared.
    pub fn parse(uri: &str) -> Result<Self, InviteError> {
        if uri.len() > MAX_INVITE_URI_LEN {
            return Err(InviteError::TooLong);
        }
        let body = strip_prefix_ignore_case(uri, URI_PREFIX).ok_or(InviteError::NotAnInvite)?;
        let bytes = base32_decode(body).ok_or(InviteError::Malformed)?;
        let (version, rest) = bytes.split_first().ok_or(InviteError::Malformed)?;
        if *version != INVITE_VERSION {
            return Err(InviteError::UnsupportedVersion(*version));
        }

        let payload = PairingInvite::decode(rest).map_err(|_| InviteError::Malformed)?;
        if payload.ble_hint.len() > MAX_BLE_HINT_LEN {
            return Err(InviteError::Malformed);
        }
        let l2_bytes: [u8; sign::PUBLIC_KEY_LEN] = payload
            .l2_pub
            .as_slice()
            .try_into()
            .map_err(|_| InviteError::Malformed)?;
        let nonce: [u8; CEREMONY_NONCE_LEN] = payload
            .ceremony_nonce
            .as_slice()
            .try_into()
            .map_err(|_| InviteError::Malformed)?;

        Ok(Self {
            l2_pub: sign::PublicKey(l2_bytes),
            ble_hint: payload.ble_hint,
            nonce,
        })
    }
}

/// Keep a locally-made hint inside the bound the reader enforces, so an invite
/// we write always parses. Same discipline as `identity::normalise_persona`:
/// the alternative is a code that fails on our own phone, in the ceremony, in
/// front of another person.
fn truncate_hint(mut hint: String) -> String {
    if hint.len() > MAX_BLE_HINT_LEN {
        let mut end = MAX_BLE_HINT_LEN;
        while !hint.is_char_boundary(end) {
            end -= 1;
        }
        hint.truncate(end);
    }
    hint
}

/// `str::strip_prefix`, but for a URI scheme, which is case-insensitive.
///
/// Only the prefix. The base32 body is uppercased by the decoder itself, so a
/// lowercase code from another implementation reads fine either way.
fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

// ── base32 (RFC 4648, unpadded) ──────────────────────────────────────────────
//
// Hand-rolled rather than pulled in, at about thirty lines: the alphabet is
// fixed, the only caller is here, and the one property that matters — that a
// given byte string has exactly one spelling — is easier to *test* than to
// confirm a dependency provides.

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode_into(input: &[u8], out: &mut String) {
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in input {
        buffer = ((buffer << 8) | byte as u32) & 0xffff;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    // A trailing partial group is left-aligned and zero-filled, which is what
    // makes the canonicality check on the way back in possible.
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
}

/// Decode, refusing anything that is not the *one* spelling of some byte
/// string.
///
/// The strictness is the point. Base32 leaves spare bits at the end of a
/// partial group, and a decoder that ignores them accepts many distinct codes
/// that all mean the same invite. Here that would be a nuisance rather than a
/// break — the invite carries no security claim — but the same bytes go on to
/// become a handshake prologue, and "two spellings of one ceremony" is not a
/// property worth having lying around. Rejected: a stray extra character
/// (`bits >= 5`, a whole character's worth of nothing), and non-zero padding
/// bits.
fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        let value = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buffer = ((buffer << 5) | value) & 0xffff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    if bits >= 5 || (buffer & ((1u32 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn an_invite() -> Invite {
        Invite::from_parts(
            sign::PublicKey([7u8; sign::PUBLIC_KEY_LEN]),
            "a1b2c3d4e5f60718",
            [9u8; CEREMONY_NONCE_LEN],
        )
    }

    #[test]
    fn round_trips() {
        let invite = an_invite();
        assert_eq!(Invite::parse(&invite.to_uri()), Ok(invite));
    }

    /// Freezes the bytes, which the round trip above cannot.
    ///
    /// A round trip only proves this build agrees with itself. Renumber a
    /// protobuf field, reorder the base32 alphabet, move the version byte
    /// behind the payload — every one of those still round-trips perfectly, and
    /// every one of them is a code that the phone in the other person's hand
    /// cannot read. The two devices in a ceremony are not required to be running
    /// the same build, so self-consistency is the one property that is worth
    /// nothing here.
    ///
    /// Deliberately written out in full rather than as a hash: the failure this
    /// catches is a *format* change, and a reviewer looking at the diff should
    /// be able to see what the format now is. The sibling vector for the SAS is
    /// `sas::tests::golden_vector`.
    #[test]
    fn golden_uri() {
        assert_eq!(
            an_invite().to_uri(),
            "HOPPLER://PAIR/AEFCABYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYHA4DQOBYH\
             CIIGCMLCGJRTGZBUMU2WMNRQG4YTQGRABEEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQ"
        );
    }

    #[test]
    fn fresh_invites_do_not_repeat_a_nonce() {
        let key = sign::PublicKey([1u8; sign::PUBLIC_KEY_LEN]);
        let a = Invite::fresh(key, "hint");
        let b = Invite::fresh(key, "hint");
        assert_ne!(a.nonce, b.nonce, "two codes shared a ceremony nonce");
    }

    /// The whole string must stay inside QR's alphanumeric character set, or
    /// the density argument in the module header is not true of what we emit.
    #[test]
    fn the_uri_stays_in_qr_alphanumeric_mode() {
        const QR_ALPHANUMERIC: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
        let uri = an_invite().to_uri();
        for c in uri.chars() {
            assert!(
                QR_ALPHANUMERIC.contains(c),
                "{c:?} forces the QR code into byte mode"
            );
        }
    }

    #[test]
    fn a_lowercase_code_still_reads() {
        let invite = an_invite();
        assert_eq!(Invite::parse(&invite.to_uri().to_lowercase()), Ok(invite));
    }

    #[test]
    fn a_foreign_qr_code_is_not_an_invite() {
        for text in [
            "",
            "https://example.com/",
            "HOPPLER://",
            "HOPPLER://PING/AAAA",
            "WIFI:S:somewhere;T:WPA;P:hunter2;;",
        ] {
            assert_eq!(
                Invite::parse(text),
                Err(InviteError::NotAnInvite),
                "{text:?} was read as a pairing code"
            );
        }
    }

    #[test]
    fn a_newer_version_is_named_rather_than_guessed_at() {
        let mut bytes = vec![INVITE_VERSION + 1];
        bytes.extend_from_slice(&[0u8; 8]);
        let mut uri = URI_PREFIX.to_string();
        base32_encode_into(&bytes, &mut uri);
        assert_eq!(
            Invite::parse(&uri),
            Err(InviteError::UnsupportedVersion(INVITE_VERSION + 1))
        );
    }

    #[test]
    fn an_oversized_code_is_refused_before_anything_is_decoded() {
        let uri = format!("{URI_PREFIX}{}", "A".repeat(MAX_INVITE_URI_LEN));
        assert_eq!(Invite::parse(&uri), Err(InviteError::TooLong));
    }

    #[test]
    fn wrong_length_keys_and_nonces_are_refused() {
        // A payload that parses as protobuf but whose fields are the wrong
        // size — the shape a truncating or padding implementation produces, and
        // one that would otherwise fail much later inside a handshake.
        for (l2_len, nonce_len) in [(31, 32), (33, 32), (32, 31), (32, 0), (0, 32)] {
            let payload = PairingInvite {
                l2_pub: vec![0u8; l2_len],
                ble_hint: "x".into(),
                ceremony_nonce: vec![0u8; nonce_len],
            };
            let mut bytes = vec![INVITE_VERSION];
            payload.encode(&mut bytes).unwrap();
            let mut uri = URI_PREFIX.to_string();
            base32_encode_into(&bytes, &mut uri);
            assert_eq!(
                Invite::parse(&uri),
                Err(InviteError::Malformed),
                "accepted an invite with a {l2_len}-byte key and a {nonce_len}-byte nonce"
            );
        }
    }

    #[test]
    fn an_overlong_hint_from_a_peer_is_refused() {
        let payload = PairingInvite {
            l2_pub: vec![0u8; sign::PUBLIC_KEY_LEN],
            ble_hint: "h".repeat(MAX_BLE_HINT_LEN + 1),
            ceremony_nonce: vec![0u8; CEREMONY_NONCE_LEN],
        };
        let mut bytes = vec![INVITE_VERSION];
        payload.encode(&mut bytes).unwrap();
        let mut uri = URI_PREFIX.to_string();
        base32_encode_into(&bytes, &mut uri);
        assert_eq!(Invite::parse(&uri), Err(InviteError::Malformed));
    }

    #[test]
    fn our_own_hint_is_normalised_so_our_own_code_always_reads() {
        // The mirror of the test above: whatever the caller hands us, the code
        // we display must survive our own parser.
        let invite = Invite::from_parts(
            sign::PublicKey([1u8; sign::PUBLIC_KEY_LEN]),
            "é".repeat(MAX_BLE_HINT_LEN),
            [0u8; CEREMONY_NONCE_LEN],
        );
        assert!(invite.ble_hint.len() <= MAX_BLE_HINT_LEN);
        assert!(invite.ble_hint.chars().all(|c| c == 'é'));
        assert_eq!(Invite::parse(&invite.to_uri()), Ok(invite));
    }

    #[test]
    fn a_truncated_code_does_not_parse_as_a_shorter_one() {
        let uri = an_invite().to_uri();
        for cut in URI_PREFIX.len()..uri.len() {
            assert!(
                Invite::parse(&uri[..cut]).is_err(),
                "a code cut at {cut} characters parsed"
            );
        }
    }

    #[test]
    fn base32_matches_rfc4648_vectors() {
        // RFC 4648 §10, with the padding dropped — this is the unpadded form.
        for (input, expected) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            let mut out = String::new();
            base32_encode_into(input.as_bytes(), &mut out);
            assert_eq!(out, expected, "encoding {input:?}");
            assert_eq!(
                base32_decode(expected).as_deref(),
                Some(input.as_bytes()),
                "decoding {expected:?}"
            );
        }
    }

    #[test]
    fn base32_refuses_a_second_spelling_of_the_same_bytes() {
        // "MZXQ" is "fo". "MZXR" differs only in bits that fell off the end,
        // so a lax decoder yields "fo" for both and one ceremony has two codes.
        assert_eq!(base32_decode("MZXQ").as_deref(), Some(&b"fo"[..]));
        assert_eq!(
            base32_decode("MZXR"),
            None,
            "accepted non-zero padding bits"
        );
        // A whole spare character encodes nothing at all.
        assert_eq!(base32_decode("MZXW6A"), None);
        // And the padding character itself is not in the alphabet.
        assert_eq!(base32_decode("MZXQ===="), None);
        assert_eq!(base32_decode("MZX!"), None);
    }
}
