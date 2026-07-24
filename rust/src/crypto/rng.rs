//! CSPRNG access. The only randomness source in the codebase.

/// Fill `buf` with cryptographically secure random bytes.
///
/// Panics if the OS RNG is unavailable — that is an unrecoverable
/// environment failure, not a condition to handle.
pub fn fill(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS RNG unavailable");
}

/// A fresh random array of `N` bytes.
pub fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    fill(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_is_not_constant() {
        // Not a randomness-quality test — only that bytes were written and
        // two draws differ (a 2^-256 false-failure probability is fine).
        let a: [u8; 32] = random_array();
        let b: [u8; 32] = random_array();
        assert_ne!(a, b);
    }
}
