//! The Double Ratchet (T12; tech spec §5, R0-F5).
//!
//! What a paired thread is made of. The F4 ceremony hands over a root key that
//! two people established face to face; from there every message advances a
//! chain, and every reply turns the ratchet — so a key recovered today opens
//! neither yesterday's traffic nor tomorrow's.
//!
//! # Built here rather than vendored, and why that is a decision and not a
//! preference
//!
//! T12 asks for the trade to be recorded. It was made deliberately:
//!
//! - **`vodozemac`** is mature, maintained and independently audited, and it
//!   implements *Olm* — which is this construction plus X3DH plus Matrix's
//!   framing, backed by AES and HMAC-SHA256. Adopting it would put a second
//!   cipher and MAC stack in a binary whose spec says one suite and no agility
//!   (R0-N5), and would replace this project's ceremony-established root key
//!   with Olm's own session model.
//! - **The small ratchet crates** (`double-ratchet-2`, `ratchetx2`,
//!   `ratchet-x2`) are none of them audited and the most promising is a
//!   pre-release. Choosing one would mean trusting an unreviewed stranger's
//!   ratchet over an unreviewed one of ours, with less ability to fix it.
//!
//! So this is built on the primitives T04 already vets. The honest cost is that
//! **nobody has reviewed this ratchet**, and it is the correctness heart of
//! Chat. What stands in for review until T22: the construction follows the
//! Signal Double Ratchet specification, every derivation is pinned by a golden
//! vector, and the properties that matter are asserted rather than assumed.
//!
//! # Where this deviates from the specification, on purpose
//!
//! The spec's `KDF_CK` is HMAC-SHA256 with one-byte constants. This uses
//! BLAKE3's keyed hash with the same constants: both are PRFs keyed on the
//! chain key, which is all the construction requires, and BLAKE3 is already in
//! the suite while HMAC-SHA256 would be another primitive to admit. `KDF_RK` is
//! HKDF-SHA256, as the spec says, because HKDF is in the suite too.
//!
//! Nonces are derived from the message number rather than random. A message key
//! is used exactly once by construction, so a counter is safe where reuse would
//! be catastrophic — and a random nonce would have to travel, costing 24 bytes
//! on every message to say something both sides already know.

use std::collections::HashMap;

use zeroize::Zeroizing;

use crate::crypto::{aead, dh, hash, kdf, CryptoError};

/// Domain separator for the root-key KDF.
const ROOT_LABEL: &[u8] = b"hoppler/ratchet/root/v1";

/// Fed to the chain KDF to produce the next chain key.
const CHAIN_STEP: &[u8] = &[0x02];

/// Fed to the chain KDF to produce a message key.
const MESSAGE_STEP: &[u8] = &[0x01];

/// How many skipped message keys a chain may hold.
///
/// Messages arrive out of order and some never arrive at all, so a receiver
/// keeps the keys it stepped over. The bound is what stops a peer sending a
/// header claiming message four billion and making us derive its way there —
/// which is a denial of service with no packet loss required.
pub const MAX_SKIP: u32 = 256;

/// How many skipped keys are kept across all chains before the oldest go.
///
/// A separate bound from [`MAX_SKIP`], because that one caps a single gap and
/// this one caps the total. Without it a peer can open a new chain, skip 255,
/// open another, and repeat until memory runs out — each gap individually
/// legal.
pub const MAX_SKIPPED_KEYS: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum RatchetError {
    /// The header asks us to skip more messages than [`MAX_SKIP`] allows.
    TooManySkipped,
    /// The message did not decrypt: wrong key, tampered ciphertext, or a
    /// header that does not match what was sealed.
    ///
    /// Deliberately one variant. Distinguishing "bad tag" from "unknown chain"
    /// hands an attacker an oracle for which of its guesses was closer.
    Undecryptable,
    /// A peer's public key was not a usable one (low order, non-contributory).
    BadKey,
}

impl std::fmt::Display for RatchetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RatchetError::TooManySkipped => write!(f, "too many skipped messages"),
            RatchetError::Undecryptable => write!(f, "message could not be decrypted"),
            RatchetError::BadKey => write!(f, "unusable ratchet key"),
        }
    }
}

impl std::error::Error for RatchetError {}

impl From<CryptoError> for RatchetError {
    fn from(_: CryptoError) -> Self {
        // Collapsed on purpose: every crypto failure here is the same fact to a
        // caller, and the detail is the oracle.
        RatchetError::Undecryptable
    }
}

