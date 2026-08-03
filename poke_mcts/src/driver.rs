use crate::belief::Belief;
use crate::chance::OpenLoop;
use crate::chance_analytic::AnalyticRoot;
use crate::chance_closed::search_world_closed;
use crate::determinize::{Determinizer, Observation, World};
use crate::eval::{Evaluator, Handcrafted};
use crate::eval_learned::LearnedEval;
use crate::rng::{splitmix64, Lcg};
use crate::search::{closed_loop_max_nodes, search_world, ArmStat, ChanceMode, SearchParams, SearchResult};
use pkmn_engine::state::*;
use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PickMode { #[default] Weighted, Argmax, Value }

/// Leaf evaluator for one decision. Carries a borrow of the once-constructed
/// LearnedEval so every world's search shares the same weights.
#[derive(Clone, Copy, Default)]
pub enum EvalKind<'a> {
    #[default]
    Handcrafted,
    Learned(&'a LearnedEval),
}

impl EvalKind<'_> {
    pub fn name(self) -> &'static str {
        match self { EvalKind::Handcrafted => "handcrafted", EvalKind::Learned(_) => "learned" }
    }
}

pub struct PimcConfig {
    pub num_worlds: usize,
    pub time_ms_per_world: u64,
    // Finite cap => same seed -> same choice (§10); u64::MAX = wall-clock-only, not reproducible.
    pub max_iters_per_world: u64,
    pub seed: u64,
    pub chance_mode: ChanceMode,
    pub pick_mode: PickMode,
    pub filter_threshold: f64,
    pub raw_root: bool,
    pub explore_coeff: f64, // UCB sqrt coefficient (c²); 2.0 = textbook c=√2
}

/// (action_byte, aggregated visit fraction), sorted desc — exposed for tests/logging.
pub fn aggregate(per_world: &[(Vec<ArmStat>, f64)]) -> Vec<(u8, f64)> {
    let mut frac: std::collections::HashMap<u8, f64> = std::collections::HashMap::new();
    for (stats, weight) in per_world {
        let total: u64 = stats.iter().map(|a| a.visits as u64).sum();
        if total == 0 { continue; }
        for a in stats {
            // A solved terminal-KO arm contributes its provable win probability (branch-chance);
            // every other arm contributes its visit fraction as before. Off the analytic root no
            // arm is solved (win_chance == 0.0), so this is identical to the visit-fraction rule.
            let share = if a.win_chance > 0.0 { a.win_chance } else { a.visits as f64 / total as f64 };
            *frac.entry(a.action).or_insert(0.0) += weight * share;
        }
    }
    let mut v: Vec<(u8, f64)> = frac.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    v
}

