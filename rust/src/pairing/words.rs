//! The v0 SAS word list — 256 words, one per byte.
//!
//! The word is the third of the three things two people compare during the
//! ceremony, and the only one they can read out over the noise of a room. That
//! is what the selection rules are for; they are enforced by the tests at the
//! bottom of this file, not merely intended.
//!
//! - **Exactly 256, so one byte selects one word.** No modulo, so no word is
//!   more likely than any other.
//! - **No two words share their first three letters.** Half-heard is the normal
//!   case — "did you say *linen* or *lilac*?" — and a shared stem is exactly
//!   where a mishearing turns into a false match rather than a re-ask.
//! - **Concrete and picturable.** Objects, animals, plants. Abstract nouns are
//!   harder to hold in mind for the seconds between hearing and comparing.
//! - **Nothing alarming.** These words appear at the moment two people are
//!   deciding to trust each other; a list with `poison` in it makes a
//!   successful ceremony read like a warning.
//! - **British spellings** where the two differ, matching the rest of the
//!   codebase (`colour`). Spelling is invisible to a spoken comparison and only
//!   matters for the on-screen text.
//!
//! Known limit, stated because it will be asked: the list is English. A
//! ceremony between two people who share no English is not served by it, and
//! localising a SAS list is harder than translating it — the words have to stay
//! distinct *within* the target language, and both devices must choose the same
//! list without negotiating it over the very channel being authenticated. That
//! is out of Ring 0's scope, and picking `v1` in the label above is what leaves
//! room for it.

/// One word per byte value. Index is the byte; order is therefore part of the
/// format and must never be re-sorted.
pub const WORDS: [&str; 256] = [
    "acorn", "album", "amber", "anchor", "ankle", "apple", "arrow", "atlas", "autumn", "bacon",
    "badge", "balloon", "bamboo", "banjo", "barrel", "basket", "beacon", "bishop", "blanket",
    "bottle", "branch", "bridge", "bucket", "button", "cabin", "cactus", "camel", "candle",
    "carpet", "castle", "cellar", "chapel", "cherry", "circus", "clover", "cobra", "coffee",
    "copper", "cotton", "cricket", "crystal", "cushion", "daisy", "dancer", "dentist", "desert",
    "diamond", "dinner", "dolphin", "donkey", "dragon", "drum", "dust", "eagle", "echo", "edge",
    "elbow", "elephant", "ember", "engine", "evening", "fabric", "falcon", "farmer", "feather",
    "fiddle", "finger", "fire", "flame", "flower", "forest", "fountain", "fox", "garden",
    "giraffe", "glacier", "glove", "golden", "granite", "guitar", "gull", "hammer", "harbour",
    "hazel", "helmet", "hermit", "hollow", "honey", "horizon", "hunter", "igloo", "indigo",
    "insect", "island", "ivory", "jacket", "jaguar", "jelly", "jewel", "jigsaw", "jockey",
    "jungle", "kayak", "kennel", "kettle", "kingdom", "kitchen", "koala", "ladder", "lagoon",
    "lantern", "laurel", "lemon", "leopard", "lettuce", "lilac", "linen", "lobster", "lotus",
    "lumber", "mammoth", "mango", "maple", "marble", "meadow", "melon", "mirror", "monkey",
    "mosaic", "mountain", "muffin", "mustard", "napkin", "nectar", "needle", "nest", "nickel",
    "noodle", "nutmeg", "oasis", "ocean", "octopus", "olive", "onion", "opal", "orchid", "ostrich",
    "otter", "oxygen", "oyster", "paddle", "palace", "panda", "parrot", "peach", "pebble",
    "pelican", "pencil", "pepper", "piano", "pigeon", "pillow", "pirate", "planet", "plum",
    "pocket", "pony", "potato", "prism", "pumpkin", "puzzle", "quarry", "queen", "quilt", "rabbit",
    "radish", "rainbow", "ranger", "raven", "ribbon", "river", "robot", "rocket", "rooster",
    "rubber", "saddle", "salmon", "sandal", "scarf", "seagull", "shadow", "shelter", "shovel",
    "signal", "silver", "sketch", "slipper", "smoke", "snorkel", "socket", "spider", "spoon",
    "spring", "squirrel", "stadium", "stone", "sugar", "sunset", "swallow", "sweater", "table",
    "tadpole", "tavern", "temple", "thunder", "tiger", "timber", "toast", "tomato", "tower",
    "trumpet", "tulip", "tunnel", "turtle", "umbrella", "unicorn", "valley", "vanilla", "velvet",
    "vessel", "village", "violet", "volcano", "voyage", "waffle", "wagon", "walnut", "wander",
    "wasp", "water", "weasel", "whale", "wheat", "whisker", "willow", "window", "wizard", "wolf",
    "wonder", "yacht", "yarn", "yeast", "yogurt", "zebra", "zigzag", "zipper",
];

#[cfg(test)]
mod tests {
    use super::WORDS;
    use std::collections::HashSet;

    /// The rules in the module header, checked rather than trusted. Every one of
    /// these is a property an edit to the list can silently break — and an edit
    /// is likely, because the list is the part of the ceremony most obviously
    /// open to opinion.
    #[test]
    fn every_word_is_distinct() {
        let unique: HashSet<&str> = WORDS.iter().copied().collect();
        assert_eq!(unique.len(), WORDS.len(), "the list repeats a word");
    }

    #[test]
    fn no_two_words_share_their_first_three_letters() {
        let mut seen: HashSet<&str> = HashSet::new();
        for word in WORDS {
            let stem = &word[..3];
            assert!(
                seen.insert(stem),
                "{word:?} shares the stem {stem:?} with an earlier word"
            );
        }
    }

    #[test]
    fn every_word_is_short_lowercase_ascii() {
        for word in WORDS {
            assert!(
                (3..=8).contains(&word.len()),
                "{word:?} is {} characters",
                word.len()
            );
            assert!(
                word.chars().all(|c| c.is_ascii_lowercase()),
                "{word:?} is not plain lowercase ASCII"
            );
        }
    }

    /// Alphabetical so a reviewer can find a word without reading all 256.
    ///
    /// It carries no meaning beyond that, and in particular it is not what
    /// freezes the list: *any* edit — a swap, an insertion, a re-sort — changes
    /// which word a given transcript produces, which is a wire-visible break.
    /// The golden vector in `sas.rs` is what catches that. This only keeps the
    /// list legible enough to review.
    #[test]
    fn the_list_is_in_alphabetical_order() {
        let mut sorted = WORDS;
        sorted.sort_unstable();
        assert_eq!(WORDS, sorted, "the list is out of order");
    }
}