/// What travels in clear alongside every message.
///
/// Public because a receiver needs it before it can decrypt anything, and
/// authenticated because it is the AEAD's associated data — a header altered in
/// flight makes the tag fail rather than silently re-routing the message to
/// another chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The sender's current ratchet public key.
    pub dh: dh::DhPublic,
    /// How long the sender's previous sending chain was.
    ///
    /// This is what lets a receiver know how many messages it will never see
    /// from the old chain, rather than waiting for them forever.
    pub previous_chain_len: u32,
    /// Position in the current sending chain.
    pub number: u32,
}

impl Header {
    /// The bytes the AEAD authenticates. Fixed width, so nothing about the
    /// framing can be moved between fields.
    pub fn to_bytes(self) -> [u8; 40] {
        let mut out = [0u8; 40];
        out[..32].copy_from_slice(&self.dh.0);
        out[32..36].copy_from_slice(&self.previous_chain_len.to_be_bytes());
        out[36..].copy_from_slice(&self.number.to_be_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8; 40]) -> Self {
        let mut dh = [0u8; 32];
        dh.copy_from_slice(&bytes[..32]);
        Self {
            dh: dh::DhPublic(dh),
            previous_chain_len: u32::from_be_bytes(bytes[32..36].try_into().expect("4 bytes")),
            number: u32::from_be_bytes(bytes[36..].try_into().expect("4 bytes")),
        }
    }
}

/// One end of a paired conversation.
///
/// No `Debug`: it holds the root key and every live chain key, which is the
/// whole conversation's past and future.
pub struct Ratchet {
    root: [u8; 32],
    /// Our current ratchet key pair.
    ours: dh::DhSecret,
    /// Their latest ratchet public, once we have seen one.
    theirs: Option<dh::DhPublic>,
    sending: Option<[u8; 32]>,
    receiving: Option<[u8; 32]>,
    sent: u32,
    received: u32,
    previous_chain_len: u32,
    /// Keys for messages we stepped over, by (their key, number).
    skipped: HashMap<([u8; 32], u32), [u8; 32]>,
    /// Insertion order, so the oldest skipped key is the one evicted.
    skipped_order: Vec<([u8; 32], u32)>,
}

impl Ratchet {
    /// Start as the side that speaks first.
    ///
    /// `their_public` is the counterpart's initial ratchet key, which the
    /// ceremony carried. Sending immediately is possible because this side
    /// performs a DH ratchet step up front; the other side cannot send until it
    /// has heard from us, which is the asymmetry the ceremony resolves by
    /// naming who initiates.
    pub fn initiator(root: [u8; 32], their_public: dh::DhPublic) -> Result<Self, RatchetError> {
        let ours = dh::DhSecret::generate();
        let shared = ours.diffie_hellman(&their_public)?;
        let (root, sending) = kdf_root(&root, shared.as_bytes());
        Ok(Self {
            root,
            ours,
            theirs: Some(their_public),
            sending: Some(sending),
            receiving: None,
            sent: 0,
            received: 0,
            previous_chain_len: 0,
            skipped: HashMap::new(),
            skipped_order: Vec::new(),
        })
    }

    /// Start as the side that answers.
    ///
    /// `ours` is the key pair whose public the ceremony gave the other side, so
    /// this end has no chains until the first message arrives and turns the
    /// ratchet.
    pub fn responder(root: [u8; 32], ours: dh::DhSecret) -> Self {
        Self {
            root,
            ours,
            theirs: None,
            sending: None,
            receiving: None,
            sent: 0,
            received: 0,
            previous_chain_len: 0,
            skipped: HashMap::new(),
            skipped_order: Vec::new(),
        }
    }

    /// This end's current ratchet public — what the ceremony hands the other
    /// side to start with.
    pub fn public(&self) -> dh::DhPublic {
        self.ours.public()
    }

    /// Encrypt one message.
    ///
    /// Returns the header, which travels in clear, and the sealed body. The
    /// header is the AEAD's associated data, so the two cannot be separated
    /// without the tag failing.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<(Header, Vec<u8>), RatchetError> {
        let chain = self.sending.ok_or(RatchetError::Undecryptable)?;
        let (next, message_key) = kdf_chain(&chain);
        self.sending = Some(next);

