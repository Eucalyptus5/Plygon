pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Same LCG as the harness `sampled_rng` (run_scenario main.rs:883-891).
pub struct Lcg(pub u64);

impl Lcg {
    pub fn new(seed: u64) -> Self { Lcg(splitmix64(seed)) }
    #[inline(always)]
    pub fn roll(&mut self, max: u32) -> u32 {
        self.0 = self.0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % max.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lcg_deterministic_and_in_range() {
        let mut a = Lcg::new(42);
        let mut b = Lcg::new(42);
        for _ in 0..1000 {
            let (x, y) = (a.roll(16), b.roll(16));
            assert_eq!(x, y);
            assert!(x < 16);
        }
        assert_eq!(Lcg::new(42).roll(0), 0); // max=0 clamps, no div-by-zero
    }
    #[test]
    fn splitmix_decorrelates_neighbor_seeds() {
        // raw sequential seeds correlate the LCG; mixed seeds must differ immediately
        let (mut a, mut b) = (Lcg::new(1), Lcg::new(2));
        let same = (0..64).filter(|_| a.roll(1 << 30) == b.roll(1 << 30)).count();
        assert!(same < 4);
    }
}
