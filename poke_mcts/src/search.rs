use crate::chance::ChanceModel;
use crate::eval::{sigmoid, winner_value, Evaluator};
use crate::node::{child_key, Bandit, Node, NO_CHILD};
use crate::rng::Lcg;
use crate::select::select_arm;
use pkmn_engine::state::*;
use std::time::Instant;

pub struct SearchParams {
    pub time_ms: u64,
    pub max_iters: u64,   // sanity cap (design §5.5); u64::MAX in normal play
    pub max_nodes: u32,   // memory cap; stop expanding past it
    pub explore_coeff: f64, // UCB sqrt coefficient (c²); 2.0 = textbook c=√2
    pub value_temp: f32, // divides the root-relative leaf value before the sigmoid; 1.0 = unchanged
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams { time_ms: 100, max_iters: u64::MAX, max_nodes: 2_000_000, explore_coeff: 2.0, value_temp: 1.0 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChanceMode {
    #[default]
    OpenLoop,
    ClosedLoop,
    AnalyticRoot,
}

// Closed-loop nodes own a BattleState + outcome states (~1.1KB/node amortized); peak RAM is
// LIVE across all rayon worlds, so the per-world cap inverts a fixed ceiling (spec §4.3).
pub const CLOSED_LOOP_RAM_CEILING_BYTES: u64 = 2_000_000_000;
const CLOSED_LOOP_BYTES_PER_NODE: u64 = 1_100;

pub fn closed_loop_max_nodes(num_worlds: usize) -> u32 {
    let worlds = num_worlds.max(1) as u64;
    let cap = CLOSED_LOOP_RAM_CEILING_BYTES / (worlds * CLOSED_LOOP_BYTES_PER_NODE);
    cap.clamp(50_000, 2_000_000) as u32
}

#[derive(Clone, Copy, Default)]
pub struct ArmStat {
    pub action: u8,
    pub visits: u32,
    pub avg_score: f64,
    // Provable terminal-win branch-chance for this root arm under the analytic root (0.0 = not
    // solved). The final-selection statistic reads this instead of the post-termination visit
    // count, which collapses once a KO child is solved. Always 0.0 off the analytic root path.
    pub win_chance: f64,
}

#[derive(Clone)]
pub struct SearchResult {
    pub s1: Vec<ArmStat>,
    pub s2: Vec<ArmStat>,
    pub iterations: u64,
    pub guard_hits: u64,
    pub depth_sum: u64,
    // Max-visit joint action (side 0's byte, side 1's byte) at each of the first three plies of
    // the finished tree, in the same absolute side order as s1/s2; NO_ARM where the ply or the
    // side's bandit is absent. Diagnostic only.
    pub principal: [(u8, u8); 3],
    #[cfg(feature = "train_value")]
    pub value_sum: f64,
    #[cfg(feature = "train_value")]
    pub value_count: u64,
    // One count per iteration, binning the backprop leaf value over [0, 1]. Diagnostic only:
    // written after the value is settled, never read by UCB, backprop or the pick.
    #[cfg(feature = "train_value")]
    pub leaf_hist: [u32; 20],
}

impl Default for SearchResult {
    fn default() -> Self {
        SearchResult {
            s1: Vec::new(),
            s2: Vec::new(),
            iterations: 0,
            guard_hits: 0,
            depth_sum: 0,
            principal: NO_PRINCIPAL,
            #[cfg(feature = "train_value")]
            value_sum: 0.0,
            #[cfg(feature = "train_value")]
            value_count: 0,
            #[cfg(feature = "train_value")]
            leaf_hist: [0; 20],
        }
    }
}

impl SearchResult {
    pub fn side(&self, side: usize) -> &[ArmStat] { if side == 0 { &self.s1 } else { &self.s2 } }
}

struct PathStep { node: usize, a1: u8, a2: u8 } // arm indices; 255 = side had no bandit
pub const NO_ARM: u8 = 255;
pub const NO_PRINCIPAL: [(u8, u8); 3] = [(NO_ARM, NO_ARM); 3];

// Shared: one tree; blinded rollouts pick our action from our true bandit and credit only the
// opponent's. Split: blinded rollouts carry their own decider bandit, phase and dice, so the
// opponent's statistics are a function of the blinded root alone; the two parts of a node share
// only the child index array, and a rollout reaching a node whose part is missing initialises it
// and stops, as an expansion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlindMode { Shared, Split }
const NO_PART: u8 = 255;
const BLIND_RNG_SALT: u64 = 0x5851_F42D_4C95_7F2D;

struct BlindPart { phase: u8, us: Bandit }

pub fn search_world(
    root_state: &BattleState,
    teams: &TeamData,
    evaluator: &impl Evaluator,
    chance: &impl ChanceModel,
    params: &SearchParams,
    seed: u64,
    decider_side: usize,
    prior: Option<&[f32]>,
) -> SearchResult {
    search_world_inner(root_state, teams, None, evaluator, chance, params, seed, decider_side, prior)
}

/// Two-root search: odd iterations roll out the blinded root (the same world with the decider's
/// side re-sampled from the opponent's belief) and credit only the opponent's bandits; even
/// iterations roll out `root_state` and credit only the decider's. A blinded root whose legal
/// arms differ from the true root's is ignored.
#[allow(clippy::too_many_arguments)]
pub fn search_world_blind(
    root_state: &BattleState,
    teams: &TeamData,
    blind_state: &BattleState,
    blind_teams: &TeamData,
    mode: BlindMode,
    evaluator: &impl Evaluator,
    chance: &impl ChanceModel,
    params: &SearchParams,
    seed: u64,
    decider_side: usize,
    prior: Option<&[f32]>,
) -> SearchResult {
    search_world_inner(root_state, teams, Some((blind_state, blind_teams, mode)), evaluator, chance, params, seed, decider_side, prior)
}

#[allow(clippy::too_many_arguments)]
fn search_world_inner(
    root_state: &BattleState,
    teams: &TeamData,
    blind: Option<(&BattleState, &TeamData, BlindMode)>,
    evaluator: &impl Evaluator,
    chance: &impl ChanceModel,
    params: &SearchParams,
    seed: u64,
    decider_side: usize,
    prior: Option<&[f32]>,
) -> SearchResult {
    let mut rng = Lcg::new(seed);
    let mut tree: Vec<Node> = Vec::with_capacity(4096);
    tree.push(Node::from_state(root_state));
    if root_state.is_game_over() || (tree[0].s1.is_empty() && tree[0].s2.is_empty()) {
        return harvest(&tree, 0, 0, 0);
    }
    let root_eval = evaluator.eval(root_state) as f32;
    let blind = blind.filter(|(b, _, _)| same_root_shape(root_state, b));
    let blind_eval = blind.map_or(root_eval, |(b, _, _)| evaluator.eval(b) as f32);
    let split = matches!(blind, Some((_, _, BlindMode::Split)));
    let mut parts: Vec<BlindPart> = Vec::new();
    if let Some((b, _, BlindMode::Split)) = blind {
        parts.reserve(4096);
        parts.push(BlindPart { phase: NO_PART, us: Bandit::default() });
        init_blind_part(&mut tree[0], &mut parts[0], b, decider_side);
    }
    let mut brng = Lcg::new(seed ^ BLIND_RNG_SALT);
    let start = Instant::now();
    let mut iters: u64 = 0;
    let mut guard_hits: u64 = 0;
    let mut depth_sum: u64 = 0;
    let mut path: Vec<PathStep> = Vec::with_capacity(64);
    #[cfg(feature = "train_value")]
    let (mut value_sum, mut value_count) = (0.0f64, 0u64);
    #[cfg(feature = "train_value")]
    let mut leaf_hist = [0u32; 20];
    // Per-root-arm provable terminal-win branch-chance (analytic root only; stays 0.0 otherwise).
    let mut win_s1 = [0.0f64; crate::node::ARM_CAP];
    let mut win_s2 = [0.0f64; crate::node::ARM_CAP];

    'outer: while iters < params.max_iters {
        // clock check batched to amortize the read (design §5.6)
        if iters % 1024 == 0 && start.elapsed().as_millis() as u64 >= params.time_ms && iters > 0 {
            break 'outer;
        }
        path.clear();
        let blinded = blind.is_some() && iters & 1 == 1;
        let (start_state, teams, root_eval) = match blind {
            Some((bs, bt, _)) if blinded => (bs, bt, blind_eval),
            _ => (root_state, teams, root_eval),
        };
        let dice: &mut Lcg = if split && blinded { &mut brng } else { &mut rng };
        let mut cur = *start_state;
        let mut idx = 0usize;
        let value: f64;
        #[cfg(feature = "train_value")]
        let value_abs: f64;

        loop {
            // arm-mismatch guard: this sample diverged from the node's recorded shape
            // (provably never fires at the root, where cur is the untouched root state)
            let stop = idx != 0
                && if split {
                    part_mismatch(&mut tree[idx], &mut parts[idx], &cur, blinded, decider_side, &mut guard_hits)
                } else {
                    let hit = tree[idx].phase != cur.phase || !arms_match(&tree[idx], &cur);
                    guard_hits += hit as u64;
                    hit
                };
            if stop {
                #[cfg(not(feature = "train_value"))]
                {
                    value = leaf(&cur, evaluator, root_eval, params.value_temp);
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval, params.value_temp);
                    value = v;
                    value_abs = a;
                    leaf_hist[bin20(v)] += 1;
                }
                break;
            }
            let node = &tree[idx];
            let (p1, p2) = bandit_priors(idx, decider_side, prior);
            let (n1, n2) = parent_visits(node, blind.is_some(), decider_side);
            let (a1, b1) = pick_with(&node.s1, n1, params.explore_coeff, p1);
            let (a2, b2) = pick_with(&node.s2, n2, params.explore_coeff, p2);
            let (a1, b1, a2, b2) = if split && blinded {
                let (af, bf) = pick_with(&parts[idx].us, node.blind_visits, params.explore_coeff, None);
                if decider_side == 0 { (af, bf, a2, b2) } else { (a1, b1, af, bf) }
            } else {
                (a1, b1, a2, b2)
            };
            if a1 == NO_ARM && a2 == NO_ARM {
                #[cfg(not(feature = "train_value"))]
                {
                    value = leaf(&cur, evaluator, root_eval, params.value_temp);
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval, params.value_temp);
                    value = v;
                    value_abs = a;
                    leaf_hist[bin20(v)] += 1;
                }
                break;
            }
            // Design 1 (05 §2): at the root only, descend one analytic weighted child (KO-split)
            // instead of a single live dice sample. Off-root and the non-single-hit fallback keep
            // the open-loop `transition`. OpenLoop returns None here, so its path is unchanged.
            cur = if idx == 0 {
                if let Some(children) = chance.analytic_root_children(&cur, teams, b1, b2) {
                    // The KO child (children[0]) carries branch-chance children[0].0. When it is a
                    // terminal win, record that probability against the WINNING side's root arm so
                    // the final pick credits a solved KO by its true win chance, not the visit count
                    // that collapses once the terminal child stops being expanded.
                    let ko = &children[0];
                    if ko.1.is_game_over() {
                        let wv = winner_value(&ko.1);
                        if wv > 0.5 {
                            if a1 != NO_ARM { win_s1[a1 as usize] = ko.0; }
                        } else if wv < 0.5 {
                            if a2 != NO_ARM { win_s2[a2 as usize] = ko.0; }
                        }
                    }
                    let r = dice.roll(crate::chance_analytic::WEIGHT_SCALE);
                    crate::chance_analytic::pick_weighted(&children, r)
                } else {
                    chance.transition(&cur, teams, b1, b2, &mut |m| dice.roll(m))
                }
            } else {
                chance.transition(&cur, teams, b1, b2, &mut |m| dice.roll(m))
            };
            path.push(PathStep { node: idx, a1, a2 });

            let key = child_key(arm0(a1), arm0(a2));
            let child = tree[idx].children[key];
            if child == NO_CHILD {
                #[cfg(not(feature = "train_value"))]
                {
                    value = if cur.is_game_over() {
                        winner_value(&cur)
                    } else {
                        leaf(&cur, evaluator, root_eval, params.value_temp)
                    };
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval, params.value_temp);
                    value = v;
                    value_abs = a;
                    leaf_hist[bin20(v)] += 1;
                }
                if (tree.len() as u32) < params.max_nodes && !cur.is_game_over() {
                    tree.push(Node::from_state(&cur));
                    if split {
                        parts.push(own_part(tree.last_mut().unwrap(), blinded, decider_side));
                    }
                    let new_idx = (tree.len() - 1) as u32;
                    tree[idx].children[key] = new_idx;
                }
                break;
            }
            if cur.is_game_over() {
                value = winner_value(&cur);
                #[cfg(feature = "train_value")]
                {
                    value_abs = value;
                    leaf_hist[bin20(value)] += 1;
                }
                break;
            }
            idx = child as usize;
        }

        depth_sum += path.len() as u64;
        #[cfg(feature = "train_value")]
        {
            value_sum += value_abs;
            value_count += 1;
        }
        let (credit_s1, credit_s2) = if blind.is_none() {
            (true, true)
        } else {
            let c = blinded ^ (decider_side == 0);
            (c, !c)
        };
        for step in path.iter() {
            let n = &mut tree[step.node];
            if split && blinded {
                credit_blind_part(n, &mut parts[step.node], step, value, decider_side);
                continue;
            }
            if blinded { n.blind_visits += 1; } else { n.visits += 1; }
            if step.a1 != NO_ARM && credit_s1 {
                let arm = &mut n.s1.arms[step.a1 as usize];
                arm.visits += 1;
                arm.total_score += value;
            }
            if step.a2 != NO_ARM && credit_s2 {
                let arm = &mut n.s2.arms[step.a2 as usize];
                arm.visits += 1;
                arm.total_score += 1.0 - value;
            }
        }
        iters += 1;
    }

    let mut res = harvest(&tree, iters, guard_hits, depth_sum);
    #[cfg(feature = "train_value")]
    {
        res.value_sum = value_sum;
        res.value_count = value_count;
        res.leaf_hist = leaf_hist;
    }
    for (i, a) in res.s1.iter_mut().enumerate() { a.win_chance = win_s1[i]; }
    for (i, a) in res.s2.iter_mut().enumerate() { a.win_chance = win_s2[i]; }
    res
}