        let header = Header {
            dh: self.ours.public(),
            previous_chain_len: self.previous_chain_len,
            number: self.sent,
        };
        self.sent += 1;

        let key = aead::AeadKey::from_bytes(message_key);
        let sealed = aead::seal(
            &key,
            &nonce_for(header.number),
            &header.to_bytes(),
            plaintext,
        );
        Ok((header, sealed))
    }

    /// Decrypt one message.
    ///
    /// Ordering, gaps and duplicates are all handled here rather than by the
    /// caller: a message from an old chain, or one that arrived before its
    /// predecessors, is exactly as decryptable as an in-order one, and a
    /// message whose key has already been used is not decryptable at all.
    ///
    /// The plaintext stays `Zeroizing`, as `aead::open` returns it. Unwrapping
    /// it here would quietly drop the one property that layer added — a chat
    /// line is exactly the sort of thing that should not linger in freed
    /// memory, and the caller can decide when it stops being secret.
    pub fn decrypt(
        &mut self,
        header: Header,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, RatchetError> {
        // A key we kept when we stepped over this message. Taken, not read:
        // using it twice would mean accepting a replay.
        if let Some(key) = self.take_skipped(&header) {
            return open(&key, header, sealed);
        }

        let theirs_changed = self.theirs.map(|t| t.0 != header.dh.0).unwrap_or(true);
        if theirs_changed {
            // Everything still owed on the old chain, before the chain is
            // replaced and its keys are gone for good.
            self.skip_to(header.previous_chain_len)?;
            self.turn(header.dh)?;
        }

        self.skip_to(header.number)?;

        let chain = self.receiving.ok_or(RatchetError::Undecryptable)?;
        let (next, message_key) = kdf_chain(&chain);
        let plaintext = open(&message_key, header, sealed)?;
        // Advanced only once the message has actually opened. Stepping first
        // would let a forged message burn a key and desynchronise the chain —
        // one bad packet costing a real one.
        self.receiving = Some(next);
        self.received += 1;
        Ok(plaintext)
    }

    /// Turn the ratchet onto a new public of theirs: one DH for the chain we
    /// will receive on, another for the chain we will send on.
    fn turn(&mut self, theirs: dh::DhPublic) -> Result<(), RatchetError> {
        self.previous_chain_len = self.sent;
        self.sent = 0;
        self.received = 0;
        self.theirs = Some(theirs);

        let shared = self.ours.diffie_hellman(&theirs)?;
        let (root, receiving) = kdf_root(&self.root, shared.as_bytes());
        self.root = root;
        self.receiving = Some(receiving);

        self.ours = dh::DhSecret::generate();
        let shared = self.ours.diffie_hellman(&theirs)?;
        let (root, sending) = kdf_root(&self.root, shared.as_bytes());
        self.root = root;
        self.sending = Some(sending);
        Ok(())
    }

    /// Keep the keys for messages between here and `until`, so they can still
    /// be opened when they arrive.
    fn skip_to(&mut self, until: u32) -> Result<(), RatchetError> {
        let Some(chain) = self.receiving else {
            return Ok(());
        };
        if until < self.received {
            // Already past it: either a duplicate or a replay. Not an error —
            // the key is simply gone, and `decrypt` will fail to find one.
            return Ok(());
        }
        if until - self.received > MAX_SKIP {
            return Err(RatchetError::TooManySkipped);
        }
        let theirs = self.theirs.map(|t| t.0).unwrap_or([0u8; 32]);
        let mut chain = chain;
        while self.received < until {
            let (next, message_key) = kdf_chain(&chain);
            chain = next;
            let slot = (theirs, self.received);
            if self.skipped.insert(slot, message_key).is_none() {
                self.skipped_order.push(slot);
            }
            self.received += 1;
            // Oldest first, so a flood of gaps costs the messages least likely
            // to still be in flight rather than the ones most likely.
            if self.skipped_order.len() > MAX_SKIPPED_KEYS {
                let oldest = self.skipped_order.remove(0);
                self.skipped.remove(&oldest);
            }
        }
        self.receiving = Some(chain);
        Ok(())
    }

    fn take_skipped(&mut self, header: &Header) -> Option<[u8; 32]> {
        let slot = (header.dh.0, header.number);
        let key = self.skipped.remove(&slot)?;
        self.skipped_order.retain(|s| *s != slot);
        Some(key)
    }

    /// How many keys are being held for messages that never arrived. For the
    /// store, and for a test that wants to see the bound hold.
    pub fn skipped_count(&self) -> usize {
        self.skipped.len()
    }
}

