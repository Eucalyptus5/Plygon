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
