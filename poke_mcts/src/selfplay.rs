use crate::belief::{engine_type_to_showdown, seed_pool, Belief};
use crate::determinize::{Determinizer, Observation, RandomBattle};
use crate::driver::{choose_action, PickMode, PimcConfig};
use crate::eval::winner_value;
use crate::policies::{greedy_action, random_action};
use crate::rng::{splitmix64, Lcg};
use crate::search::ChanceMode;
use pkmn_engine::state::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    Mcts,
    Greedy,
    Random,
}

const JUDGE_NUM_WORLDS: usize = 8;
const JUDGE_MAX_ITERS_PER_WORLD: u64 = 1000;
const JUDGE_TIME_MS_PER_WORLD: u64 = 60_000;
const JUDGE_FILTER_THRESHOLD: f64 = 0.75;

#[derive(Clone, Copy, Debug)]
pub struct JudgeBudget {
    pub num_worlds: usize,
    pub max_iters_per_world: u64,
    pub time_ms_per_world: u64,
}

impl Default for JudgeBudget {
    fn default() -> Self {
        JudgeBudget {
            num_worlds: JUDGE_NUM_WORLDS,
            max_iters_per_world: JUDGE_MAX_ITERS_PER_WORLD,
            time_ms_per_world: JUDGE_TIME_MS_PER_WORLD,
        }
    }
}

// Opponent's belief about us: only our active + fainted mons are unambiguously revealed
// (engine has no per-mon revealed flag); living bench slots are left unknown.
pub fn reconstruct_opp_belief(state: &BattleState, our_side: usize) -> Belief {
    let mut b = Belief::default();
    let active = state.active_mon(our_side);
    if active.species_id != 0 {
        b.note_species(active.species_id, active.level);
    }
    let active_idx = state.sides[our_side].active_index as usize;
    for i in 0..6 {
        if i == active_idx {
            continue;
        }
        let m = &state.sides[our_side].team[i];
        if m.species_id != 0 && m.current_hp == 0 {
            b.note_species(m.species_id, m.level);
        }
    }
    b
}

fn decide(
    policy: Policy,
    state: &BattleState,
    teams: &TeamData,
    side: usize,
    beliefs: &[Belief; 2],
    seed: u64,
    budget: JudgeBudget,
    rng: &mut Lcg,
) -> u8 {
    match policy {
        Policy::Random => random_action(state, side, rng),
        Policy::Greedy => greedy_action(state, side, rng),
        Policy::Mcts => {
            let obs = Observation { state, teams, our_side: side };
            let cfg = PimcConfig {
                num_worlds: budget.num_worlds,
                time_ms_per_world: budget.time_ms_per_world,
                max_iters_per_world: budget.max_iters_per_world,
                seed,
                chance_mode: ChanceMode::OpenLoop,
                pick_mode: PickMode::Weighted,
                filter_threshold: JUDGE_FILTER_THRESHOLD,
                raw_root: false,
                explore_coeff: 2.0,
                value_temp: 1.0,
                blind_opponent: false,
            };
            choose_action(&obs, &beliefs[side], &RandomBattle, &cfg)
        }
    }
}

#[derive(Clone, Copy)]
struct PpSnapshot {
    slot: usize,
    flags: u32,
    pp: [u8; 4],
}

fn pp_snapshot(state: &BattleState) -> [PpSnapshot; 2] {
    std::array::from_fn(|s| {
        let slot = state.sides[s].active_index as usize;
        PpSnapshot { slot, flags: state.sides[s].active.volatile_flags, pp: state.sides[s].team[slot].pp }
    })
}

// The engine spends PP on a locked or charging continuation turn; Showdown shows no move there.
fn note_pp_uses(beliefs: &mut [Belief; 2], state: &BattleState, pre: &[PpSnapshot; 2], slots: [usize; 2]) {
    for s in 0..2 {
        let p = pre[s];
        if p.flags & (VOL_MOVE_LOCKED | VOL_CHARGING) != 0 {
            continue;
        }
        let m = &state.sides[s].team[p.slot];
        for i in 0..4 {
            let d = p.pp[i].saturating_sub(m.pp[i]);
            if d > 0 {
                beliefs[1 - s].note_move_use(slots[s], m.moves[i], d);
            }
        }
    }
}

