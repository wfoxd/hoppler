//! The advert hint (T09a) — recognising a paired friend without a connection.
//!
//! An advertisement is seen by everyone in range, so it may not carry anything
//! that names us. It may carry something only a friend can read, and that is
//! what this is: eight bytes that a device we have completed a ceremony with
//! can recompute, and nobody else can distinguish from noise.
//!
//! ```text
//! hint = keyed_hash(key = l1_pub, CONTEXT || epoch || peer_id)[..8]
//! epoch = now_ms / ROTATION_PERIOD
//! ```
//!
//! # Why each ingredient is there
//!
//! **Keyed on the advertiser's Layer-1 public key**, not on a per-pairing
//! secret. A per-pairing key would mean one hint per friend in the payload, so
//! the advert would grow with the address book and BLE's hundred bytes would be
//! gone at a dozen friends. Keyed on the identity, one hint serves everybody,
//! and the set of people who can compute it is exactly the set who have stood
//! next to us and pressed confirm — a Layer-1 key is disclosed nowhere else.
//!
//! **Bound to the id it is advertised under.** This is not in the T09a sketch
//! and it is not optional. Our transport id rotates on our own twelve-minute
//! timer, which does not line up with the epoch grid, so a hint that depended
//! only on the epoch would stay the same across a rotation — and eight
//! identical bytes under two different ids link them, for any scanner at all,
//! with no key and no pairing. That is precisely the attack R0-F2 rotates to
//! prevent, so a hint built that way would give away more than it bought.
//! Binding the id in costs nothing: the scanner already has it, because it came
//! in the same advertisement.
//!
//! **Bound to an epoch**, which is what keeps a recording from working forever.
//! Without it the hint is a fixed function of an id, and anyone who copies down
//! the pair can replay it at any future date and be greeted as a friend.
//!
//! The window that closes is wider than one [`super::ROTATION_PERIOD`], because
//! [`written_by`] accepts the neighbouring epochs too. A hint written in epoch
//! W verifies for as long as the reader's own clock reads W-1, W or W+1 — so
//! from the moment it was written, somewhere between two and three periods, or
//! **24 to 36 minutes** at the current cadence. That is the price of tolerating
//! two devices that disagree about the time, and it is the number to quote
//! rather than the period itself.
//!
//! # What it costs, stated
//!
//! Somebody who pairs with us **later** can recognise adverts we sent before we
//! ever met them, because pairing hands them the key that generates every hint
//! we have ever written. Unlinkability holds against strangers; it does not
//! hold against future friends. The alternative buys that property with the
//! advert budget, and the budget is what makes the feature possible on BLE at
//! all. Worth revisiting if the advert ever gains room.

use crate::crypto::hash::{keyed_hash, HASH_LEN};

/// Bytes of the hint as it travels. Eight, because the advert is the scarcest
/// space in the system and a forger gets one guess per advertisement — 2^64
/// tries at radio speed is not a threat, whereas the other sixty-odd bytes
/// buy nothing.
pub const HINT_LEN: usize = 8;

/// BLAKE3 domain separation. Versioned, so a later hint shape cannot be
/// mistaken for this one by a device that still speaks it.
const CONTEXT: &[u8] = b"hoppler/advert-hint/v1";

/// Which epoch a wall-clock instant falls in.
///
/// `div_euclid` rather than `/`, so the grid stays even on both sides of zero.
/// Truncating division rounds toward zero, which folds the epoch before 1970
/// and the one after it into a single number — one epoch twice as long as every
/// other, and a hint that keeps verifying across it.
///
/// Neither clock in the tree reaches that today: `system_millis` and
/// `engine::now_millis` both report 0 rather than a negative on a clock they
/// cannot read. This is about the function being total rather than about a
/// failure anybody has seen — it takes an `i64` from a caller-supplied clock,
/// and a discontinuity that only appears for inputs the current callers happen
/// not to produce is the kind that surfaces years later as a bug in something
/// else.
pub fn epoch_at(now_ms: i64) -> i64 {
    now_ms.div_euclid(super::ROTATION_PERIOD.as_millis() as i64)
}

