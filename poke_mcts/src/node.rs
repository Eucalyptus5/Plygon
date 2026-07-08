use pkmn_engine::state::{legal_actions, BattleState};

pub const NO_CHILD: u32 = u32::MAX;
// arm ceiling = the engine's ActionList capacity (4 moves + 5 switches + 4 tera)
pub const ARM_CAP: usize = 13;
pub const CHILD_CAP: usize = ARM_CAP * ARM_CAP;

pub struct Node {
    pub phase: u8,
    pub visits: u32,
    pub s1: Bandit,
    pub s2: Bandit,
    pub children: [u32; CHILD_CAP],
}

impl Node {
    pub fn from_state(state: &BattleState) -> Self {
        Node {
            phase: state.phase,
            visits: 0,
            s1: Bandit::from_actions(&legal_actions(state, 0)),
            s2: Bandit::from_actions(&legal_actions(state, 1)),
            children: [NO_CHILD; CHILD_CAP],
        }
    }
}

/// Closed-loop decision node: owns its post-chance state, has no dense child array
/// (chance edges live in the side map keyed by `child_key`). ~1 KB/node (see CLOSED_LOOP_BYTES_PER_NODE).
pub struct CNode {
    pub state: BattleState,
    pub visits: u32,
    pub s1: Bandit,
    pub s2: Bandit,
}

impl CNode {
    pub fn from_state(state: &BattleState) -> Self {
        CNode {
            state: *state,
            visits: 0,
            s1: Bandit::from_actions(&legal_actions(state, 0)),
            s2: Bandit::from_actions(&legal_actions(state, 1)),
        }
    }
}

#[inline(always)]
pub fn child_key(a1_arm: usize, a2_arm: usize) -> usize { a1_arm * ARM_CAP + a2_arm }

#[derive(Clone, Copy, Default)]
pub struct MoveNode {
    pub action: u8,
    pub total_score: f64, // f64: f32 loses precision at ~30M iters (design §5.5)
    pub visits: u32,
}

#[derive(Clone, Copy)]
pub struct Bandit {
    pub arms: [MoveNode; ARM_CAP],
    pub len: u8,
}

impl Default for Bandit {
    fn default() -> Self { Bandit { arms: [MoveNode::default(); ARM_CAP], len: 0 } }
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
        assert_eq!(child_key(3, 7), 3 * ARM_CAP + 7);
        assert_eq!(child_key(0, 0), 0);
        assert_eq!(child_key(ARM_CAP - 1, ARM_CAP - 1), CHILD_CAP - 1);
        // injective + in-bounds over every arm pair (max index 168 < 169)
        let mut seen = [false; CHILD_CAP];
        for a in 0..ARM_CAP {
            for b in 0..ARM_CAP {
                let k = child_key(a, b);
                assert!(k < CHILD_CAP, "child_key({a},{b})={k} out of bounds");
                assert!(!seen[k], "child_key collision at ({a},{b})");
                seen[k] = true;
            }
        }
    }

    #[test]
    fn bandit_holds_full_action_ceiling() {
        let mut list = ActionList::new();
        for a in 0..ARM_CAP as u8 { list.actions[a as usize] = a; }
        list.count = ARM_CAP as u8;
        let b = Bandit::from_actions(&list);
        assert_eq!(b.len as usize, ARM_CAP, "all arms stored without overrun");
        for i in 0..ARM_CAP { assert_eq!(b.arms[i].action, i as u8); }
    }
}
