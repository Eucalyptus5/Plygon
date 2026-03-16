use pkmn_engine::state::BattleState;
use pkmn_engine::state::legal_moves::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::data_bridge::*;
use pkmn_engine::data::types::Type;
use pkmn_engine::data::items::ItemFlag;

fn setup() -> BattleState {
    let mut state = BattleState::default();
    
    // Give team 6 mons each
    for i in 0..6 {
        state.sides[0].team[i].species_id = 1;
        state.sides[0].team[i].current_hp = 100;
        state.sides[0].team[i].max_hp = 100;
        
        state.sides[1].team[i].species_id = 1;
        state.sides[1].team[i].current_hp = 100;
        state.sides[1].team[i].max_hp = 100;
    }
    
    // Active mon moves
    state.sides[0].team[0].moves = [1, 2, 3, 4];
    state.sides[0].team[0].pp = [10, 10, 10, 10];
    
    state.sides[1].team[0].moves = [5, 6, 7, 8];
    state.sides[1].team[0].pp = [10, 10, 10, 10];
    
    state.phase = PHASE_ACTIONS;
    
    state
}

#[test]
fn test_phase_actions_full_team() {
    let state = setup();
    let actions = legal_actions(&state, 0);
    // 4 moves + 5 switches = 9 actions
    assert_eq!(actions.count, 9);
    for i in 0..4 {
        assert_eq!(actions.actions[i], ACTION_MOVE_0 + i as u8);
    }
    for i in 0..5 {
        assert_eq!(actions.actions[4 + i], ACTION_SWITCH_0 + 1 + i as u8);
    }
}

#[test]
fn test_phase_actions_some_fainted() {
    let mut state = setup();
    state.sides[0].team[2].current_hp = 0; // Fainted
    state.sides[0].team[4].current_hp = 0; // Fainted
    state.sides[0].team[5].current_hp = 0; // Fainted
    
    let actions = legal_actions(&state, 0);
    // 4 moves + 2 switches (idx 1, 3) = 6 actions
    assert_eq!(actions.count, 6);
    assert_eq!(actions.actions[0], ACTION_MOVE_0);
    assert_eq!(actions.actions[4], ACTION_SWITCH_0 + 1);
    assert_eq!(actions.actions[5], ACTION_SWITCH_0 + 3);
}

#[test]
fn test_phases() {
    let mut state = setup();
    
    state.phase = PHASE_SWITCH_P1;
    let a1 = legal_actions(&state, 0);
    let a2 = legal_actions(&state, 1);
    assert_eq!(a1.count, 5); // 5 switches
    assert_eq!(a2.count, 0); // None
    
    state.phase = PHASE_SWITCH_P2;
    let a1 = legal_actions(&state, 0);
    let a2 = legal_actions(&state, 1);
    assert_eq!(a1.count, 0);
    assert_eq!(a2.count, 5);
    
    state.phase = PHASE_SWITCH_BOTH;
    let a1 = legal_actions(&state, 0);
    let a2 = legal_actions(&state, 1);
    assert_eq!(a1.count, 5);
    assert_eq!(a2.count, 5);
    
    state.phase = PHASE_GAME_OVER;
    let a1 = legal_actions(&state, 0);
    assert_eq!(a1.count, 0);
}

#[test]
fn test_all_pp_gone() {
    let mut state = setup();
    state.sides[0].team[0].pp = [0, 0, 0, 0];
    
    let a = legal_actions(&state, 0);
    // Struggle + 5 switches = 6
    assert_eq!(a.count, 6);
    assert_eq!(a.actions[0], ACTION_STRUGGLE);
}

#[test]
fn test_empty_moves() {
    let mut state = setup();
    state.sides[0].team[0].moves[2] = 0; // Empty
    state.sides[0].team[0].moves[3] = 0; // Empty
    
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 7); // 2 moves + 5 switches
    assert_eq!(a.actions[0], ACTION_MOVE_0);
    assert_eq!(a.actions[1], ACTION_MOVE_0 + 1);
    assert_eq!(a.actions[2], ACTION_SWITCH_0 + 1); // First switch action
}

#[test]
fn test_disabled_move() {
    let mut state = setup();
    state.sides[0].active.disabled_move = 2; // Move 2
    
    let a = legal_actions(&state, 0);
    // 3 moves + 5 switches
    assert_eq!(a.count, 8);
    // Ensure move 2 is missing (moves are 1,2,3,4. move at idx 1 is 2)
    for i in 0..a.count {
        assert_ne!(a.actions[i as usize], ACTION_MOVE_0 + 1);
    }
}

#[test]
fn test_encored_move() {
    let mut state = setup();
    state.sides[0].active.encore_move = 3; // Move 3 (idx 2)
    state.sides[0].active.encore_turns = 1;
    
    let a = legal_actions(&state, 0);
    // 1 move + 5 switches
    assert_eq!(a.count, 6);
    assert_eq!(a.actions[0], ACTION_MOVE_0 + 2);
    
    // Encored + 0 PP -> Struggle
    state.sides[0].team[0].pp[2] = 0;
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 6);
    assert_eq!(a.actions[0], ACTION_STRUGGLE);
}