/// The hint the holder of `l1_pub` advertises under `peer` during `epoch`.
pub fn hint_for(l1_pub: &[u8; HASH_LEN], peer: &str, epoch: i64) -> [u8; HINT_LEN] {
    // Fixed-width epoch first, variable-width id last: the only way to read
    // these bytes back is the way they were written, so no pair of different
    // (epoch, peer) inputs can produce the same message.
    let mut data = Vec::with_capacity(CONTEXT.len() + 8 + peer.len());
    data.extend_from_slice(CONTEXT);
    data.extend_from_slice(&epoch.to_be_bytes());
    data.extend_from_slice(peer.as_bytes());
    let full = keyed_hash(l1_pub, &data);
    let mut out = [0u8; HINT_LEN];
    out.copy_from_slice(&full[..HINT_LEN]);
    out
}

/// Read a hint out of an advertisement payload.
///
/// Strict on length. A payload of any other size is not a hint we understand —
/// an older build advertising nothing, or a later one carrying a shape this
/// version was never taught — and guessing at it would turn a stranger's bytes
/// into a friend's name.
pub fn read(payload: &[u8]) -> Option<[u8; HINT_LEN]> {
    let mut out = [0u8; HINT_LEN];
    out.copy_from_slice(
        payload
            .get(..HINT_LEN)
            .filter(|_| payload.len() == HINT_LEN)?,
    );
    Some(out)
}

/// Whether `hint`, seen under `peer` at `now_ms`, was written by the holder of
/// `l1_pub`.
///
/// Three epochs are tried — the one before, the current one, and the one after.
/// Two devices do not share a clock: the two phones on this desk have run
/// 1.11 s and 1.59 s apart on different days, and a hint computed either side
/// of a twelve-minute boundary has to match anyway. The neighbours also absorb
/// the gap between our advertising the hint and the scanner reading it.
///
/// A match makes a sighting resolve to a contact, which decides what name the
/// row carries and which device a thread will dial, so this is a security check
/// and not a display hint. Hence the constant-time compare: a caller that
/// leaked how far a comparison got would let a stranger walk a forged hint one
/// byte at a time into being greeted as somebody's friend.
pub fn written_by(l1_pub: &[u8; HASH_LEN], peer: &str, hint: &[u8; HINT_LEN], now_ms: i64) -> bool {
    let here = epoch_at(now_ms);
    // Folded into an accumulator rather than short-circuited with `any`: the
    // number of hashes must not depend on which epoch matched either.
    let mut hit = 0u8;
    for epoch in [here - 1, here, here + 1] {
        hit |= u8::from(same_bytes(&hint_for(l1_pub, peer, epoch), hint));
    }
    hit != 0
}

