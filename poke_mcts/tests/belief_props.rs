use poke_mcts::belief::{pm_set, possible, set_consistent, MonBelief};
use poke_mcts::gen_sets::{SetEntry, GEN9_SET_POOL};

struct SplitMix64(u64);
impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

// sample a true set the way production does: a species, then a set weighted by count
fn sample_true_set(rng: &mut SplitMix64) -> &'static SetEntry {
    let pool = &GEN9_SET_POOL[(rng.next() as usize) % GEN9_SET_POOL.len()];
    let total: u32 = pool.sets.iter().map(|s| s.count).sum();
    let mut r = (rng.next() % total.max(1) as u64) as u32;
    for s in pool.sets {
        if r < s.count {
            return s;
        }
        r -= s.count;
    }
    &pool.sets[pool.sets.len() - 1]
}

// a single curated bit (positions 0..20) that `infer_bits` does NOT carry
fn a_bit_not_in(infer_bits: u32, rng: &mut SplitMix64) -> u32 {
    loop {
        let p = (rng.next() % 20) as u32;
        if (infer_bits >> p) & 1 == 0 {
            return 1 << p;
        }
    }
}

#[test]
fn soundness_never_excludes_true_set_under_random_exclusions() {
    for case in 0..200_000u64 {
        let mut rng = SplitMix64::new(0xB3F1 ^ case);
        let s = sample_true_set(&mut rng);
        // soundness: excluding a bit S does NOT carry must never exclude S
        let mut b = MonBelief::default();
        b.excluded_bits = a_bit_not_in(s.infer_bits, &mut rng);
        assert!(set_consistent(s, &b), "case {case}: true set wrongly excluded");
        // dual-negative: excluding a bit S DOES carry must reject S (guards a no-op masquerade)
        if s.infer_bits != 0 {
            let mut b2 = MonBelief::default();
            b2.excluded_bits = s.infer_bits & s.infer_bits.wrapping_neg(); // lowest set bit
            assert!(!set_consistent(s, &b2), "case {case}: overlapping set not rejected");
        }
    }
}

#[test]
fn possible_pool_mask_respects_word_boundaries() {
    // default set: infer_bits=0, no reveals -> set_consistent always true, so possible() reduces to
    // the mask half. The boundary sweep catches an i>>5/i&31 off-by-one (correct for one word, wrong
    // for the 256-bit [u64;4]).
    let s = SetEntry::default();
    let bounds = [0usize, 63, 64, 127, 128, 191, 192, 255];

    // inactive mask: every index possible regardless of mask contents
    let inactive = MonBelief::default();
    for &i in &bounds {
        assert!(possible(i, &s, &inactive), "inactive mask must allow index {i}");
    }

    // active mask: exactly the masked-in index survives; an index in a different word does not
    for &i in &bounds {
        let mut b = MonBelief::default();
        b.pool_active = true;
        pm_set(&mut b.pool_mask, i);
        assert!(possible(i, &s, &b), "masked-in index {i} must be possible");
        let other = if i == 0 { 255 } else { 0 };
        assert!(!possible(other, &s, &b), "masked-out index {other} must be impossible (set i={i})");
    }
}