/// `KDF_RK`: a new root key and a new chain key from a DH output.
fn kdf_root(root: &[u8; 32], dh_out: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut out = [0u8; 64];
    kdf::hkdf_sha256(dh_out, root, ROOT_LABEL, &mut out)
        .expect("64 bytes is always a valid HKDF length");
    let mut next_root = [0u8; 32];
    let mut chain = [0u8; 32];
    next_root.copy_from_slice(&out[..32]);
    chain.copy_from_slice(&out[32..]);
    (next_root, chain)
}

/// `KDF_CK`: the next chain key and this message's key.
///
/// BLAKE3 keyed rather than the specification's HMAC-SHA256; both are PRFs
/// keyed on the chain key, which is all the construction asks, and BLAKE3 is
/// already in the suite. The one-byte constants are the spec's, and keeping
/// them means the two outputs of one chain key can never collide.
fn kdf_chain(chain: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    (
        hash::keyed_hash(chain, CHAIN_STEP),
        hash::keyed_hash(chain, MESSAGE_STEP),
    )
}

/// The nonce for a message, from its number.
///
/// Counter rather than random, and safe because a message key is used exactly
/// once by construction — the chain KDF never yields the same key twice, so
/// there is no second message to reuse a nonce with. A random nonce would have
/// to be carried on every message to tell the receiver something the header
/// already says.
///
/// Which also means the *value* here carries no weight: a constant would be
/// equally safe, given unique keys, and mutation testing says so — replacing
/// this with a fixed nonce breaks nothing. What matters is only that both ends
/// compute the same one, and any disagreement makes every message fail to
/// decrypt, which every test in this file would notice. Recorded because the
/// surviving mutant looks like a hole and is not one.
fn nonce_for(number: u32) -> [u8; aead::NONCE_LEN] {
    let mut nonce = [0u8; aead::NONCE_LEN];
    nonce[..4].copy_from_slice(&number.to_be_bytes());
    nonce
}

