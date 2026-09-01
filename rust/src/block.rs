//! The block list, and the one door every ingress goes through (R0-F10, tech
//! spec §12).
//!
//! # Why this is a module and not two fields
//!
//! Blocking is enforced in four places that share no code: the persona
//! endpoint, the session handshake, an established session, and the local
//! nearby list. Before this module there were two copies of the set — one in
//! `Discovery`, one in `Net` — kept in step by a single function remembering to
//! write both. Two copies of a security decision is one copy and a bug waiting
//! for the third surface to be written.
//!
//! There is now one [`Blocklist`], created at start-up, shared by `Arc`. Adding
//! a surface means asking it; there is nothing else to ask.
//!
//! # Why the answer is not a `bool`
//!
//! An `is_blocked(..) -> bool` reads perfectly well in either polarity, which is
//! exactly why an inverted condition survives review. [`Admit`] does not read
//! well backwards, and it is `#[must_use]`, so a caller that fetches a decision
//! and forgets to act on it does not compile. `session::handshake` makes the
//! same move with its typestate for the same reason: the check should precede
//! the response by construction, not by everyone remembering.
//!
//! # Why the refusing variant is called `Silence`
//!
//! Because that is what it produces. R0-F10 requires a blocked device to
//! observe nothing distinguishable from Discovery being off, so every refusal
//! in this system — blocked, rate-limited, off, unparseable — is the same
//! nothing. A variant named `Blocked` would invite a caller to log which one it
//! was, and that log line would be the only artefact in existence that
//! distinguishes them.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::crypto::dh;

/// The per-counterparty handle a block binds to: a peer's Layer-1-derived
/// pseudonym toward *us*, which is a DH public key.
///
/// See `identity::Identity::pseudonym_toward` for why this is the right handle
/// and a persona is not — a persona can be regenerated, and R0-F10 requires
/// that regenerating one changes nothing the filter sees.
pub type Pseudonym = [u8; dh::PUBLIC_LEN];

/// What an ingress point is allowed to do with a peer.
///
/// `#[must_use]` on purpose: the compiler is the only reviewer that reads every
/// call site.
#[must_use = "an ingress decision that is dropped is an ingress that admits everyone"]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Admit {
    /// Carry on. Nothing about this peer says otherwise.
    Yes,
    /// Say nothing at all — not an error, not a close, not a log line.
    Silence,
}

/// Pseudonyms this device refuses to hear from.
///
/// A set rather than a `Vec`: the lookup is on the receive path of every frame,
/// and the previous linear scan was fine only because the list was always
/// empty.
#[derive(Default)]
pub struct Blocklist {
    who: Mutex<HashSet<Pseudonym>>,
}

impl Blocklist {
    /// The list as it stands on disk. Called once, from `engine::install`,
    /// before anything can answer a peer.
    pub fn loaded(entries: impl IntoIterator<Item = Pseudonym>) -> Self {
        Self {
            who: Mutex::new(entries.into_iter().collect()),
        }
    }

    /// **The gate.** Every ingress point in the app calls this and nothing else.
    ///
    /// Takes the pseudonym a handshake or a request *proved*, never one a peer
    /// merely claimed — a claimed pseudonym makes the list advisory, and an
    /// advisory block list is not one.
    ///
    /// A decision that is fetched and then dropped is a door that admits
    /// everyone, so it does not build:
    ///
    /// ```compile_fail
    /// #![deny(unused_must_use)]
    /// let list = rust_lib_hoppler::block::Blocklist::default();
    /// list.ingress_gate(&[0u8; 32]);
    /// ```
    ///
    /// The control for the above — same lint, same call, decision used. Without
    /// this, a typo would make the `compile_fail` pass while proving nothing:
    ///
    /// ```
    /// #![deny(unused_must_use)]
    /// use rust_lib_hoppler::block::{Admit, Blocklist};
    /// let list = Blocklist::default();
    /// assert_eq!(list.ingress_gate(&[0u8; 32]), Admit::Yes);
    /// ```
    pub fn ingress_gate(&self, who: &Pseudonym) -> Admit {
        if self
            .who
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(who)
        {
            Admit::Silence
        } else {
            Admit::Yes
        }
    }

    /// Add a pseudonym. Idempotent — blocking someone twice is one block.
    pub fn block(&self, who: Pseudonym) {
        self.who
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(who);
    }

    /// Remove a pseudonym, restoring stranger-level status and nothing else.
    /// Whatever the block revoked stays revoked; see T18c.
    pub fn unblock(&self, who: &Pseudonym) {
        self.who
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(who);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: Pseudonym = [1u8; dh::PUBLIC_LEN];
    const BOB: Pseudonym = [2u8; dh::PUBLIC_LEN];

    #[test]
    fn the_gate_answers_for_the_person_it_was_asked_about() {
        let list = Blocklist::default();
        assert_eq!(list.ingress_gate(&ALICE), Admit::Yes);
        list.block(ALICE);
        assert_eq!(list.ingress_gate(&ALICE), Admit::Silence);
        assert_eq!(
            list.ingress_gate(&BOB),
            Admit::Yes,
            "blocking one person closed the door on another"
        );
        list.unblock(&ALICE);
        assert_eq!(list.ingress_gate(&ALICE), Admit::Yes);
    }

    #[test]
    fn a_list_starts_life_holding_what_was_on_disk() {
        let list = Blocklist::loaded([ALICE]);
        assert_eq!(list.ingress_gate(&ALICE), Admit::Silence);
        assert_eq!(list.ingress_gate(&BOB), Admit::Yes);
    }

    /// Blocking twice must not need unblocking twice.
    #[test]
    fn a_second_block_is_still_one_block() {
        let list = Blocklist::default();
        list.block(ALICE);
        list.block(ALICE);
        list.unblock(&ALICE);
        assert_eq!(list.ingress_gate(&ALICE), Admit::Yes);
    }
}