#[inline]
pub(crate) fn leaf(s: &BattleState, evaluator: &impl Evaluator, root_eval: f32, value_temp: f32) -> f64 {
    if s.is_game_over() { winner_value(s) } else { sigmoid((evaluator.eval(s) - root_eval) / value_temp) }
}

// One eval per leaf: the root-relative sigmoid backprops, the absolute sigmoid only
// feeds the value accumulator (never UCB, backprop, or the pick).
#[cfg(feature = "train_value")]
#[inline]
fn leaf_vals(s: &BattleState, evaluator: &impl Evaluator, root_eval: f32, value_temp: f32) -> (f64, f64) {
    if s.is_game_over() {
        let w = winner_value(s);
        return (w, w);
    }
    let e = evaluator.eval(s);
    (sigmoid((e - root_eval) / value_temp), sigmoid(e))
}

// 20 equal bins over [0, 1]; the closed top edge folds into the last bin
#[cfg(feature = "train_value")]
#[inline]
fn bin20(v: f64) -> usize {
    ((v * 20.0) as usize).min(19)
}

#[inline]
pub(crate) fn arm0(a: u8) -> usize { if a == NO_ARM { 0 } else { a as usize } }

// The prior reaches the root node's decider bandit and nothing else: a prior on the
// opponent's bandit would model it as choosing by our policy.
#[inline]
fn bandit_priors<'a>(
    idx: usize,
    decider_side: usize,
    prior: Option<&'a [f32]>,
) -> (Option<&'a [f32]>, Option<&'a [f32]>) {
    match (idx, decider_side) {
        (0, 0) => (prior, None),
        (0, _) => (None, prior),
        _ => (None, None),
    }
}

