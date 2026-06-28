use crate::belief::Belief;
use crate::chance::OpenLoop;
use crate::determinize::{Determinizer, Observation};
use crate::eval::Handcrafted;
use crate::rng::{splitmix64, Lcg};
use crate::search::{search_world, ArmStat, SearchParams};
use pkmn_engine::state::*;

pub struct PimcConfig {
    pub num_worlds: usize,
    pub time_ms_per_world: u64,
    // Finite cap => same seed -> same choice (§10); u64::MAX = wall-clock-only, not reproducible.
    pub max_iters_per_world: u64,
    pub seed: u64,
}

/// (action_byte, aggregated visit fraction), sorted desc — exposed for tests/logging.
pub fn aggregate(per_world: &[(Vec<ArmStat>, f64)]) -> Vec<(u8, f64)> {
    let mut frac: std::collections::HashMap<u8, f64> = std::collections::HashMap::new();
    for (stats, weight) in per_world {
        let total: u64 = stats.iter().map(|a| a.visits as u64).sum();
        if total == 0 { continue; }
        for a in stats {
            *frac.entry(a.action).or_insert(0.0) += weight * a.visits as f64 / total as f64;
        }
    }
    let mut v: Vec<(u8, f64)> = frac.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    v
}

pub fn pick_from(aggregated: &[(u8, f64)], legal: &ActionList, rng: &mut Lcg) -> u8 {
    let legal_slice = legal.as_slice();
    let survivors: Vec<(u8, f64)> = {
        let best = aggregated.iter().filter(|(a, _)| legal_slice.contains(a)).map(|x| x.1).fold(0.0, f64::max);
        aggregated.iter()
            .filter(|(a, f)| legal_slice.contains(a) && *f >= 0.75 * best)
            .cloned().collect()
    };
    if survivors.is_empty() { return legal_slice.first().copied().unwrap_or(ACTION_STRUGGLE); }
    // weighted-random among survivors, integer arithmetic over the rng seam
    let scale = 1_000_000.0;
    let total: u64 = survivors.iter().map(|(_, f)| (f * scale) as u64 + 1).sum();
    let mut r = ((rng.roll(u32::MAX) as u64) << 32 | rng.roll(u32::MAX) as u64) % total;
    for (a, f) in &survivors {
        let w = (f * scale) as u64 + 1;
        if r < w { return *a; }
        r -= w;
    }
    survivors[0].0
}

/// (num_worlds, time_ms_per_world). Time-pressure ladder clock deferred to sub-project 2.
pub fn adaptive_budget(
    revealed_mons: usize,
    active_moves_revealed: usize,
    parallelism: usize,
    base_time_ms: u64,
) -> (usize, u64) {
    if revealed_mons <= 3 && active_moves_revealed == 0 {
        (parallelism * 4, base_time_ms / 2)
    } else {
        (parallelism * 2, base_time_ms)
    }
}

pub fn choose_action(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig) -> u8 {
    let legal = legal_actions(obs.state, obs.our_side);
    if legal.count == 0 { return ACTION_STRUGGLE; }
    if legal.count == 1 { return legal.actions[0]; }
    let mut rng = Lcg::new(splitmix64(cfg.seed));
    let worlds = det.sample_worlds(obs, belief, cfg.num_worlds, &mut rng);
    let params = SearchParams {
        time_ms: cfg.time_ms_per_world,
        max_iters: cfg.max_iters_per_world,
        ..Default::default()
    };
    // M3 swaps this map to rayon par_iter (Task 18); keep the shape identical
    let per_world: Vec<(Vec<ArmStat>, f64)> = worlds.iter().enumerate().map(|(k, w)| {
        let seed = splitmix64(cfg.seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let r = search_world(&w.state, &w.teams, &Handcrafted, &OpenLoop, &params, seed);
        (r.side(obs.our_side).to_vec(), w.weight)
    }).collect();
    let agg = aggregate(&per_world);
    pick_from(&agg, &legal, &mut rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::determinize::RandomBattle;
    use crate::testutil::*;

    #[test]
    fn adaptive_budget_rule() {
        // few reveals + virgin active -> many shallow worlds; else fewer, deeper
        assert_eq!(adaptive_budget(2, 0, 8, 100), (32, 50));
        assert_eq!(adaptive_budget(2, 1, 8, 100), (16, 100));
        assert_eq!(adaptive_budget(5, 0, 8, 100), (16, 100));
    }

    fn arms(v: &[(u8, u32)]) -> Vec<ArmStat> {
        v.iter().map(|&(a, n)| ArmStat { action: a, visits: n, avg_score: 0.5 }).collect()
    }

    #[test]
    fn aggregation_weights_and_filters() {
        // world A: 80/20 on bytes 0/1 ; world B: 10/90 on bytes 1/4 — equal weights
        let agg = aggregate(&[
            (arms(&[(0, 80), (1, 20)]), 0.5),
            (arms(&[(1, 10), (4, 90)]), 0.5),
        ]);
        let f = |b: u8| agg.iter().find(|x| x.0 == b).map(|x| x.1).unwrap_or(0.0);
        assert!((f(0) - 0.40).abs() < 1e-9);
        assert!((f(1) - 0.15).abs() < 1e-9);
        assert!((f(4) - 0.45).abs() < 1e-9);
        assert_eq!(agg[0].0, 4, "sorted desc");
    }

    #[test]
    fn pick_respects_75pct_filter_and_legality() {
        let mut legal = ActionList::new();
        for a in [0u8, 1, 4] { let i = legal.count as usize; legal.actions[i] = a; legal.count += 1; }
        let agg = vec![(4u8, 0.45), (0u8, 0.40), (1u8, 0.15), (9u8, 0.44)];
        let mut rng = Lcg::new(1);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 { seen.insert(pick_from(&agg, &legal, &mut rng)); }
        assert!(seen.contains(&4) && seen.contains(&0), "both >=75%-of-best survivors picked sometimes");
        assert!(!seen.contains(&1), "0.15 < 0.75*0.45 filtered");
        assert!(!seen.contains(&9), "illegal byte masked even with high fraction");
    }

    #[test]
    fn choose_action_end_to_end_hidden_info() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        let mut belief = Belief::default();
        belief.note_species(445, s.sides[1].team[0].level);
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        // Iteration-bounded; the clock is only a safety ceiling.
        let cfg = PimcConfig { num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 9 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert!(legal_actions(&s, 0).as_slice().contains(&a));
        let b = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert_eq!(a, b, "same seed -> same choice (reproducibility, design §10)");
    }
}
