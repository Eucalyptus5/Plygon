use crate::belief::{Belief, MonBelief, ScreenMask};
use crate::determinize::{install, sample_set, sample_unrevealed_species};
use crate::rng::Lcg;
use pkmn_engine::state::*;

pub fn gen_team(rng: &mut Lcg) -> [MonBuildInput; 6] {
    let screen = ScreenMask::default();
    let mut taken: Vec<u16> = Vec::with_capacity(6);
    std::array::from_fn(|_| {
        let sp = loop {
            let sp = sample_unrevealed_species(&taken, screen, rng);
            let base = data_bridge::base_species(sp);
            if !taken.iter().any(|&t| data_bridge::base_species(t) == base) {
                break sp;
            }
        };
        taken.push(sp);
        sample_set(sp, &MonBelief { species_id: sp, ..Default::default() }, screen, rng)
    })
}

pub fn initial_state(a: &[MonBuildInput; 6], b: &[MonBuildInput; 6]) -> (BattleState, TeamData) {
    let mut state = BattleState::default();
    let mut teams = TeamData::default();
    for (side, team) in [a, b].into_iter().enumerate() {
        for (slot, input) in team.iter().enumerate() {
            install(&mut state, &mut teams, side, slot, input, None, None);
        }
    }
    state.phase = PHASE_ACTIONS;
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);
    (state, teams)
}