#[inline]
pub(crate) fn pick(b: &crate::node::Bandit, parent_visits: u32, explore_coeff: f64) -> (u8, u8) {
    pick_with(b, parent_visits, explore_coeff, None)
}

#[inline]
pub(crate) fn pick_with(b: &crate::node::Bandit, parent_visits: u32, explore_coeff: f64, prior: Option<&[f32]>) -> (u8, u8) {
    if b.is_empty() { return (NO_ARM, 0); } // engine ignores the non-acting side's byte
    let i = select_arm(b, parent_visits, explore_coeff, prior);
    (i as u8, b.arms[i].action)
}

fn arms_match(node: &Node, state: &BattleState) -> bool {
    let la1 = legal_actions(state, 0);
    let la2 = legal_actions(state, 1);
    same_actions(&node.s1, &la1) && same_actions(&node.s2, &la2)
}

fn same_actions(b: &crate::node::Bandit, l: &ActionList) -> bool {
    if b.len != l.count { return false; }
    (0..b.len as usize).all(|i| b.arms[i].action == l.actions[i])
}

#[inline]
fn parent_visits(node: &Node, blind: bool, decider_side: usize) -> (u32, u32) {
    match (blind, decider_side) {
        (false, _) => (node.visits, node.visits),
        (true, 0) => (node.visits, node.blind_visits),
        (true, _) => (node.blind_visits, node.visits),
    }
}

