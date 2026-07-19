use crate::chance::ChanceModel;
use crate::eval::{sigmoid, winner_value, Evaluator};
use crate::node::{child_key, Node, NO_CHILD};
use crate::rng::Lcg;
use crate::select::select_arm;
use pkmn_engine::state::*;
use std::time::Instant;

pub struct SearchParams {
    pub time_ms: u64,
    pub max_iters: u64,   // sanity cap (design §5.5); u64::MAX in normal play
    pub max_nodes: u32,   // memory cap; stop expanding past it
    pub explore_coeff: f64, // UCB sqrt coefficient (c²); 2.0 = textbook c=√2
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams { time_ms: 100, max_iters: u64::MAX, max_nodes: 2_000_000, explore_coeff: 2.0 }
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

#[derive(Clone, Default)]
pub struct SearchResult {
    pub s1: Vec<ArmStat>,
    pub s2: Vec<ArmStat>,
    pub iterations: u64,
    pub guard_hits: u64,
    pub depth_sum: u64,
    #[cfg(feature = "train_value")]
    pub value_sum: f64,
    #[cfg(feature = "train_value")]
    pub value_count: u64,
}

impl SearchResult {
    pub fn side(&self, side: usize) -> &[ArmStat] { if side == 0 { &self.s1 } else { &self.s2 } }
}

struct PathStep { node: usize, a1: u8, a2: u8 } // arm indices; 255 = side had no bandit
const NO_ARM: u8 = 255;

pub fn search_world(
    root_state: &BattleState,
    teams: &TeamData,
    evaluator: &impl Evaluator,
    chance: &impl ChanceModel,
    params: &SearchParams,
    seed: u64,
) -> SearchResult {
    let mut rng = Lcg::new(seed);
    let mut tree: Vec<Node> = Vec::with_capacity(4096);
    tree.push(Node::from_state(root_state));
    if root_state.is_game_over() || (tree[0].s1.is_empty() && tree[0].s2.is_empty()) {
        return harvest(&tree[0], 0, 0, 0);
    }
    let root_eval = evaluator.eval(root_state) as f32;
    let start = Instant::now();
    let mut iters: u64 = 0;
    let mut guard_hits: u64 = 0;
    let mut depth_sum: u64 = 0;
    let mut path: Vec<PathStep> = Vec::with_capacity(64);
    #[cfg(feature = "train_value")]
    let (mut value_sum, mut value_count) = (0.0f64, 0u64);
    // Per-root-arm provable terminal-win branch-chance (analytic root only; stays 0.0 otherwise).
    let mut win_s1 = [0.0f64; crate::node::ARM_CAP];
    let mut win_s2 = [0.0f64; crate::node::ARM_CAP];

    'outer: while iters < params.max_iters {
        // clock check batched to amortize the read (design §5.6)
        if iters % 1024 == 0 && start.elapsed().as_millis() as u64 >= params.time_ms && iters > 0 {
            break 'outer;
        }
        path.clear();
        let mut cur = *root_state;
        let mut idx = 0usize;
        let value: f64;
        #[cfg(feature = "train_value")]
        let value_abs: f64;

        loop {
            // arm-mismatch guard: this sample diverged from the node's recorded shape
            // (provably never fires at the root, where cur is the untouched root state)
            if idx != 0 && (tree[idx].phase != cur.phase || !arms_match(&tree[idx], &cur)) {
                guard_hits += 1;
                #[cfg(not(feature = "train_value"))]
                {
                    value = leaf(&cur, evaluator, root_eval);
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval);
                    value = v;
                    value_abs = a;
                }
                break;
            }
            let node = &tree[idx];
            let (a1, b1) = pick(&node.s1, node.visits, params.explore_coeff);
            let (a2, b2) = pick(&node.s2, node.visits, params.explore_coeff);
            if a1 == NO_ARM && a2 == NO_ARM {
                #[cfg(not(feature = "train_value"))]
                {
                    value = leaf(&cur, evaluator, root_eval);
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval);
                    value = v;
                    value_abs = a;
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
                    let r = rng.roll(crate::chance_analytic::WEIGHT_SCALE);
                    crate::chance_analytic::pick_weighted(&children, r)
                } else {
                    chance.transition(&cur, teams, b1, b2, &mut |m| rng.roll(m))
                }
            } else {
                chance.transition(&cur, teams, b1, b2, &mut |m| rng.roll(m))
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
                        leaf(&cur, evaluator, root_eval)
                    };
                }
                #[cfg(feature = "train_value")]
                {
                    let (v, a) = leaf_vals(&cur, evaluator, root_eval);
                    value = v;
                    value_abs = a;
                }
                if (tree.len() as u32) < params.max_nodes && !cur.is_game_over() {
                    tree.push(Node::from_state(&cur));
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
        for step in path.iter() {
            let n = &mut tree[step.node];
            n.visits += 1;
            if step.a1 != NO_ARM {
                let arm = &mut n.s1.arms[step.a1 as usize];
                arm.visits += 1;
                arm.total_score += value;
            }
            if step.a2 != NO_ARM {
                let arm = &mut n.s2.arms[step.a2 as usize];
                arm.visits += 1;
                arm.total_score += 1.0 - value;
            }
        }
        iters += 1;
    }

    let mut res = harvest(&tree[0], iters, guard_hits, depth_sum);
    #[cfg(feature = "train_value")]
    {
        res.value_sum = value_sum;
        res.value_count = value_count;
    }
    for (i, a) in res.s1.iter_mut().enumerate() { a.win_chance = win_s1[i]; }
    for (i, a) in res.s2.iter_mut().enumerate() { a.win_chance = win_s2[i]; }
    res
}

