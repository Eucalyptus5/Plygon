use pkmn_engine::state::*;

pub trait Evaluator { fn eval(&self, state: &BattleState) -> f32; }

/// Baseline leaf evaluator: surviving team members plus remaining HP, zero-sum.
/// Deliberately untuned. Supply your own `Evaluator` for competitive play.
pub struct Handcrafted;

pub fn winner_value(state: &BattleState) -> f64 {
    let alive = |side: usize| {
        (0..6).any(|i| {
            let m = &state.sides[side].team[i];
            m.species_id != 0 && m.current_hp > 0
        })
    };
    match (alive(0), alive(1)) {
        (true, false) => 1.0,
        (false, true) => 0.0,
        _ => 0.5, // mutual double-faint: PHASE_GAME_OVER with both wiped (turn.rs faint_sweep)
    }
}

pub fn sigmoid(x: f32) -> f64 { 1.0 / (1.0 + (-0.0125 * x as f64).exp()) }

const POKEMON_ALIVE: f32 = 30.0;
const POKEMON_HP: f32 = 100.0;

fn eval_one_side(state: &BattleState, side: usize) -> f32 {
    let s = &state.sides[side];
    let mut score = 0.0f32;
    for i in 0..6 {
        let mon = &s.team[i];
        if mon.species_id == 0 || mon.current_hp == 0 { continue; }
        score += POKEMON_ALIVE + POKEMON_HP * mon.current_hp as f32 / mon.max_hp.max(1) as f32;
    }
    score
}

impl Evaluator for Handcrafted {
    fn eval(&self, state: &BattleState) -> f32 {
        eval_one_side(state, 0) - eval_one_side(state, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn winner_value_covers_win_loss_draw() {
        let (mut s, _t) = duel(mon(25, 9, [85, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        assert_eq!(winner_value(&s), 1.0);
        s.sides[1].team[0].current_hp = 100;
        s.sides[0].team[0].current_hp = 0;
        assert_eq!(winner_value(&s), 0.0);
        s.sides[1].team[0].current_hp = 0; // mutual wipe = draw (faint_sweep has no winner field)
        assert_eq!(winner_value(&s), 0.5);
    }

    #[test]
    fn sigmoid_shape() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-9);
        assert!(sigmoid(200.0) > 0.9 && sigmoid(-200.0) < 0.1);
        assert!(sigmoid(1e6) <= 1.0 && sigmoid(-1e6) >= 0.0);
    }

    #[test]
    fn eval_zero_sum_on_mirror() {
        let (s, _t) = duel(mon(445, 24, [89, 14, 0, 0]), mon(445, 24, [89, 14, 0, 0]));
        assert!(Handcrafted.eval(&s).abs() < 1e-3);
    }

    #[test]
    fn eval_sign_tracks_hp() {
        let (mut s, _t) = duel(mon(445, 24, [89, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp /= 2;
        assert!(Handcrafted.eval(&s) > 0.0);
    }

    #[test]
    fn fainted_mon_scores_nothing() {
        let (mut s, _t) = duel(mon(445, 24, [89, 0, 0, 0]), mon(445, 24, [89, 0, 0, 0]));
        s.sides[1].team[0].current_hp = 0;
        let both_alive = eval_one_side(&s, 0);
        assert!(eval_one_side(&s, 1) == 0.0 && both_alive > 0.0);
    }
}