pub fn same_root_shape(a: &BattleState, b: &BattleState) -> bool {
    a.phase == b.phase && (0..2).all(|s| legal_actions(a, s).as_slice() == legal_actions(b, s).as_slice())
}

fn side_mut(n: &mut Node, side: usize) -> &mut Bandit {
    if side == 0 { &mut n.s1 } else { &mut n.s2 }
}

// the opponent's bandit belongs to the blind part: its arms come from the blinded state
fn init_blind_part(n: &mut Node, p: &mut BlindPart, state: &BattleState, decider_side: usize) {
    p.phase = state.phase;
    p.us = Bandit::from_actions(&legal_actions(state, decider_side));
    *side_mut(n, 1 - decider_side) = Bandit::from_actions(&legal_actions(state, 1 - decider_side));
}

fn init_true_part(n: &mut Node, state: &BattleState, decider_side: usize) {
    n.phase = state.phase;
    *side_mut(n, decider_side) = Bandit::from_actions(&legal_actions(state, decider_side));
}

// a node a blinded rollout creates owns only the blind part; one a true rollout creates only the true part
fn own_part(n: &mut Node, blinded: bool, decider_side: usize) -> BlindPart {
    if blinded {
        let p = BlindPart { phase: n.phase, us: *side_mut(n, decider_side) };
        n.phase = NO_PART;
        p
    } else {
        BlindPart { phase: NO_PART, us: Bandit::default() }
    }
}

// whether this rollout stops at `n`: its part was missing (initialised here, as an expansion) or
// the part's shape differs from `cur` (a guard hit)
fn part_mismatch(n: &mut Node, p: &mut BlindPart, cur: &BattleState, blinded: bool, decider_side: usize, guard_hits: &mut u64) -> bool {
    let o = 1 - decider_side;
    if blinded && p.phase == NO_PART {
        init_blind_part(n, p, cur, decider_side);
        return true;
    }
    if !blinded && n.phase == NO_PART {
        init_true_part(n, cur, decider_side);
        return true;
    }
    let (phase, us) = if blinded { (p.phase, &p.us) } else { (n.phase, side_of(n, decider_side)) };
    let hit = phase != cur.phase
        || !same_actions(us, &legal_actions(cur, decider_side))
        || !same_actions(side_of(n, o), &legal_actions(cur, o));
    *guard_hits += hit as u64;
    hit
}

fn side_of(n: &Node, side: usize) -> &Bandit {
    if side == 0 { &n.s1 } else { &n.s2 }
}

fn credit_blind_part(n: &mut Node, p: &mut BlindPart, step: &PathStep, value: f64, decider_side: usize) {
    n.blind_visits += 1;
    let (ad, ao, vd) = if decider_side == 0 { (step.a1, step.a2, value) } else { (step.a2, step.a1, 1.0 - value) };
    if ad != NO_ARM {
        let arm = &mut p.us.arms[ad as usize];
        arm.visits += 1;
        arm.total_score += vd;
    }
    if ao != NO_ARM {
        let arm = &mut side_mut(n, 1 - decider_side).arms[ao as usize];
        arm.visits += 1;
        arm.total_score += 1.0 - vd;
    }
}

pub(crate) fn harvest_bandits(s1: &crate::node::Bandit, s2: &crate::node::Bandit, iterations: u64, guard_hits: u64, depth_sum: u64) -> SearchResult {
    let stat = |b: &crate::node::Bandit| {
        (0..b.len as usize)
            .map(|i| {
                let a = &b.arms[i];
                ArmStat {
                    action: a.action,
                    visits: a.visits,
                    avg_score: if a.visits > 0 { a.total_score / a.visits as f64 } else { 0.0 },
                    win_chance: 0.0,
                }
            })
            .collect()
    };
    SearchResult {
        s1: stat(s1),
        s2: stat(s2),
        iterations,
        guard_hits,
        depth_sum,
        principal: NO_PRINCIPAL,
        #[cfg(feature = "train_value")]
        value_sum: 0.0,
        #[cfg(feature = "train_value")]
        value_count: 0,
        #[cfg(feature = "train_value")]
        leaf_hist: [0; 20],
    }
}