// Shared terminal loop lifted from the self-play binary. Both play_from and the bin's play call it.
pub fn play_to_terminal<F>(
    state: BattleState,
    teams: &TeamData,
    beliefs: [Belief; 2],
    game_seed: u64,
    choose: F,
) -> f64
where
    F: FnMut(usize, &BattleState, &TeamData, &[Belief; 2], u64, &mut Lcg) -> u8,
{
    play_to_terminal_timed(state, teams, beliefs, game_seed, choose, None)
}

#[derive(Clone, Copy, Default, Debug)]
pub struct LoopTimes {
    pub engine: std::time::Duration,
    pub belief: std::time::Duration,
    pub steps: u32,
    pub turns: u16,
}

fn timed<T>(bucket: Option<&mut std::time::Duration>, f: impl FnOnce() -> T) -> T {
    match bucket {
        Some(b) => {
            let t = std::time::Instant::now();
            let r = f();
            *b += t.elapsed();
            r
        }
        None => f(),
    }
}

// The bridge tracker seeds the candidate pool on the same first sighting; a re-entry keeps its prunes.
fn reveal(b: &mut Belief, species_id: u16, level: u8) -> usize {
    let (slot, fresh) = b.note_species_tracked(species_id, level);
    if fresh {
        seed_pool(&mut b.mons[slot], species_id);
    }
    slot
}

pub fn play_to_terminal_timed<F>(
    mut state: BattleState,
    teams: &TeamData,
    mut beliefs: [Belief; 2],
    game_seed: u64,
    mut choose: F,
    mut times: Option<&mut LoopTimes>,
) -> f64
where
    F: FnMut(usize, &BattleState, &TeamData, &[Belief; 2], u64, &mut Lcg) -> u8,
{
    let note_active = |beliefs: &mut [Belief; 2], state: &BattleState| {
        for s in 0..2 {
            let om = state.active_mon(1 - s);
            if om.species_id != 0 && om.current_hp > 0 {
                reveal(&mut beliefs[s], om.species_id, om.level);
            }
        }
    };
    timed(times.as_deref_mut().map(|t| &mut t.belief), || note_active(&mut beliefs, &state));

    let mut battle_rng = Lcg::new(splitmix64(game_seed));
    let mut pol_rng = Lcg::new(splitmix64(game_seed ^ 0xA5A5));
    for turn in 0..500u64 {
        if state.is_game_over() {
            break;
        }
        let s1 = splitmix64(game_seed ^ (turn << 1));
        let s2 = splitmix64(game_seed ^ (turn << 1) ^ 1);
        let a1 = if legal_actions(&state, 0).count > 0 {
            choose(0, &state, teams, &beliefs, s1, &mut pol_rng)
        } else {
            ACTION_STRUGGLE
        };
        let a2 = if legal_actions(&state, 1).count > 0 {
            choose(1, &state, teams, &beliefs, s2, &mut pol_rng)
        } else {
            ACTION_STRUGGLE
        };
        let mut belief_slots = [0usize; 2];
        timed(times.as_deref_mut().map(|t| &mut t.belief), || {
            for (s, a) in [(0usize, a1), (1usize, a2)] {
                if state.phase != PHASE_ACTIONS {
                    continue;
                }
                let viewer = 1 - s;
                let slot = reveal(&mut beliefs[viewer], state.active_mon(s).species_id, state.active_mon(s).level);
                belief_slots[s] = slot;
                match a {
                    0..=3 => {
                        let mv = effective_moves(&state, s)[a as usize];
                        beliefs[viewer].note_move(slot, mv);
                    }
                    ACTION_TERA_0..=ACTION_TERA_3 => {
                        let mv = effective_moves(&state, s)[(a - ACTION_TERA_0) as usize];
                        beliefs[viewer].note_move(slot, mv);
                        // MonBelief stores Showdown indices; the inverse map handles the Stellar (18) identity.
                        let sd_tera = engine_type_to_showdown(state.active_mon(s).tera_type);
                        beliefs[viewer].note_tera(slot, sd_tera);
                    }
                    _ => {}
                }
            }
        });
        match state.phase {
            PHASE_ACTIONS => {
                let pre = timed(times.as_deref_mut().map(|t| &mut t.belief), || pp_snapshot(&state));
                timed(times.as_deref_mut().map(|t| &mut t.engine), || {
                    execute_turn(&mut state, teams, a1, a2, &mut |m| battle_rng.roll(m))
                });
                timed(times.as_deref_mut().map(|t| &mut t.belief), || {
                    note_pp_uses(&mut beliefs, &state, &pre, belief_slots)
                });
            }
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                timed(times.as_deref_mut().map(|t| &mut t.engine), || {
                    execute_switch_turn(&mut state, teams, a1, a2, &mut |m| battle_rng.roll(m))
                })
            }
            _ => break,
        }
        if let Some(t) = times.as_deref_mut() {
            t.steps += 1;
        }
        timed(times.as_deref_mut().map(|t| &mut t.belief), || note_active(&mut beliefs, &state));
    }
    if let Some(t) = times.as_deref_mut() {
        t.turns = state.field.turn;
    }
    winner_value(&state)
}

