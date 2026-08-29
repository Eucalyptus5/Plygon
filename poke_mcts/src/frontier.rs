use crate::belief::{possible, species_sets_with_base_fallback, Belief, MonBelief, ScreenMask};
use crate::determinize::{install, sample_set, sample_unrevealed_species};
use crate::eval::{winner_value, Evaluator};
use crate::rng::{splitmix64, Lcg};
use pkmn_engine::state::*;
use std::cell::Cell;

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
        let known = known_slot(belief, src.species_id);
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Move,
    Tera,
    Switch,
    SwitchUnseen,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Move => "move",
            Kind::Tera => "tera",
            Kind::Switch => "switch",
            Kind::SwitchUnseen => "switch-unseen",
        }
    }
}

fn known_slot(belief: &Belief, species_id: u16) -> Option<&MonBelief> {
    let base = data_bridge::base_species(species_id);
    belief.mons.iter().find(|mb| mb.species_id != 0 && data_bridge::base_species(mb.species_id) == base)
}

pub fn label_space(state: &BattleState, decider: usize, belief: &Belief) -> (Vec<(Kind, u16)>, Vec<f64>) {
    let opp = 1 - decider;
    let switch_only = match state.phase {
        PHASE_SWITCH_BOTH => true,
        PHASE_SWITCH_P1 => opp == 0,
        PHASE_SWITCH_P2 => opp == 1,
        _ => false,
    };
    let mut sets: Vec<([u16; 4], u32)> = Vec::new();
    if !switch_only {
        let sp = state.active_mon(opp).species_id;
        if let (Some(mb), Some(ss)) = (known_slot(belief, sp), species_sets_with_base_fallback(sp)) {
            for (i, t) in ss.sets.iter().enumerate() {
                if possible(i, t, mb) {
                    sets.push((t.moves, t.count));
                }
            }
        }
    }
    let mut moves: Vec<u16> = sets.iter().flat_map(|(m, _)| m.iter().copied()).filter(|&m| m != 0).collect();
    moves.sort_unstable();
    moves.dedup();
    // bit 0 of _padding[0] is the side's battle-lifetime tera-used flag, the bit can_tera reads
    let tera = !switch_only && state.sides[opp]._padding[0] & 1 == 0;
    let mut labels: Vec<(Kind, u16)> = moves.iter().map(|&m| (Kind::Move, m)).collect();
    if tera {
        labels.extend(moves.iter().map(|&m| (Kind::Tera, m)));
    }
    let active = state.sides[opp].active_index as usize;
    let mut revealed = 0u32;
    let mut unseen_alive = 0u32;
    for k in 0..6 {
        let m = &state.sides[opp].team[k];
        if k == active || m.species_id == 0 || m.current_hp == 0 {
            continue;
        }
        if known_slot(belief, m.species_id).is_some() {
            labels.push((Kind::Switch, m.species_id));
            revealed += 1;
        } else {
            unseen_alive += 1;
        }
    }
    if unseen_alive > 0 {
        labels.push((Kind::SwitchUnseen, 0));
    }
    if sets.is_empty() {
        sets.push(([0; 4], 1));
    }
    let total_count: f64 = sets.iter().map(|(_, c)| *c as f64).sum();
    let mut prior = vec![0.0f64; labels.len()];
    let index = |kind: Kind, id: u16| labels.iter().position(|&e| e == (kind, id)).unwrap();
    for (set_moves, count) in &sets {
        let mut mv: Vec<u16> = set_moves.iter().copied().filter(|&m| m != 0).collect();
        mv.sort_unstable();
        mv.dedup();
        let n_mv = mv.len() as u32;
        let total_slots = n_mv + if tera { n_mv } else { 0 } + revealed + unseen_alive;
        if total_slots == 0 {
            continue;
        }
        let w = *count as f64 / total_count / total_slots as f64;
        for &m in &mv {
            prior[index(Kind::Move, m)] += w;
            if tera {
                prior[index(Kind::Tera, m)] += w;
            }
        }
        for (i, (kind, _)) in labels.iter().enumerate() {
            match kind {
                Kind::Switch => prior[i] += w,
                Kind::SwitchUnseen => prior[i] += w * unseen_alive as f64,
                _ => {}
            }
        }
    }
    (labels, prior)
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

pub fn bincode_err(e: bincode::Error) -> std::io::Error {
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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Row {
    pub game_index: u32,
    pub game_tag: u64,
    pub gen_eval: u8, // 0 = handcrafted-driven game, 1 = net-driven game
    pub turn: u16,
    pub side: u8,
    pub z: f32,
    pub state: BattleState,
    pub teams: TeamData,
}

pub fn read_rows(paths: &[String]) -> std::io::Result<Vec<Row>> {
    use std::io::Read;
    let mut rows: Vec<Row> = Vec::new();
    for path in paths {
        let mut magic = [0u8; 2];
        std::fs::File::open(path)?.read_exact(&mut magic)?;
        let bytes = if magic == [0x1f, 0x8b] {
            let out = std::process::Command::new("gzip").arg("-dc").arg(path).output()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("gzip -dc {path}: {e}")))?;
            if !out.status.success() {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("gzip -dc {path}: {}", out.status)));
            }
            out.stdout
        } else {
            std::fs::read(path)?
        };
        let part: Vec<Row> = bincode::deserialize(&bytes).map_err(bincode_err)?;
        rows.extend(part);
    }
    Ok(rows)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayoutPolicy {
    Uniform,
    Greedy,
}