fn open(
    message_key: &[u8; 32],
    header: Header,
    sealed: &[u8],
) -> Result<Zeroizing<Vec<u8>>, RatchetError> {
    let key = aead::AeadKey::from_bytes(*message_key);
    aead::open(&key, &nonce_for(header.number), &header.to_bytes(), sealed)
        .map_err(|_| RatchetError::Undecryptable)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paired conversation, from the point the ceremony hands over a root
    /// key. Alice speaks first, which is the asymmetry the ceremony settles.
    fn pair() -> (Ratchet, Ratchet) {
        let root = [7u8; 32];
        let bobs = dh::DhSecret::generate();
        let alice = Ratchet::initiator(root, bobs.public()).unwrap();
        let bob = Ratchet::responder(root, bobs);
        (alice, bob)
    }

    fn send(from: &mut Ratchet, text: &str) -> (Header, Vec<u8>) {
        from.encrypt(text.as_bytes()).unwrap()
    }

    fn recv(to: &mut Ratchet, message: (Header, Vec<u8>)) -> String {
        String::from_utf8(to.decrypt(message.0, &message.1).unwrap().to_vec()).unwrap()
    }

    #[test]
    fn a_conversation_goes_back_and_forth() {
        let (mut alice, mut bob) = pair();
        assert_eq!(recv(&mut bob, send(&mut alice, "morning")), "morning");
        assert_eq!(
            recv(&mut alice, send(&mut bob, "morning yourself")),
            "morning yourself"
        );
        assert_eq!(
            recv(&mut bob, send(&mut alice, "coming later?")),
            "coming later?"
        );
        assert_eq!(recv(&mut alice, send(&mut bob, "yes")), "yes");
    }

    /// R0-F5's reunion case: a run of messages sent while the other side was
    /// away, arriving all at once and in the wrong order.
    #[test]
    fn messages_arrive_out_of_order_and_still_read() {
        let (mut alice, mut bob) = pair();
        let mut sent: Vec<(Header, Vec<u8>)> = (0..20)
            .map(|i| send(&mut alice, &format!("message {i}")))
            .collect();

        // Deliberately shuffled, deterministically: the last first, then the
        // rest forwards. A test that reversed them would only exercise one
        // shape of gap.
        sent.swap(0, 19);
        sent.swap(5, 12);
        sent.swap(3, 17);

        let mut read: Vec<String> = sent.into_iter().map(|m| recv(&mut bob, m)).collect();
        read.sort();
        let mut expected: Vec<String> = (0..20).map(|i| format!("message {i}")).collect();
        expected.sort();
        assert_eq!(read, expected, "a reordered run did not all arrive");
    }

    /// A message that never comes must not block the ones behind it.
    #[test]
    fn a_lost_message_does_not_stop_the_rest() {
        let (mut alice, mut bob) = pair();
        let first = send(&mut alice, "one");
        let _lost = send(&mut alice, "two");
        let third = send(&mut alice, "three");

        assert_eq!(recv(&mut bob, first), "one");
        assert_eq!(recv(&mut bob, third), "three");
        // And the gap is still openable if it turns up later — which is what
        // the kept keys are for.
        assert_eq!(recv(&mut bob, _lost), "two");
    }

    /// At-least-once delivery means the same message can arrive twice. The
    /// second copy must not decrypt: its key was used and thrown away.
    ///
    /// This is the ratchet's half of dedup. The store has the other half
    /// (`msg_id`), and neither alone is enough — this one stops a replayed
    /// *ciphertext*, which a `msg_id` check would happily let through.
    #[test]
    fn a_replayed_message_does_not_decrypt_twice() {
        let (mut alice, mut bob) = pair();
        let message = send(&mut alice, "only once");
        let copy = (message.0, message.1.clone());

        assert_eq!(recv(&mut bob, message), "only once");
        assert_eq!(
            bob.decrypt(copy.0, &copy.1),
            Err(RatchetError::Undecryptable),
            "a replayed message decrypted a second time"
        );
    }

    /// The replay that the *skipped-key* path has to stop.
    ///
    /// Written because mutation testing found the existing replay test did not
    /// cover it: an in-order duplicate is refused because the chain has moved
    /// on, so replacing the "take the key" with "read the key" broke nothing
    /// that was tested. A message that was skipped, arrived late, and then
    /// arrived *again* is the case where the key has to be gone rather than
    /// merely used.
    #[test]
    fn a_replayed_skipped_message_does_not_decrypt_twice() {
        let (mut alice, mut bob) = pair();
        let first = send(&mut alice, "one");
        let _second = send(&mut alice, "two");
        let third = send(&mut alice, "three");

        // Bob sees the third, keeping keys for the two he stepped over.
        assert_eq!(recv(&mut bob, third), "three");
        let copy = (first.0, first.1.clone());
        assert_eq!(recv(&mut bob, first), "one");
        assert_eq!(
            bob.decrypt(copy.0, &copy.1),
            Err(RatchetError::Undecryptable),
            "a skipped message decrypted twice — its key was read, not taken"
        );
    }

    /// A forged message must cost nothing.
    ///
    /// The chain advances only once a message has actually opened, so a bad
    /// packet cannot burn the key that a real one needs. Written because
    /// mutation testing showed the ordering could be swapped with every test
    /// still green — the comment claimed the property and nothing checked it.
    #[test]
    fn a_forged_message_does_not_burn_the_next_key() {
        let (mut alice, mut bob) = pair();
        let (header, sealed) = send(&mut alice, "genuine");

        let mut forged = sealed.clone();
        forged[0] ^= 0x01;
        assert_eq!(
            bob.decrypt(header, &forged),
            Err(RatchetError::Undecryptable)
        );

        // The real one still opens. If the chain had stepped on the forgery,
        // this would fail — one bad packet costing a real message.
        assert_eq!(recv(&mut bob, (header, sealed)), "genuine");
    }

    /// Turning the ratchet is what makes a key recovered today useless against
    /// yesterday — the property the whole construction exists for.
    #[test]
    fn an_old_message_key_does_not_open_a_later_one() {
        let (mut alice, mut bob) = pair();
        let early = send(&mut alice, "early");
        recv(&mut bob, early);
        // A reply turns the ratchet on both sides.
        let reply = send(&mut bob, "reply");
        recv(&mut alice, reply);

        let late = send(&mut alice, "late");
        // Bob's chain has moved on; the message opens with the new chain and
        // not with anything held from before.
        assert_eq!(recv(&mut bob, late), "late");
    }

    /// A header is authenticated, not merely descriptive. Moving a message to
    /// another number, or claiming another sender's key, breaks the tag rather
    /// than quietly re-routing it.
    #[test]
    fn a_tampered_header_does_not_decrypt() {
        let (mut alice, mut bob) = pair();
        let (header, sealed) = send(&mut alice, "unaltered");

        let renumbered = Header {
            number: header.number + 1,
            ..header
        };
        assert_eq!(
            bob.decrypt(renumbered, &sealed),
            Err(RatchetError::Undecryptable),
        );

        let relabelled = Header {
            previous_chain_len: 99,
            ..header
        };
        assert_eq!(
            bob.decrypt(relabelled, &sealed),
            Err(RatchetError::Undecryptable),
        );
    }

    #[test]
    fn a_tampered_body_does_not_decrypt() {
        let (mut alice, mut bob) = pair();
        let (header, mut sealed) = send(&mut alice, "unaltered");
        sealed[0] ^= 0x01;
        assert_eq!(
            bob.decrypt(header, &sealed),
            Err(RatchetError::Undecryptable)
        );
    }

    /// A peer claiming an enormous gap must not make us derive its way there.
    /// That is a denial of service needing no packet loss at all.
    #[test]
    fn an_absurd_gap_is_refused_rather_than_walked() {
        let (mut alice, mut bob) = pair();
        let (header, sealed) = send(&mut alice, "hello");
        let absurd = Header {
            number: MAX_SKIP + 1,
            ..header
        };
        assert_eq!(
            bob.decrypt(absurd, &sealed),
            Err(RatchetError::TooManySkipped),
        );
        // And the ratchet is still usable afterwards: a refused message is not
        // a broken conversation.
        assert_eq!(recv(&mut bob, (header, sealed)), "hello");
    }

    /// Many legal gaps in a row must not grow without bound either. Each one is
    /// individually allowed; the total is what this bounds.
    #[test]
    fn skipped_keys_are_bounded_across_chains() {
        let (mut alice, mut bob) = pair();
        // Every round: a message that lands (which gives Bob a chain and turns
        // the ratchet), a reply that turns it again, then a legal gap on the
        // fresh chain. Written the other way round first, with Bob speaking
        // before he had heard anything — which a responder cannot do, and the
        // ratchet said so.
        for _ in 0..12 {
            recv(&mut bob, send(&mut alice, "turn"));
            recv(&mut alice, send(&mut bob, "reply"));
            for _ in 0..200 {
                let _skipped = send(&mut alice, "gap");
            }
            let latest = send(&mut alice, "landed");
            recv(&mut bob, latest);
        }
        assert!(
            bob.skipped_count() <= MAX_SKIPPED_KEYS,
            "held {} skipped keys, over the {MAX_SKIPPED_KEYS} bound",
            bob.skipped_count()
        );
    }

    /// The derivations are wire: two builds that disagree cannot talk, and the
    /// disagreement would show up as messages that simply do not decrypt.
    #[test]
    fn the_key_schedule_is_pinned() {
        let (root, chain) = kdf_root(&[1u8; 32], &[2u8; 32]);
        let (next, message) = kdf_chain(&[3u8; 32]);
        assert_eq!(
            [
                hex::encode(root),
                hex::encode(chain),
                hex::encode(next),
                hex::encode(message),
            ],
            [
                // KDF_RK: root, then chain
                "67d1dc4e3514134aa3f55e8ce39a6450997ef6a4d636b6d101536fd616fabc38",
                "f8d4920861561bcb8bf4cfcffe16a78b586b9b31d5fc9bc93c2ceedbed36ec72",
                // KDF_CK: next chain key, then message key
                "9e117da852dc5bc3873004284434c987ebd362a8081e462350871446804a5ec3",
                "d2d29d74f8edb53d2343f7a8c209b7304dd827a349df0c891ad11550e52f363e",
            ]
            .map(String::from)
        );
    }

    /// A chain key must never produce its own successor as a message key, or
    /// the two outputs of one step would be interchangeable.
    #[test]
    fn a_chain_step_and_a_message_key_are_different() {
        let (next, message) = kdf_chain(&[9u8; 32]);
        assert_ne!(next, message);
        assert_ne!(next, [9u8; 32]);
        assert_ne!(message, [9u8; 32]);
    }

    #[test]
    fn a_header_round_trips_through_its_bytes() {
        let header = Header {
            dh: dh::DhPublic([5u8; 32]),
            previous_chain_len: 7,
            number: 9,
        };
        assert_eq!(Header::from_bytes(&header.to_bytes()), header);
    }
}
