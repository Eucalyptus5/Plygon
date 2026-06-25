use pkmn_engine::state::{legal_actions, BattleState};

pub const NO_CHILD: u32 = u32::MAX;

pub struct Node {
    pub phase: u8,
    pub visits: u32,
    pub s1: Bandit,
    pub s2: Bandit,
    pub children: [u32; 100],
}

impl Node {
    pub fn from_state(state: &BattleState) -> Self {
        Node {
            phase: state.phase,
            visits: 0,
            s1: Bandit::from_actions(&legal_actions(state, 0)),
            s2: Bandit::from_actions(&legal_actions(state, 1)),
            children: [NO_CHILD; 100],
        }
    }
}

#[inline(always)]
pub fn child_key(a1_arm: usize, a2_arm: usize) -> usize { a1_arm * 10 + a2_arm }

#[derive(Clone, Copy, Default)]
pub struct MoveNode {
    pub action: u8,
    pub total_score: f64, // f64: f32 loses precision at ~30M iters (design §5.5)
    pub visits: u32,
}

#[derive(Clone, Copy)]
pub struct Bandit {
    pub arms: [MoveNode; 10],
    pub len: u8,
}

impl Default for Bandit {
    fn default() -> Self { Bandit { arms: [MoveNode::default(); 10], len: 0 } }
}

impl Bandit {
    pub fn from_actions(list: &pkmn_engine::state::ActionList) -> Self {
        let mut b = Bandit::default();
        for &a in list.as_slice() {
            b.arms[b.len as usize] = MoveNode { action: a, total_score: 0.0, visits: 0 };
            b.len += 1;
        }
        b
    }
    pub fn is_empty(&self) -> bool { self.len == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use pkmn_engine::state::*;

    #[test]
    fn node_bandit_layout_follows_phase() {
        let (s, _t) = build_state(
            vec![mon(25, 9, [85, 150, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 0, 0, 0]), mon(130, 22, [57, 0, 0, 0])],
        );
        let n = Node::from_state(&s);
        assert_eq!(n.phase, PHASE_ACTIONS);
        assert!(!n.s1.is_empty() && !n.s2.is_empty());

        let mut forced = s;
        forced.sides[0].team[0].current_hp = 0;
        forced.phase = PHASE_SWITCH_P1;
        let n = Node::from_state(&forced);
        assert!(!n.s1.is_empty(), "acting side has switch arms");
        assert!(n.s2.is_empty(), "non-acting side has no bandit (legal_moves.rs `_ =>` arm)");
        assert!(n.children.iter().all(|&c| c == NO_CHILD));
    }

    #[test]
    fn child_key_stride() {
        assert_eq!(child_key(3, 7), 37);
        assert_eq!(child_key(0, 0), 0);
        assert_eq!(child_key(9, 9), 99);
    }
}
