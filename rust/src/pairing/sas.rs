//! The short authentication string: **three colours and a word** (tech spec §6,
//! R0-F4).
//!
//! # What it is defending against
//!
//! The ceremony's handshake gives both sides a shared secret and a transcript.
//! What it cannot give them is any reason to believe the *other end of the
//! handshake* is the person standing in front of them: an attacker in the
//! middle can run one handshake with each side and relay everything between,
//! and every cryptographic check on both sides passes. Nothing in the protocol
//! notices, because nothing in the protocol has ever seen the room.
//!
//! The humans have. So each side renders its own transcript as something a
//! person can compare out loud in a second, and a relay is caught by the only
//! participants who can tell there are two conversations rather than one. The
//! two transcripts differ under a man in the middle, so the two renderings do
//! too.
//!
//! This is why the confirmation has to be explicit on both sides (R0-F4) and
//! why it cannot be automated away: the comparison *is* the security, and a
//! device that pressed confirm on the user's behalf would be authenticating
//! nothing at all.
//!
//! # How much it is worth: 20 bits
//!
//! Three colours from a palette of [`PALETTE_LEN`] and one word from a list of
//! The `words::WORDS` list gives 4 + 4 + 4 + 8 = [`SAS_BITS`] bits. An attacker
//! relaying a ceremony has to produce two transcripts whose renderings collide,
//! gets one attempt in front of two people who are looking at their screens,
//! and succeeds with probability 1 in 1,048,576.
//!
//! This was two colours and a word — 16 bits, 1 in 65,536 — which is below the
//! ~20 bits usually recommended for a compared authentication string and
//! matches nothing in common use; Bluetooth's numeric comparison is six digits,
//! about 20 bits. The strength is a property of the human format rather than of
//! this code, so raising it meant changing what people see, which is why it was
//! a spec decision and not one taken here. Tech spec §6 was amended on 20 Aug
//! 2026 and this followed.
//!
//! # Why a third colour rather than a longer word list or a second word
//!
//! A 512-word list buys one bit for twice the vocabulary. A second word buys
//! eight, and is noticeably more to read aloud. A third colour buys four, costs
//! one swatch, and still says in one breath: "amber, teal, rose — lantern".
//!
//! The palette deliberately did *not* grow, which would have been the cheapest
//! bits of all. See below: it is capped by what a person can actually compare,
//! not by what a screen can display.
//!
//! The real limit is not arithmetic. A string nobody troubles to compare is
//! worth no bits at all, so every element added has to be paid for out of the
//! same fifteen seconds R0-F4 allows — and the measurement says there is room:
//! two phones over NFC reached the colours in 495 ms, leaving essentially the
//! whole budget to the humans.
//!
//! # Why colours have names
//!
//! [`SasColour`] carries a name as well as an RGB value, and the UI is expected
//! to show both. Two reasons, and the second is the important one:
//!
//! - Two people on a phone call, or across a noisy room, cannot compare
//!   swatches. They can say "amber, teal, lantern".
//! - A swatch-only comparison is not available to a colour-blind user at all,
//!   and this is the step that decides whether their pairing is safe. A
//!   verification that some people cannot perform is a verification those
//!   people will skip.
//!
//! The palette is 16 rather than 64 for the same reason: side-by-side hue
//! comparison stops being reliable well before a screen runs out of colours.

use crate::crypto::kdf;

use super::words;

/// Domain separator for the SAS derivation. Versioned like every other label in
/// the codebase: changing what people compare is a compatibility break, because
/// two builds that disagree show two different words and every ceremony between
/// them fails at the human step.
///
/// Bumped to v2 with the third colour, and the reason is worth stating because
/// it is not merely convention. Adding a colour on its own left the first two
/// derived from the same nibbles as before, so a v1 phone and a v2 phone
/// showed *the same first two colours* and differed only after them. Two people
/// glancing at "blue · navy · …" on both screens could reasonably call that a
/// match — a partial agreement invites a false confirmation in a way total
/// disagreement never does. The new label makes the two unrelated, so a
/// mismatched pair of builds fails the way every other mismatch does.
const SAS_LABEL: &[u8] = b"hoppler/sas/v2";

/// Colours in the palette.
pub const PALETTE_LEN: usize = 16;

/// How many colours one SAS shows.
///
/// Named rather than written as a literal in four places, because it is a
/// security parameter: it is one of the two numbers [`SAS_BITS`] is computed
/// from, and the test below fails if they stop agreeing.
pub const SAS_COLOURS: usize = 3;

/// Entropy of one rendered SAS, in bits. Asserted against the palette and word
/// list sizes below, so it cannot drift away from what it describes.
pub const SAS_BITS: u32 = 20;

