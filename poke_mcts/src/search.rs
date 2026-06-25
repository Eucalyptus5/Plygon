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
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams { time_ms: 100, max_iters: u64::MAX, max_nodes: 2_000_000 }
    }
}

#[derive(Clone, Copy, Default)]
pub struct ArmStat { pub action: u8, pub visits: u32, pub avg_score: f64 }

#[derive(Clone, Default)]
pub struct SearchResult {
    pub s1: Vec<ArmStat>,
    pub s2: Vec<ArmStat>,
    pub iterations: u64,
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
        return harvest(&tree[0], 0);
    }
    let root_eval = evaluator.eval(root_state) as f32;
    let start = Instant::now();
    let mut iters: u64 = 0;
    let mut path: Vec<PathStep> = Vec::with_capacity(64);

    'outer: while iters < params.max_iters {
        // clock check batched to amortize the read (design §5.6)
        if iters % 1024 == 0 && start.elapsed().as_millis() as u64 >= params.time_ms && iters > 0 {
            break 'outer;
        }
        path.clear();
        let mut cur = *root_state;
        let mut idx = 0usize;
        let value: f64;

        loop {
            // arm-mismatch guard: this sample diverged from the node's recorded shape
            // (provably never fires at the root, where cur is the untouched root state)
            if idx != 0 && (tree[idx].phase != cur.phase || !arms_match(&tree[idx], &cur)) {
                value = leaf(&cur, evaluator, root_eval);
                break;
            }
            let node = &tree[idx];
            let (a1, b1) = pick(&node.s1, node.visits);
            let (a2, b2) = pick(&node.s2, node.visits);
            if a1 == NO_ARM && a2 == NO_ARM {
                value = leaf(&cur, evaluator, root_eval);
                break;
            }
            cur = chance.transition(&cur, teams, b1, b2, &mut |m| rng.roll(m));
            path.push(PathStep { node: idx, a1, a2 });

            let key = child_key(arm0(a1), arm0(a2));
            let child = tree[idx].children[key];
            if child == NO_CHILD {
                value = if cur.is_game_over() {
                    winner_value(&cur)
                } else {
                    leaf(&cur, evaluator, root_eval)
                };
                if (tree.len() as u32) < params.max_nodes && !cur.is_game_over() {
                    tree.push(Node::from_state(&cur));
                    let new_idx = (tree.len() - 1) as u32;
                    tree[idx].children[key] = new_idx;
                }
                break;
            }
            if cur.is_game_over() {
                value = winner_value(&cur);
                break;
            }
            idx = child as usize;
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

    harvest(&tree[0], iters)
}

#[inline]
pub(crate) fn leaf(s: &BattleState, evaluator: &impl Evaluator, root_eval: f32) -> f64 {
    if s.is_game_over() { winner_value(s) } else { sigmoid(evaluator.eval(s) - root_eval) }
}

#[inline]
pub(crate) fn arm0(a: u8) -> usize { if a == NO_ARM { 0 } else { a as usize } }

#[inline]
pub(crate) fn pick(b: &crate::node::Bandit, parent_visits: u32) -> (u8, u8) {
    if b.is_empty() { return (NO_ARM, 0); } // engine ignores the non-acting side's byte
    let i = select_arm(b, parent_visits);
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

pub(crate) fn harvest(root: &Node, iterations: u64) -> SearchResult {
    let stat = |b: &crate::node::Bandit| {
        (0..b.len as usize)
            .map(|i| {
                let a = &b.arms[i];
                ArmStat {
                    action: a.action,
                    visits: a.visits,
                    avg_score: if a.visits > 0 { a.total_score / a.visits as f64 } else { 0.0 },
                }
            })
            .collect()
    };
    SearchResult { s1: stat(&root.s1), s2: stat(&root.s2), iterations }
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

    #[test]
    fn terminal_root_searches_without_panic() {
        let (mut s, t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        s.phase = PHASE_GAME_OVER;
        let r = run(&s, &t, 10, 100, 1);
        assert_eq!(r.iterations, 0, "no legal arms at a terminal root");
    }
}
