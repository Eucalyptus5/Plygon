use crate::belief::Belief;
use crate::chance::OpenLoop;
use crate::chance_analytic::AnalyticRoot;
use crate::chance_closed::search_world_closed;
use crate::determinize::{Determinizer, Observation, World};
use crate::eval::{Evaluator, Handcrafted};
use crate::action_features::{action_features, ACTION_DENSE_DIM};
use crate::eval_learned::{masked_softmax, ForwardInput, LearnedEval, LearnedPolicyV2, LearnedValueV2, NUM_ACTIONS};
use crate::features::{self, DENSE_DIM, NUM_SEGMENTS};
use crate::rng::{splitmix64, Lcg};
use crate::search::{closed_loop_max_nodes, search_world, ArmStat, ChanceMode, SearchParams, SearchResult};
use pkmn_engine::state::*;
use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PickMode { #[default] Weighted, Argmax, Value }

/// Leaf evaluator for one decision. Carries a borrow of the once-constructed
/// net so every world's search shares the same weights.
#[derive(Clone, Copy, Default)]
pub enum EvalKind<'a> {
    #[default]
    Handcrafted,
    Learned(&'a LearnedEval),
    LearnedV2(&'a LearnedValueV2),
}

impl EvalKind<'_> {
    pub fn name(self) -> &'static str {
        match self {
            EvalKind::Handcrafted => "handcrafted",
            EvalKind::Learned(_) => "learned",
            EvalKind::LearnedV2(_) => "learned_v2",
        }
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
    pub value_temp: f32,
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

/// Everything one world's root-prior forward consumes. The single place the
/// per-action block handed to the forward is computed.
pub struct PriorInputs {
    pub ids: Vec<u32>,
    pub seg_lens: [u16; NUM_SEGMENTS],
    pub dense: [f32; DENSE_DIM],
    pub move_ids: [u16; 4],
    pub action_dense: [[f32; ACTION_DENSE_DIM]; NUM_ACTIONS],
    pub legal: [bool; NUM_ACTIONS],
}

/// The token stream is decider-oriented (mirrored at seat 1) because the extractor's
/// emission is side-keyed; the per-action block is NOT, because `mirror` leaves phase
/// and pending_actions side-specific, so it reads the unmirrored state at our real side.
pub fn prior_inputs(state: &BattleState, our_side: usize) -> PriorInputs {
    let oriented = if our_side == 1 { features::mirror(state) } else { *state };
    let mut ids = Vec::with_capacity(192);
    let seg_lens = features::extract_segmented(&oriented, &mut ids);
    let dense = features::extract_dense(&oriented);
    let side = &oriented.sides[0];
    let move_ids = side.team[side.active_index as usize].moves;
    let action_dense = action_features(state, our_side);
    let mut legal = [false; NUM_ACTIONS];
    for &a in legal_actions(state, our_side).as_slice() {
        if (a as usize) < NUM_ACTIONS {
            legal[a as usize] = true;
        }
    }
    PriorInputs { ids, seg_lens, dense, move_ids, action_dense, legal }
}

/// One batched forward per turn over every world, masked to each world's legal set.
pub fn root_priors(
    net: &LearnedPolicyV2,
    worlds: &[World],
    our_side: usize,
) -> Vec<[f32; NUM_ACTIONS]> {
    let inputs: Vec<PriorInputs> =
        worlds.iter().map(|w| prior_inputs(&w.state, our_side)).collect();
    let items: Vec<ForwardInput> = inputs
        .iter()
        .map(|p| ForwardInput {
            ids: &p.ids,
            seg_lens: &p.seg_lens,
            dense: &p.dense,
            move_ids: &p.move_ids,
            action_dense: if net.action_dense_dim() == 0 { None } else { Some(&p.action_dense) },
        })
        .collect();
    net.forward_batch(&items)
        .iter()
        .zip(inputs.iter())
        .map(|(logits, p)| masked_softmax(logits, &p.legal))
        .collect()
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
        ChanceMode::ClosedLoop => {
            assert!(prior.is_none(), "the closed-loop search takes no root prior");
            search_world_closed(&w.state, &w.teams, eval, params, seed)
        }
        ChanceMode::AnalyticRoot =>
            search_world(&w.state, &w.teams, eval, &AnalyticRoot, params, seed, decider_side, prior),
    }
}

pub fn choose_action(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig) -> u8 {
    choose_action_eval(obs, belief, det, cfg, EvalKind::Handcrafted, None)
}

pub fn choose_action_eval(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig, eval: EvalKind<'_>, prior: Option<&LearnedPolicyV2>) -> u8 {
    choose_action_eval_iters(obs, belief, det, cfg, eval, prior).0
}

/// `choose_action_eval` plus the iterations the search actually served, taken as the
/// minimum across worlds so one clipped world cannot hide behind the others.
pub fn choose_action_eval_iters(obs: &Observation, belief: &Belief, det: &impl Determinizer, cfg: &PimcConfig, eval: EvalKind<'_>, prior: Option<&LearnedPolicyV2>) -> (u8, u64) {
    let legal = legal_actions(obs.state, obs.our_side);
    if legal.count == 0 { return (ACTION_STRUGGLE, 0); }
    if legal.count == 1 { return (legal.actions[0], 0); }
    let mut rng = Lcg::new(splitmix64(cfg.seed));
    let worlds = det.sample_worlds(obs, belief, cfg.num_worlds, &mut rng);
    #[cfg(not(feature = "train_value"))]
    crate::train_dump::maybe_dump_worlds(&worlds, obs.our_side, obs.state.field.turn);
    let mut params = SearchParams {
        time_ms: cfg.time_ms_per_world,
        max_iters: cfg.max_iters_per_world,
        explore_coeff: cfg.explore_coeff,
        value_temp: cfg.value_temp,
        ..Default::default()
    };
    if cfg.chance_mode == ChanceMode::ClosedLoop {
        params.max_nodes = closed_loop_max_nodes(cfg.num_worlds);
    }
    // one forward per turn over every world, ahead of the parallel searches
    let priors = prior.map(|net| root_priors(net, &worlds, obs.our_side));
    let searched: Vec<(SearchResult, f64)> = worlds.par_iter().enumerate().map(|(k, w)| {
        let seed = splitmix64(cfg.seed ^ (k as u64).wrapping_mul(0x9E3779B97F4A7C15));
        let p = priors.as_ref().map(|v| &v[k][..]);
        let r = match eval {
            EvalKind::Handcrafted => search_eval(w, &Handcrafted, cfg, &params, seed, obs.our_side, p),
            EvalKind::Learned(le) => search_eval(w, le, cfg, &params, seed, obs.our_side, p),
            EvalKind::LearnedV2(lv) => search_eval(w, lv, cfg, &params, seed, obs.our_side, p),
        };
        (r, w.weight)
    }).collect();
    // Flushed only after every world's search returns, so the records carry the
    // decision's pooled value; per-decision stream order is unchanged.
    #[cfg(feature = "train_value")]
    crate::train_dump::maybe_dump_worlds_valued(&worlds, obs.our_side, obs.state.field.turn, &searched);
    let served_iters = searched.iter().map(|(r, _)| r.iterations).min().unwrap_or(0);
    let per_world: Vec<(Vec<ArmStat>, f64)> =
        searched.iter().map(|(r, w)| (r.side(obs.our_side).to_vec(), *w)).collect();
    if cfg.raw_root && cfg.num_worlds == 1 {
        // un-aggregated: return the single world's root best-arm (max visits) directly
        let (stats, _w) = &per_world[0];
        if let Some(best) = stats.iter().max_by(|a, b| a.visits.cmp(&b.visits).then(b.action.cmp(&a.action))) {
            return (best.action, served_iters);
        }
    }
    if cfg.pick_mode == PickMode::Value {
        return (pick_value(&aggregate_value(&per_world), &legal), served_iters);
    }
    let agg = aggregate(&per_world);
    (pick_from(&agg, &legal, &mut rng, cfg.pick_mode, cfg.filter_threshold), served_iters)
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
        value_temp: cfg.value_temp,
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

    fn asymmetric_root() -> (BattleState, TeamData) {
        let (mut s, t) = build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        );
        let ai = s.sides[1].active_index as usize;
        s.sides[1].team[ai].current_hp = 0;
        s.phase = PHASE_SWITCH_P2;
        (s, t)
    }

    #[test]
    fn prior_inputs_block_is_the_unmirrored_side_block() {
        use crate::action_features::action_features;
        let (s, _t) = asymmetric_root();
        let got = prior_inputs(&s, 1);
        assert_eq!(
            got.action_dense,
            action_features(&s, 1),
            "the block must come from the unmirrored state at the decider's real side"
        );
        assert_ne!(
            got.action_dense,
            action_features(&crate::features::mirror(&s), 0),
            "fixture must be side-asymmetric, so the mirrored call is the wrong answer"
        );
    }

    #[test]
    fn prior_inputs_tokens_come_from_the_decider_oriented_state() {
        let (s, _t) = asymmetric_root();
        let m = crate::features::mirror(&s);
        let mut want_ids = Vec::new();
        let want_lens = crate::features::extract_segmented(&m, &mut want_ids);
        let got = prior_inputs(&s, 1);
        assert_eq!(got.ids, want_ids, "seat 1 tokens must come from the mirrored state");
        assert_eq!(got.seg_lens, want_lens);
        assert_eq!(got.dense, crate::features::extract_dense(&m));
        let side = &m.sides[0];
        assert_eq!(
            got.move_ids,
            side.team[side.active_index as usize].moves,
            "move ids follow the oriented token-side-0 active mon"
        );
        let mut want_legal = [false; crate::eval_learned::NUM_ACTIONS];
        for &a in legal_actions(&s, 1).as_slice() {
            if (a as usize) < want_legal.len() {
                want_legal[a as usize] = true;
            }
        }
        assert_eq!(got.legal, want_legal, "mask reads the unmirrored state at our side");
    }

    #[test]
    fn prior_inputs_at_seat_zero_reads_the_state_as_given() {
        let (s, _t) = asymmetric_root();
        let got = prior_inputs(&s, 0);
        let mut want_ids = Vec::new();
        let want_lens = crate::features::extract_segmented(&s, &mut want_ids);
        assert_eq!(got.ids, want_ids);
        assert_eq!(got.seg_lens, want_lens);
        assert_eq!(got.action_dense, crate::action_features::action_features(&s, 0));
    }

    fn one_world(s: BattleState, t: TeamData) -> World {
        World { state: s, teams: t, weight: 1.0 }
    }

    fn prior_cfg(chance_mode: ChanceMode) -> PimcConfig {
        PimcConfig {
            num_worlds: 1,
            time_ms_per_world: 1000,
            max_iters_per_world: 200,
            seed: 3,
            chance_mode,
            pick_mode: PickMode::Argmax,
            filter_threshold: 0.75,
            raw_root: false,
            explore_coeff: 2.0,
            value_temp: 1.0,
        }
    }

    #[test]
    #[should_panic(expected = "closed-loop search takes no root prior")]
    fn closed_loop_refuses_a_prior() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 14, 0, 0]));
        let cfg = prior_cfg(ChanceMode::ClosedLoop);
        let params = SearchParams { time_ms: 1000, max_iters: 200, ..Default::default() };
        let p = [0.5f32; crate::eval_learned::NUM_ACTIONS];
        search_eval(&one_world(s, t), &Handcrafted, &cfg, &params, 3, 0, Some(&p));
    }

    #[test]
    fn prior_carrying_modes_search_without_panic() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 14, 0, 0]));
        let params = SearchParams { time_ms: 1000, max_iters: 200, ..Default::default() };
        let p = [0.5f32; crate::eval_learned::NUM_ACTIONS];
        for mode in [ChanceMode::OpenLoop, ChanceMode::AnalyticRoot] {
            let cfg = prior_cfg(mode);
            let w = one_world(s, t.clone());
            let r = search_eval(&w, &Handcrafted, &cfg, &params, 3, 0, Some(&p));
            assert_eq!(r.iterations, 200, "{mode:?} must run with a prior");
        }
        let cfg = prior_cfg(ChanceMode::ClosedLoop);
        let w = one_world(s, t);
        let r = search_eval(&w, &Handcrafted, &cfg, &params, 3, 0, None);
        assert!(r.iterations > 0, "closed loop still runs without a prior");
    }

    #[test]
    fn served_iters_report_the_configured_budget() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        );
        let belief = Belief::default();
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let cfg = PimcConfig {
            num_worlds: 4,
            time_ms_per_world: 600_000,
            max_iters_per_world: 512,
            seed: 11,
            chance_mode: ChanceMode::OpenLoop,
            pick_mode: PickMode::Argmax,
            filter_threshold: 0.75,
            raw_root: false,
            explore_coeff: 2.0,
            value_temp: 1.0,
        };
        let (_action, served) =
            choose_action_eval_iters(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None);
        assert_eq!(served, 512, "every world must report the configured budget");
    }

    #[test]
    fn served_iters_come_from_the_search_not_the_config() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        );
        let belief = Belief::default();
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let cfg = PimcConfig {
            num_worlds: 4,
            time_ms_per_world: 1,
            max_iters_per_world: 200_000,
            seed: 11,
            chance_mode: ChanceMode::OpenLoop,
            pick_mode: PickMode::Argmax,
            filter_threshold: 0.75,
            raw_root: false,
            explore_coeff: 2.0,
            value_temp: 1.0,
        };
        let (_action, served) =
            choose_action_eval_iters(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None);
        assert!(served > 0, "a time-clipped search still ran some iterations");
        assert!(
            served < cfg.max_iters_per_world,
            "a time-clipped search must report what it ran, not the configured cap: {served}"
        );
    }

    #[test]
    fn a_forced_decision_serves_no_iterations() {
        let (s, t) = asymmetric_root();
        assert_eq!(legal_actions(&s, 1).count, 1, "fixture must offer exactly one legal action");
        let belief = Belief::default();
        let obs = Observation { state: &s, our_side: 1, teams: &t };
        let cfg = PimcConfig { num_worlds: 4, seed: 11, ..prior_cfg(ChanceMode::OpenLoop) };
        let (action, served) =
            choose_action_eval_iters(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None);
        assert_eq!(served, 0, "a decision that returns before any search serves 0 iterations");
        assert_eq!(action, legal_actions(&s, 1).actions[0]);
    }

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
        let cfg = PimcConfig { num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 9, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0, value_temp: 1.0 };
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
        let cfg = PimcConfig { num_worlds: 4, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 9, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0, value_temp: 1.0 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        let b = choose_action_eval(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None);
        assert_eq!(a, b, "4-arg form must delegate to the eval-carrying form unchanged");
    }

    #[test]
    fn choose_action_with_a_prior_is_legal_and_reproducible() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../learned-eval/weights/lvp2-6f1e0facfcc4.bin"
        );
        let Ok(bin) = std::fs::read(path) else {
            eprintln!("SKIP prior end-to-end: {path} absent");
            return;
        };
        let net = LearnedPolicyV2::from_bytes(&bin).expect("weights must load");
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        let mut belief = Belief::default();
        belief.note_species(445, s.sides[1].team[0].level);
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let cfg = PimcConfig { num_worlds: 2, time_ms_per_world: 1000, max_iters_per_world: 200, seed: 9, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Argmax, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0, value_temp: 1.0 };
        let a = choose_action_eval(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, Some(&net));
        assert!(legal_actions(&s, 0).as_slice().contains(&a));
        let b = choose_action_eval(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, Some(&net));
        assert_eq!(a, b, "same seed -> same choice with a prior");
    }

    const SEAT_PAIR_WEIGHTS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../learned-eval/weights/lvp2-6f1e0facfcc4.bin"
    );

    // Two different teams under a side-symmetric phase with symmetric pending_actions,
    // so `mirror` is a true seat swap here and scoring the wrong side is detectable.
    fn seat_pair_root() -> BattleState {
        build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        )
        .0
    }

    fn forward_prior(net: &LearnedPolicyV2, p: &PriorInputs) -> [f32; NUM_ACTIONS] {
        let block = if net.action_dense_dim() == 0 { None } else { Some(&p.action_dense) };
        masked_softmax(&net.forward(&p.ids, &p.seg_lens, &p.dense, &p.move_ids, block), &p.legal)
    }

    // What a forward that skipped the decider-perspective mirror would produce: the token
    // stream comes off the root as given, so at seat 1 the head scores the opponent.
    fn unmirrored_prior_inputs(state: &BattleState, our_side: usize) -> PriorInputs {
        let mut ids = Vec::new();
        let seg_lens = features::extract_segmented(state, &mut ids);
        let dense = features::extract_dense(state);
        let side = &state.sides[0];
        let move_ids = side.team[side.active_index as usize].moves;
        let action_dense = action_features(state, our_side);
        let mut legal = [false; NUM_ACTIONS];
        for &a in legal_actions(state, our_side).as_slice() {
            if (a as usize) < NUM_ACTIONS {
                legal[a as usize] = true;
            }
        }
        PriorInputs { ids, seg_lens, dense, move_ids, action_dense, legal }
    }

    fn max_abs_delta(a: &[f32; NUM_ACTIONS], b: &[f32; NUM_ACTIONS]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    #[test]
    #[ignore]
    fn seat_swapped_pair_yields_the_identical_decider_prior() {
        let path = std::env::var("LVP2_WEIGHTS").unwrap_or_else(|_| SEAT_PAIR_WEIGHTS.to_string());
        let Ok(bin) = std::fs::read(&path) else {
            println!(
                "seat-orientation weights={path} bytes_compared=0 bits_identical=0 \
                 max_abs_delta=0 misoriented_max_abs_delta=0 status=FAIL"
            );
            panic!("{path} must be present: a skipped run of this gate is a failed one");
        };
        let net = LearnedPolicyV2::from_bytes(&bin).expect("LVP2 weights must load");
        let s = seat_pair_root();
        let m = features::mirror(&s);

        assert_eq!(s.phase, PHASE_ACTIONS, "fixture must sit under a side-symmetric phase");
        assert_eq!(
            s.pending_actions[0], s.pending_actions[1],
            "fixture must carry symmetric pending_actions, else mirror is not a seat swap"
        );
        assert_eq!(
            legal_actions(&m, 1).as_slice(),
            legal_actions(&s, 0).as_slice(),
            "mirror must be a true seat swap for the decider's legal set"
        );
        assert_eq!(
            action_features(&m, 1),
            action_features(&s, 0),
            "mirror must be a true seat swap for the decider's per-action block"
        );
        let mut m_ids = Vec::new();
        features::extract_segmented(&m, &mut m_ids);
        let mut s_ids = Vec::new();
        features::extract_segmented(&s, &mut s_ids);
        assert_ne!(m_ids, s_ids, "fixture must be content-asymmetric, else the test is vacuous");

        let at_seat0 = forward_prior(&net, &prior_inputs(&s, 0));
        let at_seat1 = forward_prior(&net, &prior_inputs(&m, 1));
        let misoriented = forward_prior(&net, &unmirrored_prior_inputs(&m, 1));

        let identical = at_seat0
            .iter()
            .zip(at_seat1.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        println!(
            "seat-orientation weights={path} bytes_compared={NUM_ACTIONS} \
             bits_identical={identical} max_abs_delta={:.9} misoriented_max_abs_delta={:.9} \
             seat0={at_seat0:?} status={}",
            max_abs_delta(&at_seat0, &at_seat1),
            max_abs_delta(&at_seat0, &misoriented),
            if identical { "PASS" } else { "FAIL" }
        );

        for a in 0..NUM_ACTIONS {
            assert_eq!(
                at_seat0[a].to_bits(),
                at_seat1[a].to_bits(),
                "action byte {a}: the decider's prior must not depend on its seat"
            );
        }
        assert!(
            max_abs_delta(&at_seat0, &misoriented) > 0.0,
            "a seat-1 forward that skipped the mirror must be detectable on this fixture"
        );
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
        let cfg = PimcConfig { num_worlds: 16, time_ms_per_world: 1000, max_iters_per_world: 2000, seed: 42, chance_mode: ChanceMode::OpenLoop, pick_mode: PickMode::Weighted, filter_threshold: 0.75, raw_root: false, explore_coeff: 2.0, value_temp: 1.0 };
        let a = choose_action(&obs, &belief, &RandomBattle, &cfg);
        let b = choose_action(&obs, &belief, &RandomBattle, &cfg);
        assert_eq!(a, b, "par_iter order preserved -> same seed -> same choice");
    }

    fn temp_cfg(seed: u64, value_temp: f32) -> PimcConfig {
        PimcConfig {
            num_worlds: 4,
            time_ms_per_world: 600_000,
            max_iters_per_world: 400,
            seed,
            chance_mode: ChanceMode::OpenLoop,
            pick_mode: PickMode::Argmax,
            filter_threshold: 0.75,
            raw_root: false,
            explore_coeff: 2.0,
            value_temp,
        }
    }

    #[test]
    fn value_temp_reaches_the_search() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        );
        let belief = Belief::default();
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        let pick = |seed: u64, value_temp: f32| {
            choose_action_eval(
                &obs,
                &belief,
                &RandomBattle,
                &temp_cfg(seed, value_temp),
                EvalKind::Handcrafted,
                None,
            )
        };
        let mut flipped: Vec<u64> = Vec::new();
        for seed in 0..64u64 {
            let hot = pick(seed, 2.5);
            let cold = pick(seed, 1.0);
            assert_eq!(cold, pick(seed, 1.0), "seed {seed}: T=1.0 must be reproducible");
            if cold != hot {
                flipped.push(seed);
            }
        }
        assert!(!flipped.is_empty(), "no seed flipped: the temperature never reached the search");
    }

    #[test]
    fn value_temp_one_matches_a_search_built_without_the_knob() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 150, 33, 34]), mon(143, 47, [34, 89, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 85, 0, 0])],
        );
        let belief = Belief::default();
        let obs = Observation { state: &s, our_side: 0, teams: &t };
        for seed in 0..64u64 {
            let cfg = PimcConfig { num_worlds: 1, raw_root: true, ..temp_cfg(seed, 1.0) };
            let got = choose_action_eval(&obs, &belief, &RandomBattle, &cfg, EvalKind::Handcrafted, None);
            let mut rng = Lcg::new(splitmix64(cfg.seed));
            let worlds = RandomBattle.sample_worlds(&obs, &belief, cfg.num_worlds, &mut rng);
            let params = SearchParams {
                time_ms: cfg.time_ms_per_world,
                max_iters: cfg.max_iters_per_world,
                explore_coeff: cfg.explore_coeff,
                ..Default::default()
            };
            let r = search_eval(&worlds[0], &Handcrafted, &cfg, &params, splitmix64(cfg.seed), 0, None);
            let want = r
                .side(0)
                .iter()
                .max_by(|a, b| a.visits.cmp(&b.visits).then(b.action.cmp(&a.action)))
                .expect("the root must expose arms")
                .action;
            assert_eq!(got, want, "seed {seed}: T=1.0 must match the search built without the knob");
        }
    }
}