impl PlayoutPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayoutPolicy::Uniform => "uniform",
            PlayoutPolicy::Greedy => "greedy",
        }
    }

    fn choose(self, state: &BattleState, side: usize, rng: &mut Lcg) -> u8 {
        match self {
            PlayoutPolicy::Uniform => crate::policies::random_action(state, side, rng),
            PlayoutPolicy::Greedy => crate::policies::greedy_action(state, side, rng),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Playout {
    pub value: f64,
    pub steps: u32,
    pub turn: u16,
    pub capped: bool,
}

pub const PLAYOUT_STEP_CAP: u32 = 500;
const PLAYOUT_NEXT_SALT: u64 = 0x7F4A_7C15_9E37_79B9;

pub struct PlayoutEval {
    pub teams: TeamData,
    pub k: u32,
    pub seed: Cell<u64>,
    pub policy: PlayoutPolicy,
}

impl PlayoutEval {
    pub fn new(teams: TeamData, k: u32, seed: u64, policy: PlayoutPolicy) -> Self {
        PlayoutEval { teams, k, seed: Cell::new(seed), policy }
    }

    // game i of an eval call at seed s runs at this seed whatever k is, so a k = 8 value is
    // the mean of the first 8 games of the k = 32 value at the same seed
    pub fn game_seed(seed: u64, i: u32) -> u64 {
        splitmix64(seed ^ i as u64)
    }

    pub fn playout(&self, state: &BattleState, game_seed: u64) -> Playout {
        let mut state = *state;
        let mut battle_rng = Lcg::new(splitmix64(game_seed));
        let mut pol_rng = Lcg::new(splitmix64(game_seed ^ 0xA5A5));
        let mut steps = 0u32;
        while !state.is_game_over() {
            if steps == PLAYOUT_STEP_CAP {
                return Playout { value: winner_value(&state), steps, turn: state.field.turn, capped: true };
            }
            let act = |side: usize, pol_rng: &mut Lcg| {
                if legal_actions(&state, side).count > 0 { self.policy.choose(&state, side, pol_rng) } else { ACTION_STRUGGLE }
            };
            let a1 = act(0, &mut pol_rng);
            let a2 = act(1, &mut pol_rng);
            match state.phase {
                PHASE_ACTIONS => execute_turn(&mut state, &self.teams, a1, a2, &mut |m| battle_rng.roll(m)),
                PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                    execute_switch_turn(&mut state, &self.teams, a1, a2, &mut |m| battle_rng.roll(m))
                }
                _ => break,
            }
            steps += 1;
        }
        Playout { value: winner_value(&state), steps, turn: state.field.turn, capped: false }
    }
}

impl Evaluator for PlayoutEval {
    fn eval(&self, state: &BattleState) -> f32 {
        let seed = self.seed.get();
        self.seed.set(splitmix64(seed ^ PLAYOUT_NEXT_SALT));
        let mut sum = 0.0f64;
        for i in 0..self.k {
            sum += self.playout(state, Self::game_seed(seed, i)).value;
        }
        (sum / self.k as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_sets::{SetEntry, GEN9_SET_POOL};

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

    fn opp_fixture(seed: u64) -> (BattleState, Belief) {
        let (a, b) = two_teams(seed);
        let (state, _teams) = initial_state(&a, &b);
        let mut belief = Belief::default();
        let om = state.active_mon(1);
        belief.note_species(om.species_id, om.level);
        (state, belief)
    }

    fn all_moves<'a>(sets: impl Iterator<Item = &'a SetEntry>) -> Vec<u16> {
        let mut out: Vec<u16> = sets.flat_map(|t| t.moves.iter().copied()).filter(|&m| m != 0).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn of_kind(l: &[(Kind, u16)], kind: Kind) -> Vec<u16> {
        l.iter().filter(|(k, _)| *k == kind).map(|&(_, id)| id).collect()
    }

    fn shrinking_species() -> (u16, u16) {
        for ss in GEN9_SET_POOL {
            let full = all_moves(ss.sets.iter());
            for t in ss.sets {
                for &m in t.moves.iter().filter(|&&m| m != 0) {
                    let kept = all_moves(ss.sets.iter().filter(|s| s.moves.contains(&m)));
                    if kept.len() < full.len() {
                        return (ss.species_id, m);
                    }
                }
            }
        }
        panic!("no species whose revealed move shrinks the pooled move union");
    }

    #[test]
    fn label_space_move_part_is_the_union_of_surviving_sets() {
        let (mut state, _) = opp_fixture(5);
        let (sp, m) = shrinking_species();
        let ai = state.sides[1].active_index as usize;
        state.sides[1].team[ai].species_id = sp;
        let mut belief = Belief::default();
        let k = belief.note_species(sp, 80);
        let ss = species_sets_with_base_fallback(sp).unwrap();
        let (l, _) = label_space(&state, 0, &belief);
        let moves = of_kind(&l, Kind::Move);
        assert_eq!(moves, all_moves(ss.sets.iter()));
        belief.note_move(k, m);
        let (l2, _) = label_space(&state, 0, &belief);
        let moves2 = of_kind(&l2, Kind::Move);
        assert_eq!(moves2, all_moves(ss.sets.iter().filter(|t| t.moves.contains(&m))));
        assert!(moves2.len() < moves.len());
        assert!(moves2.contains(&m));
        assert!(moves2.windows(2).all(|w| w[0] < w[1]), "ascending move ids");
    }

    #[test]
    fn label_space_tera_entries_are_suppressed_by_the_sides_tera_used_bit() {
        let (mut state, belief) = opp_fixture(6);
        let (l, _) = label_space(&state, 0, &belief);
        let moves = of_kind(&l, Kind::Move);
        assert!(!moves.is_empty());
        assert_eq!(of_kind(&l, Kind::Tera), moves);
        state.sides[1]._padding[0] |= 1;
        let (l2, _) = label_space(&state, 0, &belief);
        assert!(of_kind(&l2, Kind::Tera).is_empty());
        assert_eq!(of_kind(&l2, Kind::Move), moves);
    }

    #[test]
    fn label_space_switch_entries_follow_revealed_alive_bench_and_the_unseen_pool() {
        let (mut state, mut belief) = opp_fixture(7);
        let ai = state.sides[1].active_index as usize;
        let bench: Vec<usize> = (0..6).filter(|&k| k != ai).collect();
        let (l, _) = label_space(&state, 0, &belief);
        assert!(of_kind(&l, Kind::Switch).is_empty());
        assert!(l.contains(&(Kind::SwitchUnseen, 0)));

        let alive = state.sides[1].team[bench[0]];
        let fainted = state.sides[1].team[bench[1]];
        belief.note_species(alive.species_id, alive.level);
        belief.note_species(fainted.species_id, fainted.level);
        state.sides[1].team[bench[1]].current_hp = 0;
        let (l, _) = label_space(&state, 0, &belief);
        assert_eq!(of_kind(&l, Kind::Switch), vec![alive.species_id]);
        assert!(!l.contains(&(Kind::Switch, fainted.species_id)));
        assert!(l.contains(&(Kind::SwitchUnseen, 0)));

        for &k in &bench[2..4] {
            let m = state.sides[1].team[k];
            belief.note_species(m.species_id, m.level);
        }
        state.sides[1].team[bench[4]].current_hp = 0;
        let (l, _) = label_space(&state, 0, &belief);
        assert!(!l.contains(&(Kind::SwitchUnseen, 0)));
        let expect: Vec<u16> = [bench[0], bench[2], bench[3]].iter().map(|&k| state.sides[1].team[k].species_id).collect();
        assert_eq!(of_kind(&l, Kind::Switch), expect);
        let rank = |k: Kind| match k {
            Kind::Move => 0,
            Kind::Tera => 1,
            Kind::Switch => 2,
            Kind::SwitchUnseen => 3,
        };
        assert!(l.windows(2).all(|w| rank(w[0].0) <= rank(w[1].0)), "moves, then tera, then switches");
    }

    #[test]
    fn label_space_in_a_switch_only_phase_has_only_switch_entries() {
        let (mut state, mut belief) = opp_fixture(8);
        let ai = state.sides[1].active_index as usize;
        let k = (0..6).find(|&k| k != ai).unwrap();
        let m = state.sides[1].team[k];
        belief.note_species(m.species_id, m.level);
        let only_switches = |l: &[(Kind, u16)]| l.iter().all(|(k, _)| matches!(k, Kind::Switch | Kind::SwitchUnseen));
        for phase in [PHASE_SWITCH_P2, PHASE_SWITCH_BOTH] {
            state.phase = phase;
            let (l, p) = label_space(&state, 0, &belief);
            assert!(only_switches(&l), "phase {phase}: {l:?}");
            assert_eq!(l, vec![(Kind::Switch, m.species_id), (Kind::SwitchUnseen, 0)]);
            assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        }
        state.phase = PHASE_SWITCH_P1;
        let (l, _) = label_space(&state, 0, &belief);
        assert!(!of_kind(&l, Kind::Move).is_empty(), "our own forced switch does not restrict the opponent");
        let mut b1 = Belief::default();
        let om = state.active_mon(0);
        b1.note_species(om.species_id, om.level);
        let (l, _) = label_space(&state, 1, &b1);
        assert!(only_switches(&l));
        assert_eq!(l, vec![(Kind::SwitchUnseen, 0)]);
    }

    #[test]
    fn prior_sums_to_one_and_covers_every_entry() {
        for seed in 20..40u64 {
            let (mut state, mut belief) = opp_fixture(seed);
            let ai = state.sides[1].active_index as usize;
            let k = (0..6).find(|&k| k != ai).unwrap();
            let m = state.sides[1].team[k];
            for reveal in [false, true] {
                if reveal {
                    belief.note_species(m.species_id, m.level);
                    let om = *state.active_mon(1);
                    belief.note_move(0, om.moves[0]);
                    state.sides[1]._padding[0] |= (seed & 1) as u8;
                }
                for phase in [PHASE_ACTIONS, PHASE_SWITCH_P2, PHASE_SWITCH_BOTH] {
                    state.phase = phase;
                    let (l, p) = label_space(&state, 0, &belief);
                    assert_eq!(l.len(), p.len());
                    assert!(!l.is_empty());
                    let sum: f64 = p.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-9, "seed {seed} phase {phase} reveal {reveal}: sum {sum}");
                    assert!(p.iter().all(|&x| x > 0.0), "seed {seed} phase {phase} reveal {reveal}: {l:?} {p:?}");
                }
            }
        }
    }

    #[test]
    fn prior_favours_a_move_every_surviving_set_carries() {
        let (sp, universal, singleton) = GEN9_SET_POOL
            .iter()
            .filter(|ss| ss.sets.len() >= 2)
            .find_map(|ss| {
                let full = all_moves(ss.sets.iter());
                let u = full.iter().copied().find(|m| ss.sets.iter().all(|t| t.moves.contains(m)))?;
                let s = full.iter().copied().find(|m| ss.sets.iter().filter(|t| t.moves.contains(m)).count() == 1)?;
                Some((ss.species_id, u, s))
            })
            .unwrap();
        let (mut state, _) = opp_fixture(9);
        let ai = state.sides[1].active_index as usize;
        state.sides[1].team[ai].species_id = sp;
        let mut belief = Belief::default();
        belief.note_species(sp, 80);
        let (l, p) = label_space(&state, 0, &belief);
        let at = |kind: Kind, id: u16| p[l.iter().position(|&e| e == (kind, id)).unwrap()];
        assert!(at(Kind::Move, universal) > at(Kind::Move, singleton));
        assert!(at(Kind::Tera, universal) > at(Kind::Tera, singleton));
        assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn read_rows_concatenates_plain_and_gzipped_files_in_order() {
        let mut rng = Lcg::new(5);
        let (a, b) = (gen_team(&mut rng), gen_team(&mut rng));
        let (state, teams) = initial_state(&a, &b);
        let row = |i: u32| Row { game_index: i, game_tag: 7 * i as u64, gen_eval: (i % 2) as u8, turn: 1, side: (i % 2) as u8, z: 0.25 * i as f32, state, teams: teams.clone() };
        let dir = std::env::temp_dir().join(format!("frontier_rows_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("rows.0.bin");
        let packed = dir.join("rows.1.bin");
        std::fs::write(&plain, bincode::serialize(&vec![row(0), row(1)]).unwrap()).unwrap();
        std::fs::write(&packed, bincode::serialize(&vec![row(2)]).unwrap()).unwrap();
        assert!(std::process::Command::new("gzip").arg("-f").arg(&packed).status().unwrap().success());
        let paths = [plain.to_str().unwrap().to_string(), format!("{}.gz", packed.to_str().unwrap())];
        let rows = read_rows(&paths).unwrap();
        assert_eq!(rows.iter().map(|r| r.game_index).collect::<Vec<u32>>(), vec![0, 1, 2]);
        assert_eq!(rows[2].game_tag, 14);
        assert_eq!(rows[1].side, 1);
        assert_eq!(rows[1].gen_eval, 1);
        assert_eq!(rows[2].z, 0.5);
        assert_eq!(rows[0].state.sides[0].team[0].species_id, state.sides[0].team[0].species_id);
        assert_eq!(rows[2].teams.levels, teams.levels);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod playout_tests {
    use super::*;

    fn fixture(seed: u64) -> (BattleState, TeamData) {
        let mut rng = Lcg::new(seed);
        let (a, b) = (gen_team(&mut rng), gen_team(&mut rng));
        initial_state(&a, &b)
    }

    #[test]
    fn playout_reaches_a_terminal_and_is_seed_determined() {
        let (state, teams) = fixture(11);
        for policy in [PlayoutPolicy::Uniform, PlayoutPolicy::Greedy] {
            let pe = PlayoutEval::new(teams.clone(), 1, 5, policy);
            let p = pe.playout(&state, 99);
            assert!(!p.capped, "{policy:?}");
            assert!(p.value == 0.0 || p.value == 0.5 || p.value == 1.0, "{policy:?}: {p:?}");
            assert!(p.steps >= 1 && p.turn >= 1, "{policy:?}: {p:?}");
            assert_eq!(pe.playout(&state, 99), p, "{policy:?}");
        }
    }

    #[test]
    fn eval_is_the_mean_of_k_games_and_shares_its_first_games_across_k() {
        let (state, teams) = fixture(12);
        let e8 = PlayoutEval::new(teams.clone(), 8, 3, PlayoutPolicy::Uniform);
        let e32 = PlayoutEval::new(teams, 32, 3, PlayoutPolicy::Uniform);
        let games: Vec<f64> = (0..32).map(|i| e32.playout(&state, PlayoutEval::game_seed(3, i)).value).collect();
        let mean = |g: &[f64]| (g.iter().sum::<f64>() / g.len() as f64) as f32;
        assert_eq!(e8.eval(&state), mean(&games[..8]));
        assert_eq!(e32.eval(&state), mean(&games));
        assert!(games.iter().any(|&v| v != games[0]), "32 uniform games from the opening never all agree");
        assert_ne!(e8.seed.get(), 3, "the seed advances after an eval");
        assert_eq!(e8.seed.get(), e32.seed.get());
    }

    #[test]
    fn a_terminal_state_evaluates_to_its_winner_without_a_step() {
        let (mut state, teams) = fixture(13);
        for i in 0..6 {
            state.sides[1].team[i].current_hp = 0;
        }
        state.phase = PHASE_GAME_OVER;
        let pe = PlayoutEval::new(teams, 4, 1, PlayoutPolicy::Greedy);
        let p = pe.playout(&state, 0);
        assert_eq!((p.value, p.steps, p.capped), (1.0, 0, false));
        assert_eq!(pe.eval(&state), 1.0);
    }
}