pub fn pick_from(aggregated: &[(u8, f64)], legal: &ActionList, rng: &mut Lcg, pick_mode: PickMode, filter_threshold: f64) -> u8 {
    let legal_slice = legal.as_slice();
    let survivors: Vec<(u8, f64)> = {
        let best = aggregated.iter().filter(|(a, _)| legal_slice.contains(a)).map(|x| x.1).fold(0.0, f64::max);
        aggregated.iter()
            .filter(|(a, f)| legal_slice.contains(a) && *f >= filter_threshold * best)
            .cloned().collect()
    };
    if survivors.is_empty() { return legal_slice.first().copied().unwrap_or(ACTION_STRUGGLE); }
    if pick_mode == PickMode::Argmax { return survivors[0].0; }
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

/// Visit floor for value selection: admit any arm whose aggregate visit fraction is at least
/// this fraction of the best-visited arm's, so very-low-sample arms are excluded.
pub const VALUE_PICK_VISIT_FLOOR: f64 = 0.10;
/// Hard ceiling on the floor: above this, the floor can exclude the damaging fix-arm and
/// reproduce the no-op blunder, so the env override is clamped here.
pub const VALUE_PICK_VISIT_FLOOR_MAX: f64 = 0.20;

/// Effective floor: env-overridable for the gate sweep, clamped to [0.0, VALUE_PICK_VISIT_FLOOR_MAX].
pub fn value_pick_visit_floor() -> f64 {
    std::env::var("VALUE_PICK_VISIT_FLOOR").ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(VALUE_PICK_VISIT_FLOOR)
        .clamp(0.0, VALUE_PICK_VISIT_FLOOR_MAX)
}

/// Per action: (action, visit-weighted mean avg_score across worlds, aggregate visit fraction).
/// Sorted by mean avg_score desc, ties broken by lower byte (deterministic).
pub fn aggregate_value(per_world: &[(Vec<ArmStat>, f64)]) -> Vec<(u8, f64, f64)> {
    use std::collections::HashMap;
    let mut num: HashMap<u8, f64> = HashMap::new();
    let mut den: HashMap<u8, u64> = HashMap::new();
    let mut frac: HashMap<u8, f64> = HashMap::new();
    for (stats, weight) in per_world {
        let total: u64 = stats.iter().map(|a| a.visits as u64).sum();
        if total == 0 { continue; }
        for a in stats {
            *num.entry(a.action).or_insert(0.0) += a.avg_score * a.visits as f64;
            *den.entry(a.action).or_insert(0) += a.visits as u64;
            *frac.entry(a.action).or_insert(0.0) += weight * a.visits as f64 / total as f64;
        }
    }
    let mut v: Vec<(u8, f64, f64)> = num.keys().map(|&act| {
        (act, num[&act] / den[&act].max(1) as f64, frac[&act])
    }).collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    v
}

/// Argmax mean avg_score among legal arms that clear the (clamped, low) visit floor.
pub fn pick_value(valued: &[(u8, f64, f64)], legal: &ActionList) -> u8 {
    let legal_slice = legal.as_slice();
    let floor = value_pick_visit_floor();
    let best_frac = valued.iter()
        .filter(|(a, _, _)| legal_slice.contains(a))
        .map(|x| x.2).fold(0.0, f64::max);
    let mut best: Option<(u8, f64)> = None;
    for (a, mean, frac) in valued {
        if !legal_slice.contains(a) { continue; }
        if *frac < floor * best_frac { continue; }
        if best.map_or(true, |(_, bm)| *mean > bm) { best = Some((*a, *mean)); }
    }
    best.map(|(a, _)| a).unwrap_or_else(|| legal_slice.first().copied().unwrap_or(ACTION_STRUGGLE))
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

#[allow(clippy::too_many_arguments)]
fn search_eval(
    w: &World,
    eval: &impl Evaluator,
    cfg: &PimcConfig,
    params: &SearchParams,
    seed: u64,
    decider_side: usize,
    prior: Option<&[f32]>,
) -> SearchResult {
    match cfg.chance_mode {
        ChanceMode::OpenLoop =>
            search_world(&w.state, &w.teams, eval, &OpenLoop, params, seed, decider_side, prior),
        ChanceMode::ClosedLoop =>
            search_world_closed(&w.state, &w.teams, eval, params, seed),
        ChanceMode::AnalyticRoot =>
            search_world(&w.state, &w.teams, eval, &AnalyticRoot, params, seed, decider_side, prior),
    }
}

pub fn choose_action(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig) -> u8 {
    choose_action_eval(obs, belief, det, cfg, EvalKind::Handcrafted)
}

pub fn choose_action_eval(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig, eval: EvalKind<'_>) -> u8 {
    let legal = legal_actions(obs.state, obs.our_side);
    if legal.count == 0 { return ACTION_STRUGGLE; }
    if legal.count == 1 { return legal.actions[0]; }
    let mut rng = Lcg::new(splitmix64(cfg.seed));
    let worlds = det.sample_worlds(obs, belief, cfg.num_worlds, &mut rng);
    #[cfg(not(feature = "train_value"))]
    crate::train_dump::maybe_dump_worlds(&worlds, obs.our_side, obs.state.field.turn);
    let mut params = SearchParams {
        time_ms: cfg.time_ms_per_world,
        max_iters: cfg.max_iters_per_world,
        explore_coeff: cfg.explore_coeff,
        ..Default::default()
    };
    if cfg.chance_mode == ChanceMode::ClosedLoop {
        params.max_nodes = closed_loop_max_nodes(cfg.num_worlds);
    }
    let searched: Vec<(SearchResult, f64)> = worlds.par_iter().enumerate().map(|(k, w)| {
        let seed = splitmix64(cfg.seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let r = match eval {
            EvalKind::Handcrafted => search_eval(w, &Handcrafted, cfg, &params, seed, obs.our_side, None),
            EvalKind::Learned(le) => search_eval(w, le, cfg, &params, seed, obs.our_side, None),
        };
        (r, w.weight)
    }).collect();
    // Flushed only after every world's search returns, so the records carry the
    // decision's pooled value; per-decision stream order is unchanged.
    #[cfg(feature = "train_value")]
    crate::train_dump::maybe_dump_worlds_valued(&worlds, obs.our_side, obs.state.field.turn, &searched);
    let per_world: Vec<(Vec<ArmStat>, f64)> =
        searched.iter().map(|(r, w)| (r.side(obs.our_side).to_vec(), *w)).collect();
    if cfg.raw_root && cfg.num_worlds == 1 {
        // un-aggregated: return the single world's root best-arm (max visits) directly
        let (stats, _w) = &per_world[0];
        if let Some(best) = stats.iter().max_by(|a, b| a.visits.cmp(&b.visits).then(b.action.cmp(&a.action))) {
            return best.action;
        }
    }
    if cfg.pick_mode == PickMode::Value {
        return pick_value(&aggregate_value(&per_world), &legal);
    }
    let agg = aggregate(&per_world);
    pick_from(&agg, &legal, &mut rng, cfg.pick_mode, cfg.filter_threshold)
}

// Diagnostic mirror of `choose_action` that surfaces the internals a live decision hides:
// per-world effective iterations + determinized opponent, the aggregated root arm table,
// the 0.75-filter survivors, and the final pick. Additive; production path is untouched.
#[derive(Clone)]
pub struct WorldTrace {
    pub iterations: u64,
    pub depth_sum: u64,
    pub guard_hits: u64,
    pub opp_species: u16,
    pub opp_item: u16,
    pub opp_ability: u16,
    pub opp_hp: u16,
    pub opp_max_hp: u16,
    pub opp_status: u8,
    pub arms: Vec<ArmStat>,
}

#[derive(Clone)]
pub struct DecisionTrace {
    pub per_world: Vec<WorldTrace>,
    pub aggregate: Vec<(u8, f64)>,
    pub survivors: Vec<(u8, f64)>,
    pub legal: Vec<u8>,
    pub picked: u8,
}

pub fn choose_action_traced(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig) -> DecisionTrace {
    let legal = legal_actions(obs.state, obs.our_side);
    let legal_vec: Vec<u8> = legal.as_slice().to_vec();
    if legal.count <= 1 {
        let picked = if legal.count == 1 { legal.actions[0] } else { ACTION_STRUGGLE };
        return DecisionTrace { per_world: vec![], aggregate: vec![], survivors: vec![], legal: legal_vec, picked };
    }
    let mut rng = Lcg::new(splitmix64(cfg.seed));
    let worlds = det.sample_worlds(obs, belief, cfg.num_worlds, &mut rng);
    let opp = 1 - obs.our_side;
    let mut params = SearchParams {
        time_ms: cfg.time_ms_per_world,
        max_iters: cfg.max_iters_per_world,
        explore_coeff: cfg.explore_coeff,
        ..Default::default()
    };
    if cfg.chance_mode == ChanceMode::ClosedLoop {
        params.max_nodes = closed_loop_max_nodes(cfg.num_worlds);
    }
    let traced: Vec<WorldTrace> = worlds.par_iter().enumerate().map(|(k, w)| {
        let seed = splitmix64(cfg.seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let r = match cfg.chance_mode {
            ChanceMode::OpenLoop =>
                search_world(&w.state, &w.teams, &Handcrafted, &OpenLoop, &params, seed, obs.our_side, None),
            ChanceMode::ClosedLoop =>
                search_world_closed(&w.state, &w.teams, &Handcrafted, &params, seed),
            ChanceMode::AnalyticRoot =>
                search_world(&w.state, &w.teams, &Handcrafted, &AnalyticRoot, &params, seed, obs.our_side, None),
        };
        let ai = w.state.sides[opp].active_index as usize;
        let om = &w.state.sides[opp].team[ai];
        WorldTrace {
            iterations: r.iterations, depth_sum: r.depth_sum, guard_hits: r.guard_hits,
            opp_species: om.species_id, opp_item: om.item_id, opp_ability: om.ability_id,
            opp_hp: om.current_hp, opp_max_hp: om.max_hp, opp_status: om.status,
            arms: r.side(obs.our_side).to_vec(),
        }
    }).collect();
    let per_world_stats: Vec<(Vec<ArmStat>, f64)> =
        traced.iter().zip(worlds.iter()).map(|(t, w)| (t.arms.clone(), w.weight)).collect();
    let agg = aggregate(&per_world_stats);
    let legal_slice = legal.as_slice();
    let best = agg.iter().filter(|(a, _)| legal_slice.contains(a)).map(|x| x.1).fold(0.0, f64::max);
    let survivors: Vec<(u8, f64)> = agg.iter()
        .filter(|(a, f)| legal_slice.contains(a) && *f >= cfg.filter_threshold * best)
        .cloned().collect();
    // same `rng` (post-sample state) as choose_action, so the pick byte is reproduced exactly
    let picked = if cfg.pick_mode == PickMode::Value {
        pick_value(&aggregate_value(&per_world_stats), &legal)
    } else {
        pick_from(&agg, &legal, &mut rng, cfg.pick_mode, cfg.filter_threshold)
    };
    DecisionTrace { per_world: traced, aggregate: agg, survivors, legal: legal_vec, picked }
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
        v.iter().map(|&(a, n)| ArmStat { action: a, visits: n, avg_score: 0.5, win_chance: 0.0 }).collect()
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
        for _ in 0..200 { seen.insert(pick_from(&agg, &legal, &mut rng, PickMode::Weighted, 0.75)); }
        assert!(seen.contains(&4) && seen.contains(&0), "both >=75%-of-best survivors picked sometimes");
        assert!(!seen.contains(&1), "0.15 < 0.75*0.45 filtered");
        assert!(!seen.contains(&9), "illegal byte masked even with high fraction");
    }

    #[test]
    fn value_pick_prefers_value_over_visits() {
        // Measured Granbull case: the most-visited arm (byte 0) has the lower avg_score.
        let per_world = vec![(
            vec![
                ArmStat { action: 0, visits: 620, avg_score: 0.074, win_chance: 0.0 },
                ArmStat { action: 1, visits: 180, avg_score: 0.088, win_chance: 0.0 },
                ArmStat { action: 2, visits: 150, avg_score: 0.025, win_chance: 0.0 },
            ],
            1.0f64,
        )];
        let mut legal = ActionList::new();
        for a in [0u8, 1, 2] { let i = legal.count as usize; legal.actions[i] = a; legal.count += 1; }

        let agg = aggregate(&per_world);
        assert_eq!(agg[0].0, 0, "visit-based aggregate ranks the no-op first");

        let valued = aggregate_value(&per_world);
        assert_eq!(pick_value(&valued, &legal), 1, "value pick chooses the higher-value move");
    }

    #[test]
    fn value_pick_floor_must_stay_low_to_keep_the_fix_arm() {
        // The no-op (byte 0) is the most-visited arm; the fix (byte 1) is lower-visited.
        let per_world = vec![(
            vec![
                ArmStat { action: 0, visits: 620, avg_score: 0.074, win_chance: 0.0 },
                ArmStat { action: 1, visits: 180, avg_score: 0.088, win_chance: 0.0 },
            ],
            1.0f64,
        )];
        let mut legal = ActionList::new();
        for a in [0u8, 1] { let i = legal.count as usize; legal.actions[i] = a; legal.count += 1; }
        let valued = aggregate_value(&per_world);
        for f in ["0.0", "0.10", "0.20", "0.50", "0.90"] {
            std::env::set_var("VALUE_PICK_VISIT_FLOOR", f);
            let got = pick_value(&valued, &legal);
            std::env::remove_var("VALUE_PICK_VISIT_FLOOR");
            assert_eq!(got, 1, "floor {f} must keep the fix-arm");
        }
        assert!(value_pick_visit_floor() <= VALUE_PICK_VISIT_FLOOR_MAX);
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
        let cfg = PimcConfig { num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 9, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert!(legal_actions(&s, 0).as_slice().contains(&a));
        let b = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert_eq!(a, b, "same seed -> same choice (reproducibility, design §10)");
    }

    #[test]
    fn eval_sibling_handcrafted_matches_default_form() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        let mut belief = Belief::default();
        belief.note_species(445, s.sides[1].team[0].level);
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let cfg = PimcConfig { num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 9, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        let b = choose_action_eval(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted);
        assert_eq!(a, b, "4-arg form must delegate to the eval-carrying form unchanged");
    }

    #[test]
    fn parallel_choice_is_deterministic() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        let mut belief = Belief::default();
        belief.note_species(445, s.sides[1].team[0].level);
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let cfg = PimcConfig { num_worlds: 16, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 42, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        let b = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert_eq!(a, b, "par_iter order preserved -> same seed -> same choice");
    }
}
