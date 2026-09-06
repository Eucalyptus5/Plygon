use pkmn_engine::state::*;

pub fn mon(species_id: u16, ability_id: u16, moves: [u16; 4]) -> MonBuildInput {
    MonBuildInput {
        species_id, ability_id, item_id: 0, moves,
        ivs: [31; 6], evs: [85; 6], nature: 0, level: 80,
        tera_type: 0, is_female: false,
    }
}

pub fn empty_mon() -> MonBuildInput { mon(0, 0, [0; 4]) }

pub fn build_state(p1: Vec<MonBuildInput>, p2: Vec<MonBuildInput>) -> (BattleState, TeamData) {
    let fill = |v: Vec<MonBuildInput>| -> [MonBuildInput; 6] {
        let mut a: [MonBuildInput; 6] = std::array::from_fn(|_| empty_mon());
        for (i, m) in v.into_iter().enumerate().take(6) { a[i] = m; }
        a
    };
    let (t1, b1, l1) = build_team(&fill(p1));
    let (t2, b2, l2) = build_team(&fill(p2));
    let teams = TeamData { mons: [b1, b2], levels: [l1, l2] };
    let mut state = BattleState::default();
    state.sides[0].team = t1;
    state.sides[1].team = t2;
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    (state, teams)
}

pub fn duel(m1: MonBuildInput, m2: MonBuildInput) -> (BattleState, TeamData) {
    build_state(vec![m1], vec![m2])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duel_builds_and_steps() {
        let (mut s, teams) = duel(mon(25, 9, [85, 150, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        assert_eq!(s.phase, PHASE_ACTIONS);
        assert!(legal_actions(&s, 0).count >= 1);
        let mut rng = |_m: u32| 0u32;
        execute_turn(&mut s, &teams, 0, 0, &mut rng);
        assert!(s.field.turn >= 1 || s.is_game_over());
    }
}
