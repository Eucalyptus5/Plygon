use crate::node::Bandit;

// `explore_coeff` is the coefficient inside the UCB sqrt (c²). Default 2.0 = textbook c=√2,
// preserving the prior `(2.0 * ln_n / n).sqrt()` bit-for-bit.
pub const C_PUCT: f64 = 1.0;

pub fn select_arm(bandit: &Bandit, parent_visits: u32, explore_coeff: f64, prior: Option<&[f32]>) -> usize {
    debug_assert!(bandit.len > 0);
    if let Some(p) = prior {
        // no first-visit shortcut here: ordering the unvisited arms is what the prior is for
        let sqrt_n = (parent_visits as f64).sqrt();
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for i in 0..bandit.len as usize {
            let a = &bandit.arms[i];
            let q = if a.visits > 0 { a.total_score / a.visits as f64 } else { 0.0 };
            // an action byte outside the slice (ACTION_STRUGGLE = 255) scores no prior mass
            let pa = p.get(a.action as usize).copied().unwrap_or(0.0) as f64;
            let v = q + C_PUCT * pa * sqrt_n / (1.0 + a.visits as f64);
            if v > best_v { best_v = v; best = i; }
        }
        return best;
    }
    let ln_n = (parent_visits.max(1) as f64).ln();
    let mut best = 0usize;
    let mut best_v = f64::NEG_INFINITY;
    for i in 0..bandit.len as usize {
        let a = &bandit.arms[i];
        if a.visits == 0 { return i; }
        let v = a.total_score / a.visits as f64 + (explore_coeff * ln_n / a.visits as f64).sqrt();
        if v > best_v { best_v = v; best = i; }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::MoveNode;
    use pkmn_engine::state::ACTION_STRUGGLE;

    fn bandit(arms: &[(f64, u32)]) -> Bandit {
        let mut b = Bandit::default();
        for (i, &(score, visits)) in arms.iter().enumerate() {
            b.arms[i] = MoveNode { action: i as u8, total_score: score, visits };
            b.len += 1;
        }
        b
    }

    fn acted(arms: &[(u8, f64, u32)]) -> Bandit {
        let mut b = Bandit::default();
        for (i, &(action, total_score, visits)) in arms.iter().enumerate() {
            b.arms[i] = MoveNode { action, total_score, visits };
            b.len += 1;
        }
        b
    }

    // verbatim copy of the pre-prior body; the None branch is asserted against it
    fn ucb1_reference(bandit: &Bandit, parent_visits: u32, explore_coeff: f64) -> usize {
        let ln_n = (parent_visits.max(1) as f64).ln();
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for i in 0..bandit.len as usize {
            let a = &bandit.arms[i];
            if a.visits == 0 { return i; }
            let v = a.total_score / a.visits as f64 + (explore_coeff * ln_n / a.visits as f64).sqrt();
            if v > best_v { best_v = v; best = i; }
        }
        best
    }

    fn puct_reference(bandit: &Bandit, parent_visits: u32, prior: &[f32]) -> usize {
        let sqrt_n = (parent_visits as f64).sqrt();
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for i in 0..bandit.len as usize {
            let a = &bandit.arms[i];
            let q = if a.visits > 0 { a.total_score / a.visits as f64 } else { 0.0 };
            let p = prior.get(a.action as usize).copied().unwrap_or(0.0) as f64;
            let v = q + C_PUCT * p * sqrt_n / (1.0 + a.visits as f64);
            if v > best_v { best_v = v; best = i; }
        }
        best
    }

    struct Xs(u64);
    impl Xs {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn unvisited_arm_selected_first() {
        let b = bandit(&[(10.0, 20), (0.0, 0), (5.0, 10)]);
        assert_eq!(select_arm(&b, 30, 2.0, None), 1);
    }

    #[test]
    fn exploit_dominant_arm() {
        let b = bandit(&[(0.9 * 100.0, 100), (0.1 * 100.0, 100)]);
        assert_eq!(select_arm(&b, 200, 2.0, None), 0);
    }

    #[test]
    fn explore_undervisited_arm() {
        // near-equal averages, one arm starved: UCB bonus must flip the pick
        let b = bandit(&[(0.50 * 1000.0, 1000), (0.49 * 2.0, 2)]);
        assert_eq!(select_arm(&b, 1002, 2.0, None), 1);
    }

    #[test]
    fn no_prior_matches_the_ucb1_reference() {
        let mut r = Xs(0x243F_6A88_85A3_08D3);
        for _ in 0..4000 {
            let n = (r.next() % 13 + 1) as usize;
            let mut b = Bandit::default();
            for i in 0..n {
                let visits = (r.next() % 40) as u32;
                let total_score = (r.next() % 1_000_001) as f64 / 1e6 * visits as f64;
                b.arms[i] = MoveNode { action: (r.next() % 14) as u8, total_score, visits };
                b.len += 1;
            }
            let parent = (r.next() % 5000) as u32;
            let coeff = [0.49f64, 1.0, 2.0][(r.next() % 3) as usize];
            assert_eq!(
                select_arm(&b, parent, coeff, None),
                ucb1_reference(&b, parent, coeff),
                "None branch must be the untouched UCB1 body"
            );
        }
    }

    #[test]
    fn prior_matches_the_puct_reference() {
        let mut r = Xs(0xB5AD_4ECE_DA1C_E2A9);
        for _ in 0..4000 {
            let n = (r.next() % 13 + 1) as usize;
            let mut b = Bandit::default();
            for i in 0..n {
                let visits = (r.next() % 40) as u32;
                let total_score = (r.next() % 1_000_001) as f64 / 1e6 * visits as f64;
                b.arms[i] = MoveNode { action: (r.next() % 14) as u8, total_score, visits };
                b.len += 1;
            }
            let mut prior = [0.0f32; 14];
            for p in prior.iter_mut() {
                *p = (r.next() % 1_000_001) as f32 / 1e6;
            }
            let parent = (r.next() % 5000) as u32;
            assert_eq!(
                select_arm(&b, parent, 2.0, Some(&prior)),
                puct_reference(&b, parent, &prior),
                "prior branch must score Q + C_PUCT * P * sqrt(N) / (1 + n)"
            );
        }
    }

    #[test]
    fn prior_orders_unvisited_arms() {
        let b = acted(&[(0, 0.0, 0), (1, 0.0, 0), (2, 0.0, 0)]);
        let mut prior = [0.0f32; 14];
        prior[0] = 0.1;
        prior[1] = 0.2;
        prior[2] = 0.7;
        assert_eq!(select_arm(&b, 9, 2.0, Some(&prior)), 2);
        assert_eq!(select_arm(&b, 9, 2.0, None), 0, "UCB1 keeps its first-visit shortcut");
    }

    #[test]
    fn prior_branch_has_no_first_visit_shortcut() {
        // arm 0 unvisited with a near-zero prior; arm 1 visited, high Q, high prior
        let b = acted(&[(0, 0.0, 0), (1, 90.0, 100)]);
        let mut prior = [0.0f32; 14];
        prior[0] = 0.001;
        prior[1] = 0.999;
        assert_eq!(select_arm(&b, 100, 2.0, Some(&prior)), 1);
        assert_eq!(select_arm(&b, 100, 2.0, None), 0);
    }

    #[test]
    fn prior_zero_visit_arm_scores_q_zero() {
        // arm 2 carries a negative total_score at zero visits: Q must read 0.0,
        // not total_score/0 (which would sink it to -inf and hand arm 1 the pick)
        let b = acted(&[(0, 0.0, 0), (1, 5.0, 10), (2, -50.0, 0)]);
        let mut prior = [0.0f32; 14];
        prior[0] = 0.01;
        prior[2] = 0.9;
        // arm0: 0 + 0.01*10/1 = 0.1 ; arm1: 0.5 + 0 ; arm2: 0 + 0.9*10/1 = 9
        assert_eq!(select_arm(&b, 100, 2.0, Some(&prior)), 2);
    }

    #[test]
    fn prior_lookup_out_of_range_scores_zero() {
        let mut prior = [0.0f32; 14];
        prior[1] = 1.0;
        let b = acted(&[(ACTION_STRUGGLE, 0.0, 0), (1, 0.0, 0)]);
        assert_eq!(select_arm(&b, 16, 2.0, Some(&prior)), 1, "byte 255 must score P = 0");
        let only = acted(&[(ACTION_STRUGGLE, 0.0, 0)]);
        assert_eq!(select_arm(&only, 16, 2.0, Some(&prior)), 0, "no panic on a lone out-of-range byte");
        let short = acted(&[(0, 0.0, 0), (1, 0.0, 0)]);
        assert_eq!(select_arm(&short, 16, 2.0, Some(&prior[..1])), 0, "short slice must not panic");
    }
}