/// Compare without telling the caller where the difference was.
fn same_bytes(a: &[u8; HINT_LEN], b: &[u8; HINT_LEN]) -> bool {
    let mut diff = 0u8;
    for i in 0..HINT_LEN {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Derived, never restated: a test that hardcoded twelve minutes would go
    /// on passing against a changed [`super::super::ROTATION_PERIOD`] while
    /// saying nothing about it.
    const PERIOD_MS: i64 = super::super::ROTATION_PERIOD.as_millis() as i64;
    const KEY: [u8; HASH_LEN] = [3u8; HASH_LEN];
    const OTHER_KEY: [u8; HASH_LEN] = [4u8; HASH_LEN];

    fn advert(key: &[u8; HASH_LEN], peer: &str, at_ms: i64) -> [u8; HINT_LEN] {
        hint_for(key, peer, epoch_at(at_ms))
    }

    #[test]
    fn the_key_that_wrote_a_hint_is_recognised() {
        let now = 50 * PERIOD_MS;
        assert!(written_by(&KEY, "abc", &advert(&KEY, "abc", now), now));
    }

    #[test]
    fn another_key_is_not() {
        let now = 50 * PERIOD_MS;
        assert!(!written_by(
            &OTHER_KEY,
            "abc",
            &advert(&KEY, "abc", now),
            now
        ));
    }

    /// The whole reason the id is in the hash. Two ids carrying the same eight
    /// bytes are linkable by any scanner in range, with no key and no pairing,
    /// which is the linkage R0-F2 rotates to break — so a rotation inside one
    /// epoch must still change what we advertise.
    #[test]
    fn rotating_the_id_changes_the_hint_even_inside_one_epoch() {
        let now = 50 * PERIOD_MS;
        assert_ne!(advert(&KEY, "before", now), advert(&KEY, "after", now));
    }

    /// The other half of it: a hint copied off the air is worthless under
    /// anybody else's id, so it cannot be worn as a disguise.
    #[test]
    fn a_hint_does_not_travel_to_another_id() {
        let now = 50 * PERIOD_MS;
        assert!(!written_by(
            &KEY,
            "somebody-else",
            &advert(&KEY, "mine", now),
            now
        ));
    }

    #[test]
    fn the_hint_turns_with_the_epoch() {
        let peer = "abc";
        assert_ne!(
            advert(&KEY, peer, 50 * PERIOD_MS),
            advert(&KEY, peer, 51 * PERIOD_MS)
        );
    }

    /// Two phones do not share a clock — the pair on this desk have run 1.11 s
    /// and 1.59 s apart on different days — so a hint written just the other
    /// side of a boundary has to be recognised anyway.
    #[test]
    fn the_neighbouring_epochs_are_accepted() {
        let now = 50 * PERIOD_MS;
        for written_at in [now - PERIOD_MS, now, now + PERIOD_MS] {
            assert!(
                written_by(&KEY, "abc", &advert(&KEY, "abc", written_at), now),
                "epoch at {written_at} should have been accepted"
            );
        }
    }

    /// And no further, or the replay window grows without limit.
    #[test]
    fn anything_further_out_is_refused() {
        let now = 50 * PERIOD_MS;
        for written_at in [now - 2 * PERIOD_MS, now + 2 * PERIOD_MS] {
            assert!(
                !written_by(&KEY, "abc", &advert(&KEY, "abc", written_at), now),
                "epoch at {written_at} should have been refused"
            );
        }
    }

    /// A boundary is a boundary from both sides: the last millisecond of an
    /// epoch and the first of the next must not land on the same number.
    #[test]
    fn the_epoch_grid_divides_where_it_says() {
        assert_eq!(epoch_at(50 * PERIOD_MS), 50);
        assert_eq!(epoch_at(51 * PERIOD_MS - 1), 50);
        assert_eq!(epoch_at(51 * PERIOD_MS), 51);
    }

    /// A device whose clock has never been set reports a moment before 1970.
    /// Truncating division would fold the epoch either side of zero into one,
    /// making a single epoch twice as long as every other; `div_euclid` does
    /// not.
    #[test]
    fn the_grid_holds_before_1970() {
        assert_eq!(epoch_at(-1), -1);
        assert_eq!(epoch_at(-PERIOD_MS), -1);
        assert_eq!(epoch_at(-PERIOD_MS - 1), -2);
    }

    #[test]
    fn a_payload_of_exactly_the_right_size_is_a_hint() {
        assert_eq!(read(&[9u8; HINT_LEN]), Some([9u8; HINT_LEN]));
    }

    /// Neither a shorter payload nor a longer one is read as a hint. The long
    /// case is the one worth pinning: taking the first eight bytes of a payload
    /// some later version defined would turn its unrelated bytes into a
    /// friend's name on this screen.
    #[test]
    fn any_other_payload_is_not() {
        assert_eq!(read(&[]), None);
        assert_eq!(read(&[9u8; HINT_LEN - 1]), None);
        assert_eq!(read(&[9u8; HINT_LEN + 1]), None);
    }

    /// The hint is a wire format: two devices that disagree about these bytes
    /// stop recognising each other, and nothing on either screen says why. So
    /// it is pinned as a value and not as a restatement of how it is built —
    /// the test above derives its expectation from the same constants the code
    /// does, and would follow a change to any of them without a word.
    ///
    /// Changing this number is changing the protocol. It needs both phones
    /// reflashed together, and it belongs behind the version gate.
    #[test]
    fn the_hint_has_not_moved() {
        assert_eq!(hex::encode(hint_for(&KEY, "abc", 7)), "a23fc305f49fdc5d");
    }

    #[test]
    fn the_hint_is_the_front_of_the_keyed_hash() {
        let full = keyed_hash(&KEY, {
            let mut d = Vec::from(CONTEXT);
            d.extend_from_slice(&7i64.to_be_bytes());
            d.extend_from_slice(b"abc");
            &d.clone()
        });
        assert_eq!(hint_for(&KEY, "abc", 7)[..], full[..HINT_LEN]);
    }

    /// The epoch is fixed-width and comes first, so no (epoch, id) pair can be
    /// re-cut into another one. Without that, epoch 1 under id "23" and epoch
    /// 12 under id "3" would be the same message.
    #[test]
    fn the_epoch_and_the_id_cannot_be_confused_for_each_other() {
        assert_ne!(hint_for(&KEY, "23", 1), hint_for(&KEY, "3", 12));
    }
}
