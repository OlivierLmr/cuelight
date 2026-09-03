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

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bit stream is part of what a stored scenario *means*: change the generator and
    /// every recorded seed silently starts describing a different run. Pinned on purpose.
    #[test]
    fn stream_is_pinned() {
        let mut r = Rng::new(1);
        let got: Vec<u64> = (0..5).map(|_| r.next_u64()).collect();
        assert_eq!(
            got,
            vec![
                10451216379200822465,
                13757245211066428519,
                17911839290282890590,
                8196980753821780235,
                8195237237126968761,
            ]
        );
    }

    #[test]
    fn same_seed_same_stream() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn range_stays_inside_bounds() {
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            let v = r.range(10, 20);
            assert!((10..20).contains(&v), "{v} outside 10..20");
        }
    }

    #[test]
    #[should_panic(expected = "empty range")]
    fn empty_range_panics() {
        Rng::new(1).range(5, 5);
    }
}
