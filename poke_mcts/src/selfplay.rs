use crate::belief::{engine_type_to_showdown, Belief};
use crate::determinize::{Observation, RandomBattle};
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
            };
            choose_action(&obs, &beliefs[side], &RandomBattle, &cfg)
        }
    }
}

// Shared terminal loop lifted from the self-play binary. Both play_from and the bin's play call it.
pub fn play_to_terminal<F>(
    mut state: BattleState,
    teams: &TeamData,
    mut beliefs: [Belief; 2],
    game_seed: u64,
    mut choose: F,
) -> f64
where
    F: FnMut(usize, &BattleState, &TeamData, &[Belief; 2], u64, &mut Lcg) -> u8,
{
    let note_active = |beliefs: &mut [Belief; 2], state: &BattleState| {
        for s in 0..2 {
            let om = state.active_mon(1 - s);
            if om.species_id != 0 && om.current_hp > 0 {
                beliefs[s].note_species(om.species_id, om.level);
            }
        }
    };
    note_active(&mut beliefs, &state);

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
        for (s, a) in [(0usize, a1), (1usize, a2)] {
            if state.phase != PHASE_ACTIONS {
                continue;
            }
            let viewer = 1 - s;
            let slot = beliefs[viewer].note_species(state.active_mon(s).species_id, state.active_mon(s).level);
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
        match state.phase {
            PHASE_ACTIONS => execute_turn(&mut state, teams, a1, a2, &mut |m| battle_rng.roll(m)),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(&mut state, teams, a1, a2, &mut |m| battle_rng.roll(m))
            }
            _ => break,
        }
        note_active(&mut beliefs, &state);
    }
    winner_value(&state)
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
    let mut forced_done = false;
    let v = play_to_terminal(*state, teams, *beliefs, game_seed, move |side, st, tm, bel, seed, rng| {
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

    #[test]
    fn forced_ko_wins_for_our_side() {
        // side 0 (us): healthy attacker with a damaging move in slot 0.
        // side 1 (opp): a single last mon set to 1 HP so the forced move guarantees the KO.
        let (mut state, teams) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0])],
            vec![mon(143, 47, [34, 0, 0, 0])],
        );
        state.sides[1].team[0].current_hp = 1;

        let opp_active = state.active_mon(1);
        let mut our_belief = Belief::default();
        our_belief.note_species(opp_active.species_id, opp_active.level);
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
        state.sides[0].team[0].current_hp = 1;

        let opp_active = state.active_mon(0);
        let mut our_belief = Belief::default();
        our_belief.note_species(opp_active.species_id, opp_active.level);
        let beliefs = [reconstruct_opp_belief(&state, 1), our_belief];

        let v = play_from(&state, &teams, &beliefs, 1, 0, Policy::Mcts, 7);
        assert!(v > 0.99, "forced KO must win from side 1's perspective, got {v}");
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
}
