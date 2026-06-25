use pkmn_engine::state::*;

pub trait ChanceModel {
    fn transition(
        &self,
        parent: &BattleState,
        teams: &TeamData,
        a1: u8,
        a2: u8,
        rng: &mut impl FnMut(u32) -> u32,
    ) -> BattleState;
}

pub struct OpenLoop;

impl ChanceModel for OpenLoop {
    fn transition(
        &self,
        parent: &BattleState,
        teams: &TeamData,
        a1: u8,
        a2: u8,
        rng: &mut impl FnMut(u32) -> u32,
    ) -> BattleState {
        let mut s = *parent;
        match s.phase {
            PHASE_ACTIONS => execute_turn(&mut s, teams, a1, a2, rng),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(&mut s, teams, a1, a2, rng)
            }
            _ => {}
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Lcg;
    use crate::testutil::*;

    #[test]
    fn transition_steps_and_is_seed_deterministic() {
        let (s, t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        let mut r1 = Lcg::new(7);
        let mut r2 = Lcg::new(7);
        let c1 = OpenLoop.transition(&s, &t, 0, 0, &mut |m| r1.roll(m));
        let c2 = OpenLoop.transition(&s, &t, 0, 0, &mut |m| r2.roll(m));
        assert!(c1 == c2, "same seed, same child");
        assert!(c1 != s, "turn advanced");
        assert_eq!(s.phase, PHASE_ACTIONS, "parent untouched (Copy clone)");
    }

    #[test]
    fn transition_dispatches_switch_phase() {
        let (s, t) = build_state(
            vec![mon(25, 9, [85, 0, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 0, 0, 0])],
        );
        let mut forced = s;
        forced.sides[0].team[0].current_hp = 0;
        forced.phase = PHASE_SWITCH_P1;
        let mut r = Lcg::new(7);
        let c = OpenLoop.transition(&forced, &t, ACTION_SWITCH_0 + 1, 0, &mut |m| r.roll(m));
        assert_eq!(c.sides[0].active_index, 1, "forced switch executed");
    }
}