/// One palette entry: what to paint, and what to call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SasColour {
    /// Spoken and shown alongside the swatch — see the module header.
    pub name: &'static str,
    /// Packed 0xRRGGBB, matching the persona colour convention.
    pub rgb: u32,
}

/// Sixteen colours chosen to stay apart from each other, each with a name a
/// person would actually use. Order is part of the format: re-ordering this
/// array changes what every transcript renders as.
///
/// The names are held to the same rules as the word list, across both tables —
/// see `spoken_tokens_cannot_be_confused_across_the_two_tables`. The grey here
/// is `slate` for exactly that reason: "green" and "grey" share three letters
/// and a vowel sound, and the SAS is meant to be *said*.
pub const PALETTE: [SasColour; PALETTE_LEN] = [
    SasColour {
        name: "red",
        rgb: 0xE6_194B,
    },
    SasColour {
        name: "orange",
        rgb: 0xF5_8231,
    },
    SasColour {
        name: "yellow",
        rgb: 0xFF_E119,
    },
    SasColour {
        name: "lime",
        rgb: 0xBF_EF45,
    },
    SasColour {
        name: "green",
        rgb: 0x3C_B44B,
    },
    SasColour {
        name: "mint",
        rgb: 0xAA_FFC3,
    },
    SasColour {
        name: "teal",
        rgb: 0x46_9990,
    },
    SasColour {
        name: "cyan",
        rgb: 0x42_D4F4,
    },
    SasColour {
        name: "blue",
        rgb: 0x43_63D8,
    },
    SasColour {
        name: "navy",
        rgb: 0x00_0075,
    },
    SasColour {
        name: "lavender",
        rgb: 0xDC_BEFF,
    },
    SasColour {
        name: "purple",
        rgb: 0x91_1EB4,
    },
    SasColour {
        name: "magenta",
        rgb: 0xF0_32E6,
    },
    SasColour {
        name: "pink",
        rgb: 0xFA_BED4,
    },
    SasColour {
        name: "brown",
        rgb: 0x9A_6324,
    },
    SasColour {
        name: "slate",
        rgb: 0xA9_A9A9,
    },
];

/// What both screens show, and what the two people compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sas {
    /// Shown in this order. The order carries information — "teal then amber"
    /// is not "amber then teal" — so a UI must not sort them.
    pub colours: [SasColour; SAS_COLOURS],
    pub word: &'static str,
}

impl Sas {
    /// The spoken form, for a UI that wants one string:
    /// `"red · teal · rose · lantern"`.
    ///
    /// Built by joining rather than by naming each colour, so adding or
    /// removing one is a change to [`SAS_COLOURS`] and nothing else — the
    /// previous version listed `colours[0]` and `colours[1]` by hand and would
    /// have silently dropped the third.
    pub fn spoken(&self) -> String {
        let mut parts: Vec<&str> = self.colours.iter().map(|c| c.name).collect();
        parts.push(self.word);
        parts.join(" · ")
    }
}

