use crate::rng::Lcg;
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::*;

pub fn random_action(state: &BattleState, side: usize, rng: &mut Lcg) -> u8 {
    let legal = legal_actions(state, side);
    if legal.count == 0 { return ACTION_STRUGGLE; } // byte-255 noop convention
    legal.actions[rng.roll(legal.count as u32) as usize]
}

// Median damage roll, no crit, guaranteed hit — same convention as the harness
// calc_damage mode (run_scenario main.rs:1099-1178).
pub fn median_roll(max: u32) -> u32 {
    match max { 16 => 7, 24 => 23, 100 => 0, _ => 0 }
}

pub fn greedy_action(state: &BattleState, side: usize, rng: &mut Lcg) -> u8 {
    let legal = legal_actions(state, side);
    if legal.count == 0 { return ACTION_STRUGGLE; }
    if state.phase != PHASE_ACTIONS {
        return legal.actions[rng.roll(legal.count as u32) as usize];
    }
    let moves = effective_moves(state, side);
    let mut best: Option<(u8, u16)> = None;
    for &a in legal.as_slice() {
        if a > 3 { continue; } // moves only: greedy never switches or teras
        let move_id = moves[a as usize];
        if move_id == 0 { continue; }
        let dmg = calc_damage(state, side, move_id, 100, &mut median_roll).damage;
        if best.map_or(true, |(_, d)| dmg > d) { best = Some((a, dmg)); }
    }
    best.map(|(a, _)| a).unwrap_or(legal.actions[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn random_picks_legal() {
        let (s, _t) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        let mut rng = Lcg::new(3);
        let legal = legal_actions(&s, 0);
        for _ in 0..50 {
            let a = random_action(&s, 0, &mut rng);
            assert!(legal.as_slice().contains(&a));
        }
    }

    #[test]
    fn greedy_prefers_super_effective() {
        // Pikachu vs Gyarados: Thunderbolt (4x) over Splash; Garchomp target: Ice Beam check
        let (s, _t) = duel(mon(25, 9, [150, 85, 0, 0]), mon(130, 22, [57, 0, 0, 0]));
        let mut rng = Lcg::new(3);
        assert_eq!(greedy_action(&s, 0, &mut rng), 1, "Thunderbolt slot, not Splash");
    }

    #[test]
    fn greedy_handles_forced_switch() {
        let (s, _t) = build_state(
            vec![mon(25, 9, [85, 0, 0, 0]), mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 0, 0, 0])],
        );
        let mut forced = s;
        forced.sides[0].team[0].current_hp = 0;
        forced.phase = PHASE_SWITCH_P1;
        let mut rng = Lcg::new(3);
        let a = greedy_action(&forced, 0, &mut rng);
        assert!(legal_actions(&forced, 0).as_slice().contains(&a));
    }
}