// Ground-truth world seed salt, kept distinct from play_to_terminal's battle/policy/per-turn seeds.
const GROUND_TRUTH_WORLD_SALT: u64 = 0xC0DA_15E5_5EED_F00D;

// Materialize ONE concrete battle world from our belief: our real side is preserved verbatim, the
// opponent (and any unrevealed bench) is sampled via the SAME RandomBattle path the live search uses
// (RandomBattle::sample_worlds), honoring the observed public facts — active species, HP-percent ->
// real max-HP, status, revealed moves. Seeded off game_seed so both judge arms share the identical
// sampled opponent per CRN repeat; only the forced first move differs between arms.
pub fn determinize_ground_truth(
    state: &BattleState,
    teams: &TeamData,
    belief: &Belief,
    our_side: usize,
    game_seed: u64,
) -> (BattleState, TeamData) {
    let obs = Observation { state, teams, our_side };
    let mut rng = Lcg::new(splitmix64(game_seed ^ GROUND_TRUTH_WORLD_SALT));
    let mut worlds = RandomBattle.sample_worlds(&obs, belief, 1, &mut rng);
    let w = worlds.pop().expect("sample_worlds(n=1) yields one world");
    (w.state, w.teams)
}

pub fn play_from(
    state: &BattleState,
    teams: &TeamData,
    beliefs: &[Belief; 2],
    our_side: usize,
    forced_first: u8,
    policy: Policy,
    game_seed: u64,
) -> f64 {
    play_from_with_budget(state, teams, beliefs, our_side, forced_first, policy, game_seed, JudgeBudget::default())
}