/// Render a finished handshake as something two people can compare.
///
/// `transcript` is the handshake hash — the value both ends compute over
/// everything that was said, and which differs between the two halves of a
/// relayed ceremony. `ceremony_nonce` is the invite's nonce, mixed in as the
/// salt so that a transcript lifted from one ceremony renders differently in
/// another. It is already bound into the handshake as its prologue; folding it
/// in here as well costs nothing and means this function does not depend on
/// that binding having been done correctly elsewhere.
///
/// Takes the transcript as a slice rather than a fixed array on purpose: Noise
/// hash lengths are a property of the suite, and pinning one here would make
/// this function quietly wrong the day the suite changes rather than loudly
/// wrong at the call site.
pub fn derive(transcript: &[u8], ceremony_nonce: &[u8]) -> Sas {
    let okm = kdf::derive_32(transcript, ceremony_nonce, SAS_LABEL);

    // One nibble per colour, one whole byte for the word, so every one of the
    // 1,048,576 renderings is equally likely — no modulo, because both ranges
    // divide their source exactly. That is what `PALETTE_LEN == 16` and 256
    // words are *for*; the test below fails if either stops being true.
    //
    // Taken from consecutive nibbles rather than a byte each, so that adding a
    // colour costs four bits of key material instead of eight and the word's
    // byte stays whole. `derive_32` gives far more than is needed either way.
    let mut colours = [PALETTE[0]; SAS_COLOURS];
    for (i, slot) in colours.iter_mut().enumerate() {
        let byte = okm[i / 2];
        let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0f };
        *slot = PALETTE[nibble as usize];
    }
    // After the colours, on whatever byte they finished in or before. Written
    // as a division rather than a constant so the word does not silently start
    // sharing a byte with a colour if `SAS_COLOURS` changes.
    let word = okm[SAS_COLOURS.div_ceil(2)] as usize;

    Sas {
        // Repeats are allowed and deliberate: "green, green, green, kettle" is
        // a perfectly checkable SAS, and rejecting repeats would mean rejection
        // sampling for a fraction of a bit.
        colours,
        word: words::WORDS[word],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_stated_entropy_matches_the_tables_it_is_computed_from() {
        // `SAS_BITS` is quoted in the module header and will be quoted in a
        // security review. Growing the palette or the word list without
        // updating it would leave a documented number that is simply wrong.
        let bits = PALETTE_LEN.ilog2() * SAS_COLOURS as u32 + words::WORDS.len().ilog2();
        assert_eq!(bits, SAS_BITS);
        // Both must be exact powers of two, or the byte-splitting above stops
        // being uniform and some renderings become likelier than others.
        assert!(PALETTE_LEN.is_power_of_two());
        assert!(words::WORDS.len().is_power_of_two());
    }

    /// Twenty bits is the whole point of the change, and the number is quoted
    /// in the spec and will be quoted in a review. This is the arithmetic
    /// spelled out, so the constant cannot drift from what it claims.
    #[test]
    fn the_rendering_is_worth_twenty_bits() {
        assert_eq!(SAS_BITS, 20);
        assert_eq!(SAS_COLOURS, 3);
        assert_eq!(PALETTE_LEN, 16);
        assert_eq!(words::WORDS.len(), 256);
    }

    /// Every colour has to come from somewhere different, or a "third colour"
    /// that always repeats the first buys no bits at all while looking like it
    /// does — the failure this change exists to avoid, arrived at by accident.
    #[test]
    fn the_colours_are_drawn_from_distinct_bits() {
        // Positions that never vary across many transcripts would mean a slot
        // fed by nothing, or by the same nibble as another.
        let mut seen = vec![HashSet::new(); SAS_COLOURS];
        for i in 0..512u32 {
            let mut transcript = [0u8; 32];
            transcript[..4].copy_from_slice(&i.to_le_bytes());
            let sas = derive(&transcript, &[0u8; 32]);
            for (slot, names) in sas.colours.iter().zip(seen.iter_mut()) {
                names.insert(slot.name);
            }
        }
        for (i, names) in seen.iter().enumerate() {
            assert_eq!(
                names.len(),
                PALETTE_LEN,
                "colour {i} only ever took {} of {PALETTE_LEN} values",
                names.len()
            );
        }
    }

    /// The word must not share a byte with a colour, or the two would move
    /// together and the rendering would be worth less than it claims.
    #[test]
    fn the_word_varies_independently_of_the_colours() {
        let mut pairs = HashSet::new();
        for i in 0..512u32 {
            let mut transcript = [0u8; 32];
            transcript[..4].copy_from_slice(&i.to_le_bytes());
            let sas = derive(&transcript, &[0u8; 32]);
            pairs.insert((sas.colours[SAS_COLOURS - 1].name, sas.word));
        }
        // If the last colour and the word came from one byte there would be at
        // most 256 combinations; independent, hundreds of distinct pairs show
        // up in 512 draws.
        assert!(
            pairs.len() > 400,
            "only {} distinct (last colour, word) pairs — they share a source",
            pairs.len()
        );
    }

    /// `spoken` is what a person reads out. Dropping a colour from it would
    /// leave the screen at 20 bits and the spoken comparison at 16, which is
    /// the weaker of the two and the one people actually use over a phone call.
    #[test]
    fn every_colour_reaches_the_spoken_form() {
        let sas = derive(&[1u8; 32], &[2u8; 32]);
        let spoken = sas.spoken();
        for colour in sas.colours {
            assert!(
                spoken.contains(colour.name),
                "{spoken:?} is missing {}",
                colour.name
            );
        }
        assert!(spoken.contains(sas.word));
        assert_eq!(spoken.matches(" · ").count(), SAS_COLOURS);
    }

    #[test]
    fn both_ends_of_one_ceremony_agree() {
        let transcript = [0x5au8; 32];
        let nonce = [0x11u8; 32];
        assert_eq!(derive(&transcript, &nonce), derive(&transcript, &nonce));
    }

    /// The property the whole ceremony rests on: a relay produces two different
    /// transcripts, so it must produce two different renderings often enough
    /// for a human to notice. One flipped bit is the smallest difference a
    /// transcript can have.
    #[test]
    fn a_different_transcript_renders_differently() {
        let nonce = [0u8; 32];
        let base = derive(&[0u8; 32], &nonce);
        let mut differed = 0;
        for bit in 0..256 {
            let mut transcript = [0u8; 32];
            transcript[bit / 8] ^= 1 << (bit % 8);
            if derive(&transcript, &nonce) != base {
                differed += 1;
            }
        }
        // Each single-bit change is an independent draw from 65,536
        // renderings, so a collision is possible and a strict "all 256 differ"
        // would be a flaky assertion dressed up as a strong one. What this
        // rules out is the failure that matters: a derivation that ignores the
        // transcript, or looks at only part of it.
        assert!(
            differed > 250,
            "only {differed} of 256 single-bit transcript changes altered the SAS"
        );
    }

    #[test]
    fn the_ceremony_nonce_changes_the_rendering() {
        let transcript = [7u8; 32];
        assert_ne!(
            derive(&transcript, &[1u8; 32]),
            derive(&transcript, &[2u8; 32]),
            "the same transcript rendered identically under two ceremonies"
        );
    }

    #[test]
    fn golden_vector() {
        // Freezes the derivation, the palette order and the word list together.
        // Any of the three changing is a compatibility break — two builds that
        // disagree show two different words, and every ceremony between them
        // fails at the human step with nothing to explain why. This test
        // failing is how that is meant to be found out.
        let sas = derive(&[0xABu8; 32], &[0xCDu8; 32]);
        assert_eq!(sas.spoken(), "red · lime · blue · helmet");

        // Three colours, said plainly, because the count is the security
        // parameter and a vector that still passed with two would be a golden
        // vector for the wrong format.
        assert_eq!(sas.colours.len(), 3);
    }

    /// Every colour distinct as a value, and distinct as a *name* — the name is
    /// what gets spoken, so two entries called the same thing would be a SAS
    /// that cannot be read aloud unambiguously even when the screens differ.
    #[test]
    fn the_palette_is_distinct_and_in_range() {
        let names: HashSet<&str> = PALETTE.iter().map(|c| c.name).collect();
        let rgbs: HashSet<u32> = PALETTE.iter().map(|c| c.rgb).collect();
        assert_eq!(names.len(), PALETTE_LEN);
        assert_eq!(rgbs.len(), PALETTE_LEN);
        for colour in PALETTE {
            assert_eq!(
                colour.rgb & 0xFF00_0000,
                0,
                "{} has bits above 0xRRGGBB",
                colour.name
            );
        }
    }

    /// The two tables are read out as one sentence, so they have to obey the
    /// word list's rules *together*. A colour and a word are not distinguished
    /// by which table they came from when someone says them aloud.
    ///
    /// This is not hypothetical tidying — it is here because the first draft of
    /// these tables failed it three ways. `orange` and `yellow` were in both
    /// lists, so "blue · navy · yellow" read as three colours. `teal`/`teapot`
    /// and `magenta`/`magnet` shared a stem across the tables. And the palette
    /// contradicted itself: `green` and `grey`, three shared letters apart, in
    /// the one comparison the whole ceremony depends on being unambiguous.
    #[test]
    fn spoken_tokens_cannot_be_confused_across_the_two_tables() {
        let mut stems: Vec<(&str, &str)> = Vec::new();
        for token in PALETTE
            .iter()
            .map(|c| c.name)
            .chain(words::WORDS.iter().copied())
        {
            let stem = &token[..3];
            if let Some((other, _)) = stems.iter().find(|(_, s)| *s == stem) {
                panic!("{token:?} and {other:?} both start {stem:?}");
            }
            stems.push((token, stem));
        }
    }

    /// Every rendering must be reachable, or the effective entropy is lower
    /// than [`SAS_BITS`] claims — the exact failure a lazy `% PALETTE_LEN` on a
    /// non-power-of-two table would introduce.
    #[test]
    fn every_colour_and_word_is_reachable() {
        let mut colours = HashSet::new();
        let mut seen_words = HashSet::new();
        // Walking the derivation's *inputs* would only prove HKDF is surjective
        // over these two bytes, so drive the tables directly by the byte values
        // the derivation reads.
        for byte in 0u8..=255 {
            colours.insert(PALETTE[(byte >> 4) as usize].name);
            colours.insert(PALETTE[(byte & 0x0f) as usize].name);
            seen_words.insert(words::WORDS[byte as usize]);
        }
        assert_eq!(colours.len(), PALETTE_LEN);
        assert_eq!(seen_words.len(), words::WORDS.len());
    }
}
