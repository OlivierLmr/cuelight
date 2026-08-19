//! SplitMix64.
//!
//! Deliberately hand-rolled rather than pulling in `rand`: the exact bit stream is part of the
//! meaning of a stored scenario. If the generator ever changed, every recorded scenario would
//! silently start describing a different run.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi)`. Panics if the range is empty.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        assert!(hi > lo, "empty range {lo}..{hi}");
        lo + self.next_u64() % (hi - lo)
    }
}