pub fn mask_to_skeleton(state: &BattleState, decider: usize, belief: &Belief) -> BattleState {
    let opp = 1 - decider;
    let mut out = *state;
    for slot in 0..6 {
        let src = &state.sides[opp].team[slot];
        let base = data_bridge::base_species(src.species_id);
        let known = belief.mons.iter().find(|mb| mb.species_id != 0 && data_bridge::base_species(mb.species_id) == base);
        let mut t = MonSlot::default();
        if let (true, Some(mb)) = (src.species_id != 0, known) {
            t.species_id = src.species_id;
            t.level = src.level;
            t.status = src.status;
            t.status_counter = src.status_counter;
            t.max_hp = 100;
            if src.current_hp != 0 {
                let pct = (src.current_hp as u32 * 100).div_ceil(src.max_hp.max(1) as u32).clamp(1, 100);
                t.current_hp = if pct == 100 && src.current_hp < src.max_hp { 99 } else { pct as u16 };
            }
            if src.is_terastallized() {
                t.flags |= MON_FLAG_TERASTALLIZED;
                t.tera_type = src.tera_type;
            }
            t.moves = mb.moves;
            for i in 0..4 {
                if mb.moves[i] != 0 {
                    t.pp[i] = (move_base_pp(mb.moves[i]) as u16 * 8 / 5) as u8;
                }
            }
            t.item_id = mb.item_id;
            t.ability_id = mb.ability_id;
        }
        out.sides[opp].team[slot] = t;
    }
    out
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct NativeSnapshot {
    pub game: u64,
    pub turn: u16,
    pub side: u8,
    pub state: BattleState,
    pub teams: TeamData,
    pub beliefs: [Belief; 2],
    pub pick: u8,
    pub seed: u64,
}

fn bincode_err(e: bincode::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}

pub fn write_all(path: &str, snaps: &[NativeSnapshot]) -> std::io::Result<()> {
    let bytes = bincode::serialize(snaps).map_err(bincode_err)?;
    std::fs::write(path, bytes)
}

pub fn read_all(path: &str) -> std::io::Result<Vec<NativeSnapshot>> {
    let bytes = std::fs::read(path)?;
    bincode::deserialize(&bytes).map_err(bincode_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_teams(seed: u64) -> ([MonBuildInput; 6], [MonBuildInput; 6]) {
        let mut rng = Lcg::new(seed);
        let a = gen_team(&mut rng);
        let b = gen_team(&mut rng);
        (a, b)
    }

    #[test]
    fn gen_team_draws_six_distinct_base_species() {
        let mut rng = Lcg::new(1);
        let team = gen_team(&mut rng);
        let mut bases: Vec<u16> = Vec::new();
        for m in &team {
            assert_ne!(m.species_id, 0);
            assert!(m.level > 0);
            assert!(m.moves.iter().any(|&mv| mv != 0), "species {} has no moves", m.species_id);
            bases.push(data_bridge::base_species(m.species_id));
        }
        bases.sort_unstable();
        bases.dedup();
        assert_eq!(bases.len(), 6, "six distinct base species");
    }

    #[test]
    fn initial_state_installs_both_teams_and_is_playable() {
        let (a, b) = two_teams(2);
        let (state, teams) = initial_state(&a, &b);
        for (s, team) in [&a, &b].into_iter().enumerate() {
            for i in 0..6 {
                let m = &state.sides[s].team[i];
                assert_ne!(m.species_id, 0);
                assert!(m.max_hp > 0);
                assert_eq!(m.current_hp, m.max_hp);
                assert_eq!(teams.levels[s][i], team[i].level);
            }
        }
        assert_eq!(state.phase, PHASE_ACTIONS);
        assert!(legal_actions(&state, 0).count >= 1);
        assert!(legal_actions(&state, 1).count >= 1);
        let v = crate::selfplay::play_to_terminal(state, &teams, [Belief::default(); 2], 7, |side, st, _, _, _, rng| {
            crate::policies::random_action(st, side, rng)
        });
        assert!(v == 0.0 || v == 0.5 || v == 1.0, "terminal value {v}");
    }

    #[test]
    fn mask_to_skeleton_zeroes_unrevealed_and_rescales_revealed() {
        let (a, b) = two_teams(3);
        let (mut state, _teams) = initial_state(&a, &b);
        let opp0 = state.sides[1].team[0];
        assert!(opp0.max_hp > 100 && opp0.moves[1] != 0, "the fixture needs a normal 4-move set");
        state.sides[1].team[0].current_hp = opp0.max_hp / 2;
        let mut belief = Belief::default();
        let k = belief.note_species(opp0.species_id, opp0.level);
        belief.note_move(k, opp0.moves[0]);
        belief.note_move(k, opp0.moves[1]);
        belief.mons[k].item_id = 5;

        let skel = mask_to_skeleton(&state, 0, &belief);
        let t = skel.sides[1].team[0];
        assert_eq!(t.max_hp, 100);
        assert_eq!(t.current_hp, 50);
        assert_eq!(t.moves, belief.mons[k].moves);
        assert_eq!(t.pp[0], opp0.pp[0]);
        assert_eq!(t.pp[1], opp0.pp[1]);
        assert_eq!(t.pp[2], 0);
        assert_eq!(t.pp[3], 0);
        assert_eq!(t.item_id, 5);
        assert_eq!(t.ability_id, 0);
        assert_eq!(t.stats, [0; 5]);
        assert_eq!(t.species_id, opp0.species_id);
        assert_eq!(t.level, opp0.level);
        assert_eq!(t.status, opp0.status);
        assert_eq!(t.status_counter, opp0.status_counter);
        for slot in 1..6 {
            assert!(skel.sides[1].team[slot] == MonSlot::default(), "unrevealed slot {slot} must be zero");
        }
        assert!(skel.sides[0] == state.sides[0], "the decider's side is untouched");
        assert!(skel.field == state.field);
        assert!(skel.sides[1].active == state.sides[1].active);
        assert_eq!(skel.sides[1].active_index, state.sides[1].active_index);

        state.sides[1].team[0].current_hp = 0;
        assert_eq!(mask_to_skeleton(&state, 0, &belief).sides[1].team[0].current_hp, 0);
        state.sides[1].team[0].current_hp = 1;
        assert_eq!(mask_to_skeleton(&state, 0, &belief).sides[1].team[0].current_hp, 1);
        state.sides[1].team[0].current_hp = opp0.max_hp - 1;
        assert_eq!(mask_to_skeleton(&state, 0, &belief).sides[1].team[0].current_hp, 99);
    }

    #[test]
    fn snapshot_round_trip() {
        let (a, b) = two_teams(4);
        let (state, teams) = initial_state(&a, &b);
        let mut belief = Belief::default();
        belief.note_species(state.sides[1].team[0].species_id, state.sides[1].team[0].level);
        let snaps = vec![
            NativeSnapshot { game: 1, turn: 2, side: 0, state, teams: teams.clone(), beliefs: [belief, Belief::default()], pick: 3, seed: 9 },
            NativeSnapshot { game: 5, turn: 6, side: 1, state, teams, beliefs: [Belief::default(), belief], pick: 7, seed: 11 },
        ];
        let path = std::env::temp_dir().join(format!("frontier_snapshots_{}.bin", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        write_all(&path, &snaps).unwrap();
        let back = read_all(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.len(), snaps.len());
        for (x, y) in snaps.iter().zip(&back) {
            assert_eq!(x.game, y.game);
            assert_eq!(x.turn, y.turn);
            assert_eq!(x.side, y.side);
            assert!(x.state == y.state);
            assert_eq!(x.teams, y.teams);
            assert_eq!(x.beliefs, y.beliefs);
            assert_eq!(x.pick, y.pick);
            assert_eq!(x.seed, y.seed);
        }
    }
}