#[inline]
pub(crate) fn leaf(s: &BattleState, evaluator: &impl Evaluator, root_eval: f32) -> f64 {
    if s.is_game_over() { winner_value(s) } else { sigmoid(evaluator.eval(s) - root_eval) }
}

// One eval per leaf: the root-relative sigmoid backprops, the absolute sigmoid only
// feeds the value accumulator (never UCB, backprop, or the pick).
#[cfg(feature = "train_value")]
#[inline]
fn leaf_vals(s: &BattleState, evaluator: &impl Evaluator, root_eval: f32) -> (f64, f64) {
    if s.is_game_over() {
        let w = winner_value(s);
        return (w, w);
    }
    let e = evaluator.eval(s);
    (sigmoid(e - root_eval), sigmoid(e))
}

#[inline]
pub(crate) fn arm0(a: u8) -> usize { if a == NO_ARM { 0 } else { a as usize } }

#[inline]
pub(crate) fn pick(b: &crate::node::Bandit, parent_visits: u32, explore_coeff: f64) -> (u8, u8) {
    if b.is_empty() { return (NO_ARM, 0); } // engine ignores the non-acting side's byte
    let i = select_arm(b, parent_visits, explore_coeff);
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
        #[cfg(feature = "train_value")]
        value_sum: 0.0,
        #[cfg(feature = "train_value")]
        value_count: 0,
    }
}

pub(crate) fn harvest(root: &Node, iterations: u64, guard_hits: u64, depth_sum: u64) -> SearchResult {
    harvest_bandits(&root.s1, &root.s2, iterations, guard_hits, depth_sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chance::OpenLoop;
    use crate::eval::Handcrafted;
    use crate::testutil::*;

    fn run(s: &BattleState, t: &TeamData, ms: u64, iters: u64, seed: u64) -> SearchResult {
        let p = SearchParams { time_ms: ms, max_iters: iters, ..Default::default() };
        search_world(s, t, &Handcrafted, &OpenLoop, &p, seed)
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
    fn terminal_root_searches_without_panic() {
        let (mut s, t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        s.phase = PHASE_GAME_OVER;
        let r = run(&s, &t, 10, 100, 1);
        assert_eq!(r.iterations, 0, "no legal arms at a terminal root");
    }
}