#[allow(clippy::too_many_arguments)]
pub fn play_from_with_budget(
    state: &BattleState,
    teams: &TeamData,
    beliefs: &[Belief; 2],
    our_side: usize,
    forced_first: u8,
    policy: Policy,
    game_seed: u64,
    budget: JudgeBudget,
) -> f64 {
    let (gt_state, gt_teams) = determinize_ground_truth(state, teams, &beliefs[our_side], our_side, game_seed);
    let mut forced_done = false;
    let v = play_to_terminal(gt_state, &gt_teams, *beliefs, game_seed, move |side, st, tm, bel, seed, rng| {
        if side == our_side && !forced_done && st.phase == PHASE_ACTIONS {
            forced_done = true;
            return forced_first;
        }
        decide(policy, st, tm, side, bel, seed, budget, rng)
    });
    if our_side == 0 { v } else { 1.0 - v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::Belief;
    use crate::determinize::{Determinizer, Observation, RandomBattle};
    use crate::rng::Lcg;
    use crate::testutil::{build_state, mon};
    use pkmn_engine::data::{MOVE_BODY_SLAM, MOVE_FLY, MOVE_OUTRAGE, MOVE_SPLASH, MOVE_U_TURN};
    use pkmn_engine::state::MonSlot;

    // Mirrors play_to_terminal's PHASE_ACTIONS body: reveal the chosen move, step, then book the uses.
    fn step(beliefs: &mut [Belief; 2], state: &mut BattleState, teams: &TeamData, a1: u8, a2: u8) {
        let mut slots = [0usize; 2];
        for (s, a) in [(0usize, a1), (1usize, a2)] {
            let viewer = 1 - s;
            let active = state.active_mon(s);
            slots[s] = beliefs[viewer].note_species(active.species_id, active.level);
            if a <= 3 {
                let mv = effective_moves(state, s)[a as usize];
                beliefs[viewer].note_move(slots[s], mv);
            }
        }
        let pre = pp_snapshot(state);
        execute_turn(state, teams, a1, a2, &mut |_m| 0u32);
        note_pp_uses(beliefs, state, &pre, slots);
    }

    fn duel_vs_splash(attacker: MonBuildInput) -> (BattleState, TeamData) {
        build_state(vec![attacker], vec![mon(143, 0, [MOVE_SPLASH as u16, 0, 0, 0])])
    }

    #[test]
    fn locked_move_continuation_records_one_use() {
        let (mut state, teams) = duel_vs_splash(mon(242, 0, [MOVE_OUTRAGE as u16, 0, 0, 0]));
        let mut beliefs = [Belief::default(); 2];
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED), "the first Outrage locks the user in");
        step(&mut beliefs, &mut state, &teams, 0, 0);
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert_eq!(beliefs[1].mons[0].moves[0], MOVE_OUTRAGE as u16);
        assert_eq!(beliefs[1].mons[0].move_uses[0], 1, "a locked run costs the opponent one visible use");
    }

    #[test]
    fn locked_move_cut_short_records_one_use() {
        let (mut state, teams) = duel_vs_splash(mon(242, 0, [MOVE_OUTRAGE as u16, 0, 0, 0]));
        let mut beliefs = [Belief::default(); 2];
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert!(state.sides[0].active.has_volatile(VOL_MOVE_LOCKED));
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert_eq!(beliefs[1].mons[0].move_uses[0], 1, "stopping mid-run still books exactly one use");
    }

    #[test]
    fn charge_turn_records_one_use() {
        let (mut state, teams) = duel_vs_splash(mon(242, 0, [MOVE_FLY as u16, 0, 0, 0]));
        let mut beliefs = [Belief::default(); 2];
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert!(state.sides[0].active.has_volatile(VOL_CHARGING), "Fly's first turn is the charge turn");
        assert_eq!(beliefs[1].mons[0].move_uses[0], 1, "the charge turn is where the use shows");
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert!(!state.sides[0].active.has_volatile(VOL_CHARGING), "the second turn releases");
        assert_eq!(beliefs[1].mons[0].move_uses[0], 1, "the release turn adds nothing");
    }

    #[test]
    fn sleeping_turn_records_no_use() {
        let (mut state, teams) = duel_vs_splash(mon(242, 0, [MOVE_BODY_SLAM as u16, 0, 0, 0]));
        state.sides[0].team[0].status = STATUS_SLEEP;
        state.sides[0].team[0].status_counter = 3;
        let mut beliefs = [Belief::default(); 2];
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert_eq!(state.sides[0].team[0].status, STATUS_SLEEP, "the mon really slept through the turn");
        assert_eq!(beliefs[1].mons[0].moves[0], MOVE_BODY_SLAM as u16, "the attempted move is still revealed");
        assert_eq!(beliefs[1].mons[0].move_uses[0], 0, "a move that never executed costs no PP");
    }

    #[test]
    fn actor_dragged_out_mid_turn_still_books_its_own_use() {
        let (mut state, teams) = build_state(
            vec![
                mon(242, 0, [MOVE_U_TURN as u16, 0, 0, 0]),
                mon(143, 0, [MOVE_SPLASH as u16, 0, 0, 0]),
            ],
            vec![mon(130, 0, [MOVE_SPLASH as u16, 0, 0, 0])],
        );
        state.sides[1].team[0].item_id = data_bridge::ITEM_RED_CARD;
        let mut beliefs = [Belief::default(); 2];
        let bench = beliefs[1].note_species(143, state.sides[0].team[1].level);
        beliefs[1].note_move(bench, MOVE_SPLASH as u16);
        step(&mut beliefs, &mut state, &teams, 0, 0);
        assert_eq!(state.sides[0].active_index, 1, "Red Card dragged the pivoting user out");
        let pivot = beliefs[1].mons.iter().position(|m| m.species_id == 242).unwrap();
        assert_eq!(beliefs[1].mons[pivot].moves[0], MOVE_U_TURN as u16);
        assert_eq!(beliefs[1].mons[pivot].move_uses[0], 1, "the use lands on the mon that acted");
        assert_eq!(beliefs[1].mons[bench].move_uses, [0; 4], "the dragged-in mon spent nothing");
    }

    #[test]
    fn the_native_loop_books_one_use_per_completed_turn() {
        let (state, teams) = build_state(
            vec![mon(242, 0, [MOVE_SPLASH as u16, 0, 0, 0])],
            vec![mon(143, 0, [MOVE_SPLASH as u16, 0, 0, 0])],
        );
        let seen = std::cell::RefCell::new(Vec::new());
        play_to_terminal(state, &teams, [Belief::default(); 2], 7, |side, st, _tm, bel, _seed, _rng| {
            if side == 0 {
                seen.borrow_mut().push(bel[1]);
            }
            let idx = st.sides[side].active_index as usize;
            if st.sides[side].team[idx].pp[0] > 0 { 0 } else { ACTION_STRUGGLE }
        });
        let seen = seen.into_inner();
        assert!(seen.len() > 4, "the duel ran several turns, got {}", seen.len());
        for (turn, b) in seen.iter().take(5).enumerate() {
            assert_eq!(b.mons[0].move_uses[0] as usize, turn, "turn {turn} sees one use per prior turn");
        }
    }

    #[test]
    fn the_native_loop_seeds_a_full_candidate_pool_on_reveal() {
        use crate::belief::{pm_get, species_sets_with_base_fallback};
        let (state, teams) = build_state(
            vec![mon(242, 0, [MOVE_SPLASH as u16, 0, 0, 0])],
            vec![mon(143, 0, [MOVE_SPLASH as u16, 0, 0, 0])],
        );
        let seen = std::cell::RefCell::new(None);
        play_to_terminal(state, &teams, [Belief::default(); 2], 7, |side, st, _tm, bel, _seed, _rng| {
            if seen.borrow().is_none() {
                *seen.borrow_mut() = Some(*bel);
            }
            let idx = st.sides[side].active_index as usize;
            if st.sides[side].team[idx].pp[0] > 0 { 0 } else { ACTION_STRUGGLE }
        });
        let seen = seen.into_inner().expect("the duel made a decision");
        for (viewer, species) in [(0usize, 143u16), (1, 242)] {
            let m = &seen[viewer].mons[0];
            assert_eq!(m.species_id, species);
            assert!(m.pool_active, "viewer {viewer} sees a seeded pool on the first reveal");
            let n = species_sets_with_base_fallback(species).unwrap().sets.len();
            assert!((0..n).all(|i| pm_get(&m.pool_mask, i)), "every set of the species starts possible");
        }
    }

    // Make `opp_side`'s active a genuine last mon (1 HP) backed by 5 revealed-fainted teammates, and
    // return our view of it. Since play_from re-determinizes the opponent from this belief, the bench
    // must be present-and-fainted (not empty) so it stays fainted instead of being fabricated alive.
    fn last_mon_opp(state: &mut BattleState, opp_side: usize) -> Belief {
        state.sides[opp_side].team[0].current_hp = 1;
        for (i, &sid) in [130u16, 248, 94, 6, 9].iter().enumerate() {
            state.sides[opp_side].team[i + 1] =
                MonSlot { species_id: sid, current_hp: 0, max_hp: 100, level: 80, ..Default::default() };
        }
        let mut b = Belief::default();
        for i in 0..6 {
            let m = &state.sides[opp_side].team[i];
            b.note_species(m.species_id, m.level);
        }
        b
    }

    #[test]
    fn forced_ko_wins_for_our_side() {
        // side 0 (us): healthy attacker with a damaging move in slot 0.
        // side 1 (opp): a last mon at 1 HP so the forced move guarantees the KO.
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(143, 47, [34, 0, 0, 0])],
        );
        let our_belief = last_mon_opp(&mut state, 1);
        let beliefs = [our_belief, reconstruct_opp_belief(&state, 0)];

        let v1 = play_from(&state, &teams, &beliefs, 0, 0, Policy::Mcts, 7);
        let v2 = play_from(&state, &teams, &beliefs, 0, 0, Policy::Mcts, 7);
        assert!(v1 > 0.99, "forced KO must win for side 0, got {v1}");
        assert_eq!(v1, v2, "fixed seed must be deterministic");
    }

    #[test]
    fn forced_ko_wins_for_side_one() {
        // side 1 (us) KOs side 0's last mon with a forced first move; our_side = 1.
        let (mut state, teams) = build_state(
            vec![mon(143, 47, [34, 0, 0, 0])],
            vec![mon(445, 24, [89, 14, 0, 0])],
        );
        let our_belief = last_mon_opp(&mut state, 0);
        let beliefs = [reconstruct_opp_belief(&state, 1), our_belief];

        let v = play_from(&state, &teams, &beliefs, 1, 0, Policy::Mcts, 7);
        assert!(v > 0.99, "forced KO must win from side 1's perspective, got {v}");
    }

    #[test]
    fn ground_truth_determinizes_real_opponent() {
        // Observed placeholder: the opponent has 0 stats, 0 moves, max_hp=100 (percent scale).
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(143, 47, [34, 0, 0, 0])],
        );
        {
            let m = &mut state.sides[1].team[0];
            m.stats = [0; 5];
            m.moves = [0; 4];
            m.pp = [0; 4];
            m.ability_id = 0;
            m.item_id = 0;
            m.max_hp = 100;
            m.current_hp = 100;
        }
        let our_real_active = state.sides[0].team[0];
        let opp = state.active_mon(1);
        let mut belief = Belief::default();
        belief.note_species(opp.species_id, opp.level);

        let (gt, _) = determinize_ground_truth(&state, &teams, &belief, 0, 7);
        let opp_after = gt.active_mon(1);
        assert_ne!(opp_after.max_hp, 100, "opponent must get a real max-HP, not the percent placeholder");
        assert_ne!(opp_after.moves, [0, 0, 0, 0], "opponent must get real moves, not placeholder zeros");
        assert!(opp_after.stats.iter().any(|&s| s != 0), "opponent must get real stats");
        assert_eq!(gt.sides[0].team[0], our_real_active, "our real side must be preserved verbatim");

        // Same game_seed -> the same sampled opponent world (the load-bearing CRN pairing across arms).
        let (gt2, _) = determinize_ground_truth(&state, &teams, &belief, 0, 7);
        assert_eq!(gt.sides[1].team[0], gt2.sides[1].team[0], "same game_seed must sample the same opponent");
    }

    #[test]
    fn determinizer_blocker_guard() {
        const OUR_ACTIVE: u16 = 445; // Garchomp
        const SEED: u64 = 12345;
        let (state, teams) = build_state(
            vec![mon(OUR_ACTIVE, 24, [89, 14, 0, 0])],
            vec![mon(130, 22, [57, 0, 0, 0])],
        );
        let obs = Observation { state: &state, teams: &teams, our_side: 1 };

        let preserved = |belief: &Belief| -> usize {
            let mut rng = Lcg::new(SEED);
            let worlds = RandomBattle.sample_worlds(&obs, belief, 8, &mut rng);
            worlds
                .iter()
                .filter(|w| {
                    let idx = w.state.sides[0].active_index as usize;
                    w.state.sides[0].team[idx].species_id == OUR_ACTIVE
                })
                .count()
        };

        // (a) empty belief on side 1 -> side 0 (us) sampled fully random -> our active not preserved.
        let empty = Belief::default();
        assert_eq!(preserved(&empty), 0, "empty belief must not preserve our active");

        // (b) a belief naming a SIDE-1 (opp) species, fed as side 1's belief about side 0, must panic.
        let opp_active = state.active_mon(1);
        let mut our_view_of_opp = Belief::default();
        our_view_of_opp.note_species(opp_active.species_id, opp_active.level);
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut rng = Lcg::new(SEED);
            RandomBattle.sample_worlds(&obs, &our_view_of_opp, 8, &mut rng)
        }));
        std::panic::set_hook(prev_hook);
        assert!(r.is_err(), "a belief naming a non-side-0 species must panic the determinizer");

        // (c) reconstructed belief about side 0 -> our active preserved in every world.
        let recon = reconstruct_opp_belief(&state, 0);
        assert_eq!(preserved(&recon), 8, "reconstructed belief must preserve our active in all worlds");
    }

    #[test]
    fn reconstruct_notes_active_and_fainted_only() {
        const OUR_ACTIVE: u16 = 445; // Garchomp
        const FAINTED: u16 = 25; // Pikachu
        const LIVING: u16 = 143; // Snorlax
        let (mut state, _teams) = build_state(
            vec![
                mon(OUR_ACTIVE, 24, [89, 14, 0, 0]),
                mon(FAINTED, 9, [85, 0, 0, 0]),
                mon(LIVING, 47, [34, 0, 0, 0]),
            ],
            vec![mon(130, 22, [57, 0, 0, 0])],
        );
        state.sides[0].team[1].current_hp = 0;

        let b = reconstruct_opp_belief(&state, 0);
        let has = |sid: u16| b.mons.iter().any(|m| m.species_id == sid);
        assert!(has(OUR_ACTIVE), "active species noted");
        assert!(has(FAINTED), "fainted bench species noted");
        assert!(!has(LIVING), "living bench species must not be noted");
        assert_eq!(b.revealed_count(), 2, "only active + fainted revealed");
    }

    #[test]
    fn timed_loop_matches_untimed_result() {
        let mut rng = Lcg::new(3);
        let a = crate::frontier::gen_team(&mut rng);
        let b = crate::frontier::gen_team(&mut rng);
        let (state, teams) = crate::frontier::initial_state(&a, &b);
        let chooser = |side: usize, st: &BattleState, _: &TeamData, _: &[Belief; 2], _: u64, rng: &mut Lcg| {
            random_action(st, side, rng)
        };
        let plain = play_to_terminal(state, &teams, [Belief::default(); 2], 11, chooser);
        let mut lt = LoopTimes::default();
        let with_times = play_to_terminal_timed(state, &teams, [Belief::default(); 2], 11, chooser, Some(&mut lt));
        assert_eq!(plain, with_times);
        assert!(lt.steps > 0);
        assert!(lt.turns > 0);
        assert!(lt.engine > std::time::Duration::ZERO);
        assert!(lt.belief > std::time::Duration::ZERO);
    }
}