#[test]
fn test_taunt() {
    let mut state = setup();
    state.sides[0].active.taunt_turns = 1;
    // Normally, moves 1,2,3,4 would be checked for category. 
    // We can't mock MoveCategory without the data table, but wait, `move_id = 1` might be physical or status.
    // If we use moves 0.. we get data table stuff. The stub has 0, but what if we set move_id to something that we know is status?
    // Move "Swords Dance" = 14. We could test if it's excluded. But wait, we don't have the table!
    // The prompt says "Taunted: Status moves excluded".
    // I can just check if Taunt logic works using a known status move, or if the engine just queries the table.
    // Actually, `src/data/generated/gen_moves.rs` has all 915 moves since `gen_species.rs` was fully populated.
    // Let's use Swords Dance (14) which is Status.
    state.sides[0].team[0].moves[0] = 14;
    
    let a = legal_actions(&state, 0);
    // Move 0 should be missing
    for i in 0..a.count {
        assert_ne!(a.actions[i as usize], ACTION_MOVE_0);
    }
    
    // All status -> Struggle
    state.sides[0].team[0].moves = [14, 14, 14, 14];
    let a = legal_actions(&state, 0);
    assert_eq!(a.actions[0], ACTION_STRUGGLE);
}

#[test]
fn test_torment() {
    let mut state = setup();
    state.sides[0].active.set_volatile(VOL_TORMENT);
    state.sides[0].active.last_move = 2; // Move 2 at idx 1
    
    let a = legal_actions(&state, 0);
    // Move 1 should be missing
    for i in 0..a.count {
        assert_ne!(a.actions[i as usize], ACTION_MOVE_0 + 1);
    }
}

#[test]
fn test_choice_locked() {
    let mut state = setup();
    state.sides[0].active.choice_locked_move = 2; // Move 2 (idx 1)
    
    let a = legal_actions(&state, 0);
    // 1 move + 5 switches
    assert_eq!(a.count, 6);
    assert_eq!(a.actions[0], ACTION_MOVE_0 + 1);
    
    // Choice locked + 0 PP -> Struggle
    state.sides[0].team[0].pp[1] = 0;
    let a = legal_actions(&state, 0);
    assert_eq!(a.actions[0], ACTION_STRUGGLE);
}

#[test]
fn test_recharging() {
    let mut state = setup();
    state.sides[0].active.set_volatile(VOL_RECHARGING);
    
    let a = legal_actions(&state, 0);
    // Recharging: forced to skip turn, only Struggle (no switches)
    assert_eq!(a.count, 1);
    assert_eq!(a.actions[0], ACTION_STRUGGLE);
}

#[test]
fn test_move_locked() {
    let mut state = setup();
    state.sides[0].active.set_volatile(VOL_MOVE_LOCKED);
    state.sides[0].active.last_move = 1; // Move 1 (idx 0)
    
    let a = legal_actions(&state, 0);
    // 1 move + 0 switches (move locked prevents switching)
    // Wait, the prompt says "VOL_MOVE_LOCKED: only last_move is legal"
    assert_eq!(a.count, 1);
    assert_eq!(a.actions[0], ACTION_MOVE_0);
}

#[test]
fn test_imprison() {
    let mut state = setup();
    state.sides[1].active.set_volatile(VOL_IMPRISON);
    // Imprison blocks moves that the opponent knows
    // Opponent is side 1. Does it block side 0 from using moves side 1 knows?
    // Opponent knows 5, 6, 7, 8. Side 0 knows 1, 2, 3, 5.
    state.sides[0].team[0].moves[3] = 5;
    
    let a = legal_actions(&state, 0);
    // Move 5 (idx 3) should be missing
    for i in 0..a.count {
        assert_ne!(a.actions[i as usize], ACTION_MOVE_3);
    }
}

#[test]
fn test_trapping() {
    let mut state = setup();
    
    // VOL_TRAPPED
    state.sides[0].active.set_volatile(VOL_TRAPPED);
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 4); // 4 moves, 0 switches
    
    state.sides[0].active.clear_volatile(VOL_TRAPPED);
    state.sides[0].active.set_volatile(VOL_INGRAIN);
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 4); // 4 moves, 0 switches
    
    state.sides[0].active.clear_volatile(VOL_INGRAIN);
    state.sides[0].active.set_volatile(VOL_BOUND);
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 4); // 4 moves, 0 switches
}

#[test]
fn test_trap_immunity() {
    let mut state = setup();
    state.sides[0].active.set_volatile(VOL_TRAPPED);
    
    // Ghost type
    state.sides[0].active.override_types = [Type::Ghost as u8, Type::Ghost as u8];
    state.sides[0].active.set_volatile(VOL_TYPES_OVERRIDDEN);
    
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 9); // Immune to trap, switches available
    
    // Shed Shell
    state.sides[0].active.clear_volatile(VOL_TYPES_OVERRIDDEN);
    state.sides[0].team[0].item_id = 437; // Shed Shell
    let a = legal_actions(&state, 0);
    assert_eq!(a.count, 9); // Immune
}

#[test]
fn test_forced_switch_phase_ignores_trapping() {
    let mut state = setup();
    state.phase = PHASE_SWITCH_P1;
    state.sides[0].active.set_volatile(VOL_TRAPPED);
    
    let a = legal_actions(&state, 0);
    // Forced switch -> trapping doesn't matter
    assert_eq!(a.count, 5); 
}

#[test]
fn test_active_mon_not_switch_target() {
    let state = setup();
    // active_index is 0. So ACTION_SWITCH_0 should not be in the list.
    let a = legal_actions(&state, 0);
    for i in 0..a.count {
        assert_ne!(a.actions[i as usize], ACTION_SWITCH_0);
    }
}