pub(crate) fn harvest(tree: &[Node], iterations: u64, guard_hits: u64, depth_sum: u64) -> SearchResult {
    let mut res = harvest_bandits(&tree[0].s1, &tree[0].s2, iterations, guard_hits, depth_sum);
    res.principal = principal_path(tree);
    res
}

// max visits, ties to the lowest action byte -- the rule the raw-root pick already uses
fn best_arm(b: &crate::node::Bandit) -> (u8, u8) {
    if b.is_empty() { return (NO_ARM, NO_ARM); }
    let mut best = 0usize;
    for i in 1..b.len as usize {
        let (x, y) = (&b.arms[i], &b.arms[best]);
        if x.visits > y.visits || (x.visits == y.visits && x.action < y.action) { best = i; }
    }
    (best as u8, b.arms[best].action)
}

fn principal_path(tree: &[Node]) -> [(u8, u8); 3] {
    let mut out = NO_PRINCIPAL;
    let mut idx = 0usize;
    for ply in out.iter_mut() {
        let node = &tree[idx];
        let (i1, a1) = best_arm(&node.s1);
        let (i2, a2) = best_arm(&node.s2);
        if a1 == NO_ARM && a2 == NO_ARM { break; }
        *ply = (a1, a2);
        let child = node.children[child_key(arm0(i1), arm0(i2))];
        if child == NO_CHILD { break; }
        idx = child as usize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chance::OpenLoop;
    use crate::eval::Handcrafted;
    use crate::testutil::*;

    fn run(s: &BattleState, t: &TeamData, ms: u64, iters: u64, seed: u64) -> SearchResult {
        run_prior(s, t, ms, iters, seed, 0, None)
    }

    fn run_prior(
        s: &BattleState,
        t: &TeamData,
        ms: u64,
        iters: u64,
        seed: u64,
        decider_side: usize,
        prior: Option<&[f32]>,
    ) -> SearchResult {
        let p = SearchParams { time_ms: ms, max_iters: iters, ..Default::default() };
        search_world(s, t, &Handcrafted, &OpenLoop, &p, seed, decider_side, prior)
    }

    // side 0 holds one mon with one move (exactly one legal action); side 1 holds
    // three moves plus a live bench slot
    fn one_action_side0() -> (BattleState, TeamData) {
        build_state(
            vec![mon(25, 9, [85, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 33, 0]), mon(130, 22, [57, 0, 0, 0])],
        )
    }

    #[test]
    fn bandit_priors_reach_the_root_decider_only() {
        let p = [0.25f32; 14];
        let some = Some(&p[..]);
        let at = |idx, side| {
            let (a, b) = bandit_priors(idx, side, some);
            (a.is_some(), b.is_some())
        };
        assert_eq!(at(0, 0), (true, false), "root, decider side 0 -> s1 only");
        assert_eq!(at(0, 1), (false, true), "root, decider side 1 -> s2 only");
        assert_eq!(at(1, 0), (false, false), "non-root -> neither");
        assert_eq!(at(1, 1), (false, false), "non-root -> neither");
        assert_eq!(at(7, 0), (false, false), "non-root -> neither");
        let (a, b) = bandit_priors(0, 0, None);
        assert!(a.is_none() && b.is_none(), "no prior -> neither");
    }

    #[test]
    fn zero_prior_collapses_the_root_decider_bandit_to_greedy() {
        // an all-zero prior kills the exploration term at the root, so PUCT scores Q
        // alone and every iteration re-picks the first arm; UCB1 must still spread
        let (s, t) = duel(mon(25, 9, [85, 150, 33, 34]), mon(445, 24, [89, 0, 0, 0]));
        let zero = [0.0f32; 14];
        let r = run_prior(&s, &t, 10_000, 1200, 5, 0, Some(&zero));
        let a0 = r.s1.iter().find(|a| a.action == 0).expect("byte 0 must be legal");
        assert_eq!(a0.visits as u64, r.iterations, "zero prior -> Q-greedy root bandit");
        let spread = run_prior(&s, &t, 10_000, 1200, 5, 0, None);
        assert!(
            spread.s1.iter().filter(|a| a.visits > 0).count() > 1,
            "UCB1 control must visit more than one root arm"
        );
    }

    #[test]
    fn root_prior_routes_to_the_deciders_bandit_only() {
        let (s, t) = one_action_side0();
        assert_eq!(legal_actions(&s, 0).count, 1, "fixture: side 0 has one legal action");
        assert!(legal_actions(&s, 1).count >= 2, "fixture: side 1 branches");
        let zero = [0.0f32; 14];
        let base = run_prior(&s, &t, 10_000, 1500, 11, 1, None);
        let steered = run_prior(&s, &t, 10_000, 1500, 11, 1, Some(&zero));
        assert_eq!(
            steered.s2[0].visits as u64, steered.iterations,
            "naming side 1 the decider must put PUCT on its root bandit"
        );
        assert!(
            base.s2.iter().filter(|a| a.visits > 0).count() > 1,
            "UCB1 control must spread side 1's root bandit"
        );
        // the same prior with side 0 named decider must not reach side 1's bandit
        let other = run_prior(&s, &t, 10_000, 1500, 11, 0, Some(&zero));
        assert_eq!(base.iterations, other.iterations);
        for (a, b) in base.s2.iter().zip(other.s2.iter()) {
            assert_eq!(a.action, b.action);
            assert_eq!(a.visits, b.visits, "opponent root arm visits must not move");
            assert_eq!(
                a.avg_score.to_bits(),
                b.avg_score.to_bits(),
                "opponent root arm value must not move"
            );
        }
    }

    #[test]
    fn opponent_root_bandit_is_byte_identical_prior_on_vs_off() {
        use crate::node::{Bandit, MoveNode};
        let (s, t) = one_action_side0();
        let decider = 0usize;
        assert_eq!(legal_actions(&s, decider).count, 1, "fixture: the decider has one legal action");
        assert!(legal_actions(&s, 1 - decider).count >= 2, "fixture: the opponent has two or more");
        // peaked on an opponent-legal byte: a uniform prior would score every arm alike and
        // a leak onto both root bandits would leave no trace
        let mut p = [0.001f32; 14];
        p[2] = 0.95;
        let off = run_prior(&s, &t, 10_000, 1500, 11, decider, None);
        let on = run_prior(&s, &t, 10_000, 1500, 11, decider, Some(&p));
        assert_eq!(off.iterations, on.iterations);
        assert_eq!(off.side(1 - decider).len(), on.side(1 - decider).len(), "opponent arm count");
        assert!(off.side(1 - decider).len() >= 2);
        for (a, b) in off.side(1 - decider).iter().zip(on.side(1 - decider).iter()) {
            assert_eq!(a.action, b.action, "opponent root arm byte");
            assert_eq!(a.visits, b.visits, "opponent root arm visits");
            assert_eq!(a.avg_score.to_bits(), b.avg_score.to_bits(), "opponent root arm score");
        }
        for (a, b) in off.side(decider).iter().zip(on.side(decider).iter()) {
            assert_eq!(a.visits, b.visits, "the single decider arm cannot move either");
            assert_eq!(a.avg_score.to_bits(), b.avg_score.to_bits());
        }
        // A one-arm bandit returns index 0 under either rule, so the decider's slot is inert
        // here and routing the prior to the opponent alone reproduces a both-bandit leak.
        for pv in [1u32, 2, 37, 1500] {
            for v in [0u32, 1, 900] {
                let mut b = Bandit::default();
                b.arms[0] = MoveNode { action: 0, total_score: 0.3 * v as f64, visits: v };
                b.len = 1;
                assert_eq!(select_arm(&b, pv, 2.0, Some(&p[..])), 0);
                assert_eq!(select_arm(&b, pv, 2.0, None), 0);
            }
        }
        let leak = run_prior(&s, &t, 10_000, 1500, 11, 1 - decider, Some(&p));
        let moved = off
            .side(1 - decider)
            .iter()
            .zip(leak.side(1 - decider).iter())
            .filter(|(a, b)| a.visits != b.visits || a.avg_score.to_bits() != b.avg_score.to_bits())
            .count();
        println!(
            "opponent-invariance decider_legal={} opponent_arms={} iterations={} \
             opponent_visits_off={:?} opponent_visits_on={:?} leak_visits={:?} leak_arms_moved={moved}",
            legal_actions(&s, decider).count,
            off.side(1 - decider).len(),
            off.iterations,
            off.side(1 - decider).iter().map(|a| a.visits).collect::<Vec<_>>(),
            on.side(1 - decider).iter().map(|a| a.visits).collect::<Vec<_>>(),
            leak.side(1 - decider).iter().map(|a| a.visits).collect::<Vec<_>>(),
        );
        assert!(moved > 0, "a prior reaching the opponent's root bandit must break the stream");
    }

    #[test]
    fn finds_the_kill() {
        // Opponent at 1 HP: Thunderbolt (slot 0) always KOs; Splash (slot 1) never does.
        // Gyarados (Water/Flying) is 4x weak to Electric; a Ground-type target would be immune.
        let (mut s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(130, 22, [58, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 1;
        let r = run(&s, &t, 1000, 3000, 42);
        let bolt = r.s1.iter().find(|a| a.action == 0).unwrap();
        let total: u32 = r.s1.iter().map(|a| a.visits).sum();
        assert!(bolt.visits as f64 / total as f64 > 0.7, "KO move must dominate visits");
    }

    #[test]
    fn finds_the_kill_side2() {
        // mirror of finds_the_kill: side 2 must KO; pins the 1-v backprop sign
        let (mut s, t) = duel(mon(130, 22, [58, 0, 0, 0]), mon(25, 9, [85, 150, 0, 0]));
        s.sides[0].team[0].current_hp = 1;
        let r = run(&s, &t, 1000, 3000, 42);
        let bolt = r.s2.iter().find(|a| a.action == 0).unwrap();
        let total: u32 = r.s2.iter().map(|a| a.visits).sum();
        assert!(bolt.visits as f64 / total as f64 > 0.7, "KO move must dominate s2 visits");
    }

    #[test]
    fn reproducible_under_seed() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 58, 0, 0]));
        let a = run(&s, &t, 1000, 2000, 7);
        let b = run(&s, &t, 1000, 2000, 7);
        for (x, y) in a.s1.iter().zip(b.s1.iter()) {
            assert_eq!(x.visits, y.visits);
            assert_eq!(x.avg_score, y.avg_score);
        }
        assert_eq!(a.iterations, b.iterations);
    }

    #[test]
    fn visit_conservation_and_termination() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        let r = run(&s, &t, 10_000, 500, 1);
        assert_eq!(r.iterations, 500, "max_iters cap respected");
        let total: u64 = r.s1.iter().map(|a| a.visits as u64).sum();
        assert_eq!(total, 500, "every iteration credits exactly one root arm");
    }

    #[cfg(feature = "train_value")]
    #[test]
    fn accumulator_tracks_every_iteration_in_range() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        let r = run(&s, &t, 10_000, 500, 1);
        assert_eq!(r.value_count, r.iterations, "one absolute leaf value per iteration");
        let mean = r.value_sum / r.value_count as f64;
        assert!((0.0..=1.0).contains(&mean), "mean leaf value {mean} outside [0,1]");
    }

    #[test]
    fn best_arm_takes_max_visits_and_the_lowest_byte_on_ties() {
        use crate::node::{Bandit, MoveNode};
        let mut b = Bandit::default();
        for (i, (action, visits)) in [(4u8, 10u32), (0, 10), (1, 9)].into_iter().enumerate() {
            b.arms[i] = MoveNode { action, total_score: 0.0, visits };
            b.len += 1;
        }
        assert_eq!(best_arm(&b), (1, 0), "tie goes to the lowest byte, at its own arm index");
        b.arms[2].visits = 30;
        assert_eq!(best_arm(&b), (2, 1));
        assert_eq!(best_arm(&Bandit::default()), (NO_ARM, NO_ARM));
    }

    // both sides keep a live bench, so the max-visit joint action does not always end the game
    fn benched_root() -> (BattleState, TeamData) {
        build_state(
            vec![mon(143, 47, [34, 89, 33, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(130, 22, [57, 85, 33, 0]), mon(445, 24, [89, 14, 0, 0])],
        )
    }

    #[test]
    fn principal_follows_the_max_visit_joint_path() {
        let (s, t) = benched_root();
        let r = run(&s, &t, 10_000, 20_000, 3);
        let top = |arms: &[ArmStat]| {
            arms.iter().max_by(|a, b| a.visits.cmp(&b.visits).then(b.action.cmp(&a.action))).unwrap().action
        };
        assert_eq!(r.principal[0], (top(&r.s1), top(&r.s2)), "ply 1 is the root's max-visit joint arm");
        for (d, ply) in r.principal.iter().enumerate() {
            assert_ne!(*ply, (NO_ARM, NO_ARM), "ply {} must exist after 20,000 iterations", d + 1);
        }
    }

    #[test]
    fn principal_is_absent_at_a_terminal_root() {
        let (mut s, t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        s.phase = PHASE_GAME_OVER;
        assert_eq!(run(&s, &t, 10, 100, 1).principal, NO_PRINCIPAL);
    }

    #[test]
    fn principal_marks_the_side_with_no_bandit() {
        let (mut s, t) = benched_root();
        s.sides[0].team[0].current_hp = 0;
        s.phase = PHASE_SWITCH_P1;
        assert_eq!(legal_actions(&s, 1).count, 0, "fixture: side 1 does not act in this phase");
        let r = run(&s, &t, 10_000, 2_000, 4);
        assert_eq!(r.principal[0].1, NO_ARM, "the non-acting side carries no byte");
        assert_ne!(r.principal[0].0, NO_ARM, "the replacing side does");
    }

    #[test]
    fn terminal_root_searches_without_panic() {
        let (mut s, t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        s.phase = PHASE_GAME_OVER;
        let r = run(&s, &t, 10, 100, 1);
        assert_eq!(r.iterations, 0, "no legal arms at a terminal root");
    }

    fn bytes(arms: &[ArmStat]) -> Vec<u8> {
        arms.iter().map(|a| a.action).collect()
    }

    fn visits(arms: &[ArmStat]) -> Vec<(u8, u32)> {
        arms.iter().map(|a| (a.action, a.visits)).collect()
    }

    fn total_visits(arms: &[ArmStat]) -> u64 {
        arms.iter().map(|a| a.visits as u64).sum()
    }

    // the blinded root re-samples one side's moves (same counts, so both roots expose the same arms)
    fn blinded_root(side: usize) -> (BattleState, TeamData) {
        let ours = vec![mon(143, 47, [85, 150, 57, 0]), mon(25, 9, [34, 89, 0, 0])];
        let theirs = vec![mon(130, 22, [14, 89, 85, 0]), mon(445, 24, [57, 33, 0, 0])];
        let (truth, _) = benched_root();
        let (p1, p2) = if side == 0 {
            (ours, vec![mon(130, 22, [57, 85, 33, 0]), mon(445, 24, [89, 14, 0, 0])])
        } else {
            (vec![mon(143, 47, [34, 89, 33, 0]), mon(25, 9, [85, 150, 0, 0])], theirs)
        };
        let out = build_state(p1, p2);
        assert!(same_root_shape(&truth, &out.0), "fixture: both roots expose the same arms");
        assert_ne!(truth.sides[side].team[0].moves, out.0.sides[side].team[0].moves, "fixture: side {side} differs");
        assert!(truth.sides[1 - side] == out.0.sides[1 - side], "fixture: side {} is unchanged", 1 - side);
        out
    }

    #[test]
    fn blinded_search_keeps_the_deciders_root_arms_and_splits_the_rollouts() {
        let (s, t) = benched_root();
        let (bs, bt) = blinded_root(0);
        let p = SearchParams { time_ms: 10_000, max_iters: 2_000, ..Default::default() };
        let plain = search_world(&s, &t, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        let blind = search_world_blind(&s, &t, &bs, &bt, BlindMode::Shared, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        assert_eq!(blind.iterations, 2_000);
        assert_eq!(bytes(&blind.s1), bytes(&plain.s1), "our root arms keep their bytes");
        assert_eq!(bytes(&blind.s1), legal_actions(&s, 0).as_slice());
        assert_eq!(bytes(&blind.s2), bytes(&plain.s2));
        assert_eq!(total_visits(&blind.s1), 1_000, "the true rollouts credit our arms");
        assert_eq!(total_visits(&blind.s2), 1_000, "the blinded rollouts credit the opponent's");
        assert_ne!(visits(&blind.s2), visits(&plain.s2), "the opponent's statistics come from the blinded root");
    }

    #[test]
    fn the_decider_at_seat_one_is_credited_by_the_true_rollouts() {
        let (s, t) = benched_root();
        let (bs, bt) = blinded_root(1);
        let p = SearchParams { time_ms: 10_000, max_iters: 2_001, ..Default::default() };
        let plain = search_world(&s, &t, &Handcrafted, &OpenLoop, &p, 5, 1, None);
        let blind = search_world_blind(&s, &t, &bs, &bt, BlindMode::Shared, &Handcrafted, &OpenLoop, &p, 5, 1, None);
        assert_eq!(blind.iterations, 2_001);
        assert_eq!(bytes(&blind.s2), bytes(&plain.s2));
        assert_eq!(bytes(&blind.s2), legal_actions(&s, 1).as_slice());
        assert_eq!(total_visits(&blind.s2), 1_001, "even iterations are true rollouts and credit the decider");
        assert_eq!(total_visits(&blind.s1), 1_000, "odd iterations are blinded and credit the opponent");
        assert_ne!(visits(&blind.s1), visits(&plain.s1));
    }

    #[test]
    fn a_blinded_root_of_another_shape_is_ignored() {
        let (s, t) = benched_root();
        let (mut bs, bt) = blinded_root(0);
        bs.sides[0].team[1].current_hp = 0;
        assert!(!same_root_shape(&s, &bs), "fixture: the fainted bench mon removes a switch arm");
        let p = SearchParams { time_ms: 10_000, max_iters: 2_000, ..Default::default() };
        let plain = search_world(&s, &t, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        for mode in [BlindMode::Shared, BlindMode::Split] {
            let blind = search_world_blind(&s, &t, &bs, &bt, mode, &Handcrafted, &OpenLoop, &p, 5, 0, None);
            assert_eq!(blind.iterations, plain.iterations);
            assert_eq!(visits(&blind.s1), visits(&plain.s1), "{mode:?}");
            assert_eq!(visits(&blind.s2), visits(&plain.s2), "{mode:?}");
            assert_eq!(blind.principal, plain.principal);
        }
    }

    // a third side-0 team of the same shape, for the blinded root of the split tests
    fn third_root() -> (BattleState, TeamData) {
        let out = build_state(
            vec![mon(143, 47, [85, 33, 89, 0]), mon(25, 9, [150, 34, 0, 0])],
            vec![mon(130, 22, [57, 85, 33, 0]), mon(445, 24, [89, 14, 0, 0])],
        );
        assert!(same_root_shape(&benched_root().0, &out.0));
        out
    }

    #[test]
    fn split_blinding_makes_the_opponents_statistics_a_function_of_the_blinded_root() {
        let (s, t) = benched_root();
        let (s2, t2) = blinded_root(0);
        let (b, bt) = third_root();
        let p = SearchParams { time_ms: 10_000, max_iters: 2_000, ..Default::default() };
        let r1 = search_world_blind(&s, &t, &b, &bt, BlindMode::Split, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        let r2 = search_world_blind(&s2, &t2, &b, &bt, BlindMode::Split, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        assert_eq!(r1.iterations, 2_000);
        assert_eq!(bytes(&r1.s1), legal_actions(&s, 0).as_slice(), "our root arms keep their bytes");
        assert_eq!(total_visits(&r1.s1), 1_000);
        assert_eq!(total_visits(&r1.s2), 1_000);
        assert_eq!(visits(&r1.s2), visits(&r2.s2), "the opponent's root statistics do not move when our true side changes");
        for (x, y) in r1.s2.iter().zip(&r2.s2) {
            assert_eq!(x.avg_score.to_bits(), y.avg_score.to_bits());
        }
        assert_ne!(visits(&r1.s1), visits(&r2.s1), "our own statistics do");
        let h1 = search_world_blind(&s, &t, &b, &bt, BlindMode::Shared, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        let h2 = search_world_blind(&s2, &t2, &b, &bt, BlindMode::Shared, &Handcrafted, &OpenLoop, &p, 5, 0, None);
        assert_ne!(visits(&h1.s2), visits(&h2.s2), "in the shared mode our true side still reaches the opponent");
    }

    #[test]
    fn split_blinding_at_seat_one_credits_each_part_from_its_own_rollouts() {
        let (s, t) = benched_root();
        let (bs, bt) = blinded_root(1);
        let p = SearchParams { time_ms: 10_000, max_iters: 2_001, ..Default::default() };
        let plain = search_world(&s, &t, &Handcrafted, &OpenLoop, &p, 5, 1, None);
        let blind = search_world_blind(&s, &t, &bs, &bt, BlindMode::Split, &Handcrafted, &OpenLoop, &p, 5, 1, None);
        assert_eq!(bytes(&blind.s2), legal_actions(&s, 1).as_slice());
        assert_eq!(total_visits(&blind.s2), 1_001);
        assert_eq!(total_visits(&blind.s1), 1_000);
        assert_ne!(visits(&blind.s1), visits(&plain.s1));
    }
}
