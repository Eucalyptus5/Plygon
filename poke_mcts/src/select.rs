use crate::node::Bandit;

// `explore_coeff` is the coefficient inside the UCB sqrt (c²). Default 2.0 = textbook c=√2,
// preserving the prior `(2.0 * ln_n / n).sqrt()` bit-for-bit.
pub fn select_arm(bandit: &Bandit, parent_visits: u32, explore_coeff: f64) -> usize {
    debug_assert!(bandit.len > 0);
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

    fn bandit(arms: &[(f64, u32)]) -> Bandit {
        let mut b = Bandit::default();
        for (i, &(score, visits)) in arms.iter().enumerate() {
            b.arms[i] = MoveNode { action: i as u8, total_score: score, visits };
            b.len += 1;
        }
        b
    }

    #[test]
    fn unvisited_arm_selected_first() {
        let b = bandit(&[(10.0, 20), (0.0, 0), (5.0, 10)]);
        assert_eq!(select_arm(&b, 30, 2.0), 1);
    }

    #[test]
    fn exploit_dominant_arm() {
        let b = bandit(&[(0.9 * 100.0, 100), (0.1 * 100.0, 100)]);
        assert_eq!(select_arm(&b, 200, 2.0), 0);
    }

    #[test]
    fn explore_undervisited_arm() {
        // near-equal averages, one arm starved: UCB bonus must flip the pick
        let b = bandit(&[(0.50 * 1000.0, 1000), (0.49 * 2.0, 2)]);
        assert_eq!(select_arm(&b, 1002, 2.0), 1);
    }
}
