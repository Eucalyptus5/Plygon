//! Legal action generation.

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::state::accessors::*;
use crate::data::moves::MoveFlags;

/// Move IDs that are blocked by Gravity (all moves with `gravity: 1` in Showdown).
const GRAVITY_BLOCKED: [u16; 9] = [
    19,  // Fly
    26,  // Jump Kick
    136, // High Jump Kick
    150, // Splash
    340, // Bounce
    393, // Magnet Rise
    477, // Telekinesis
    507, // Sky Drop
    560, // Flying Press
];

#[derive(Debug, Clone, Copy, Default)]
pub struct ActionList {
    pub actions: [u8; 10],
    pub count: u8,
}

impl ActionList {
    pub fn new() -> Self { Self { actions: [0; 10], count: 0 } }
    #[inline(always)]
    fn push(&mut self, action: u8) { self.actions[self.count as usize] = action; self.count += 1; }
    pub fn as_slice(&self) -> &[u8] { &self.actions[..self.count as usize] }
    pub fn is_empty(&self) -> bool { self.count == 0 }
}

pub fn legal_actions(state: &BattleState, side: usize) -> ActionList {
    match state.phase {
        PHASE_ACTIONS => generate_full_actions(state, side),
        PHASE_SWITCH_P1 if side == 0 => generate_switch_only(state, side),
        PHASE_SWITCH_P2 if side == 1 => generate_switch_only(state, side),
        PHASE_SWITCH_BOTH => generate_switch_only(state, side),
        _ => ActionList::new(),
    }
}

fn generate_full_actions(state: &BattleState, side: usize) -> ActionList {
    let mut list = ActionList::new();
    // Recharging: forced to skip turn, no choices at all (no moves, no switches)
    if state.sides[side].active.has_volatile(VOL_RECHARGING) {
        list.push(ACTION_STRUGGLE);
        return list;
    }
    let move_count = generate_legal_moves(state, side, &mut list);
    if move_count == 0 { list.push(ACTION_STRUGGLE); }
    generate_legal_switches(state, side, &mut list);
    if can_tera(state, side) {
        list.push(ACTION_TERA);
    }
    list
}

/// Check if the side can Terastallize.
#[inline]
fn can_tera(state: &BattleState, side: usize) -> bool {
    // _padding[0] bit 0 = tera used this battle
    if state.sides[side]._padding[0] & 1 != 0 { return false; }
    let mon = state.active_mon(side);
    if mon.is_fainted() { return false; }
    if mon.is_terastallized() { return false; }
    if mon.tera_type == 0 { return false; }
    true
}

fn generate_switch_only(state: &BattleState, side: usize) -> ActionList {
    let mut list = ActionList::new();
    generate_legal_switches(state, side, &mut list);
    list
}

fn generate_legal_moves(state: &BattleState, side: usize, list: &mut ActionList) -> u8 {
    let active = &state.sides[side].active;
    let moves = effective_moves(state, side);
    let opp = 1 - side;
    let mut count = 0u8;

    // Charging (turn 2 pending): the only legal move is the stored charge move.
    // execute_move will override move_id to last_move anyway, but MCTS needs
    // choice generation to reflect this constraint.
    if active.has_volatile(VOL_CHARGING) {
        for i in 0..4 {
            if moves[i] == active.last_move && moves[i] != 0 {
                list.push(i as u8); return 1;
            }
        }
        // Fallback: offer slot 0 (execute_move will override to last_move)
        list.push(0); return 1;
    }

    if active.has_volatile(VOL_MOVE_LOCKED) {
        for i in 0..4 {
            if moves[i] == active.last_move && moves[i] != 0 && effective_pp(state, side, i) > 0 {
                // Disable can hit mid-Outrage: if locked move is also disabled, Struggle
                if active.disabled_move != 0 && active.disabled_move == moves[i] {
                    return 0;
                }
                list.push(i as u8); return 1;
            }
        }
        return 0;
    }

    if active.encore_turns > 0 && active.encore_move != 0 {
        for i in 0..4 {
            if moves[i] == active.encore_move && effective_pp(state, side, i) > 0 {
                // Stacking: encored move may also be blocked by Disable, Taunt, or Assault Vest
                if active.disabled_move != 0 && active.disabled_move == moves[i] {
                    return 0;
                }
                if active.taunt_turns > 0 {
                    let md = data_bridge::move_hot(moves[i]);
                    if md.category == MoveCategory::Status { return 0; }
                }
                if state.field.magic_room_turns() == 0 {
                    if data_bridge::item(state.sides[side].team[state.sides[side].active_index as usize].item_id)
                        .has(data_bridge::ItemFlag::ASSAULT_VEST)
                    {
                        let md = data_bridge::move_hot(moves[i]);
                        if md.category == MoveCategory::Status { return 0; }
                    }
                }
                if active.heal_block_turns > 0 {
                    let md = data_bridge::move_hot(moves[i]);
                    if md.flags & MoveFlags::HEAL != 0 { return 0; }
                }
                list.push(i as u8); return 1;
            }
        }
        return 0;
    }

    for i in 0..4 {
        if moves[i] == 0 { continue; }
        if effective_pp(state, side, i) == 0 { continue; }
        if active.disabled_move != 0 && active.disabled_move == moves[i] { continue; }
        if active.taunt_turns > 0 {
            let md = data_bridge::move_hot(moves[i]);
            if md.category == MoveCategory::Status { continue; }
        }
        // Assault Vest: block Status-category moves (suppressed by Magic Room)
        if state.field.magic_room_turns() == 0 {
            if data_bridge::item(state.sides[side].team[state.sides[side].active_index as usize].item_id)
                .has(data_bridge::ItemFlag::ASSAULT_VEST)
            {
                let md = data_bridge::move_hot(moves[i]);
                if md.category == MoveCategory::Status { continue; }
            }
        }
        if active.heal_block_turns > 0 {
            let md = data_bridge::move_hot(moves[i]);
            if md.flags & MoveFlags::HEAL != 0 { continue; }
        }
        if active.has_volatile(VOL_TORMENT) && moves[i] == active.last_move { continue; }
        // Choice lock: suppressed by Magic Room (items), but Gorilla Tactics ability lock
        // is not suppressed by Magic Room.
        if active.choice_locked_move != 0 && moves[i] != active.choice_locked_move {
            let locked_by_gt = effective_ability(state, side)
                == data_bridge::ABILITY_GORILLA_TACTICS;
            if locked_by_gt || state.field.magic_room_turns() == 0 { continue; }
        }
        // Gravity: block flying/levitation moves
        if state.field.gravity_turns > 0 && GRAVITY_BLOCKED.contains(&moves[i]) { continue; }
        let opp_active = &state.sides[opp].active;
        if opp_active.has_volatile(VOL_IMPRISON) {
            let opp_moves = effective_moves(state, opp);
            if opp_moves.contains(&moves[i]) { continue; }
        }
        list.push(i as u8);
        count += 1;
    }
    count
}

fn generate_legal_switches(state: &BattleState, side: usize, list: &mut ActionList) {
    let s = &state.sides[side];
    let current_idx = s.active_index as usize;

    // Only PHASE_ACTIONS honors trapping (forced replacement must be allowed).
    if state.phase == PHASE_ACTIONS && is_trapped(state, side) {
        return;
    }

    for j in 0..6 {
        if j == current_idx { continue; }
        if s.team[j].current_hp == 0 { continue; }
        if s.team[j].species_id == 0 { continue; }
        list.push(ACTION_SWITCH_0 + j as u8);
    }
}

/// Returns true if the active mon cannot use any normal move this turn
/// (all moves blocked by disable/choice-lock/taunt/pp=0/imprison/etc.)
/// and must therefore fall back to Struggle.
///
/// Matches the same filter logic as `generate_legal_moves` — when it would
/// return count==0, this returns true.
#[inline]
pub fn must_struggle(state: &BattleState, side: usize) -> bool {
    // Recharging / charge-turn-2 / move-lock paths have their own forced move.
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_RECHARGING) { return true; }
    if active.has_volatile(VOL_CHARGING) { return false; }
    if active.has_volatile(VOL_MOVE_LOCKED) { return false; }
    // Fast path: no blocking volatiles in effect → legal-move generation will
    // return >=1 as long as any move slot has positive PP. This is the overwhelmingly
    // common case in MCTS rollouts (no Disable, no Encore, no Choice lock, no
    // Taunt, no Torment, no Assault Vest, no Heal Block, no Imprison, no Gravity).
    if active.disabled_move == 0
        && active.encore_turns == 0
        && active.choice_locked_move == 0
        && active.taunt_turns == 0
        && active.heal_block_turns == 0
        && !active.has_volatile(VOL_TORMENT)
        && !state.sides[1 - side].active.has_volatile(VOL_IMPRISON)
        && state.field.gravity_turns == 0
    {
        let moves = effective_moves(state, side);
        for i in 0..4 {
            if moves[i] != 0 && effective_pp(state, side, i) > 0 {
                return false;
            }
        }
        return true;
    }
    let mut list = ActionList::new();
    let count = generate_legal_moves(state, side, &mut list);
    count == 0
}

pub fn available_switches(state: &BattleState, side: usize) -> u8 {
    let s = &state.sides[side];
    let current = s.active_index as usize;
    (0..6).filter(|&j| j != current && s.team[j].current_hp > 0 && s.team[j].species_id != 0).count() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_state() -> BattleState {
        let mut s = BattleState::default();
        s.sides[0].team[0] = MonSlot { species_id: 0, current_hp: 200, max_hp: 200,
            moves: [85, 521, 447, 417], pp: [24, 32, 32, 32], ..Default::default() };
        s.sides[0].team[1] = MonSlot { species_id: 1, current_hp: 100, max_hp: 100, ..Default::default() };
        s.sides[0].team[2] = MonSlot { species_id: 1, current_hp: 100, max_hp: 100, ..Default::default() };
        s.phase = PHASE_ACTIONS;
        s
    }
    #[test] fn test_normal() { let s = make_state(); assert_eq!(legal_actions(&s, 0).count, 6); }
    #[test] fn test_no_pp_struggle() { let mut s = make_state(); s.sides[0].team[0].pp = [0;4]; assert!(legal_actions(&s, 0).as_slice().contains(&ACTION_STRUGGLE)); }
    #[test] fn test_trapped() { let mut s = make_state(); s.sides[0].active.volatile_flags |= VOL_TRAPPED; let a = legal_actions(&s, 0); assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0)); }

    #[test]
    fn test_charging_restricts_to_charged_move() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_CHARGING;
        s.sides[0].active.last_move = 521;
        let a = legal_actions(&s, 0);
        assert_eq!(a.count, 1);
        assert_eq!(a.actions[0], 1);
        assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0));
    }

    #[test]
    fn test_charging_blocks_switches() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_CHARGING;
        s.sides[0].active.last_move = 85;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0));
    }

    #[test]
    fn test_move_locked_restricts_to_locked_move() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_MOVE_LOCKED;
        s.sides[0].active.last_move = 447;
        let a = legal_actions(&s, 0);
        assert_eq!(a.count, 1);
        assert_eq!(a.actions[0], 2);
    }

    #[test]
    fn test_must_switch_returns_only_switches() {
        let mut s = make_state();
        // VOL_MUST_SWITCH triggers a switch phase via faint_sweep,
        // so test in PHASE_SWITCH_P1 (the actual phase after faint_sweep)
        s.phase = PHASE_SWITCH_P1;
        let a = legal_actions(&s, 0);
        assert_eq!(a.count, 2);
        assert!(a.as_slice().iter().all(|&x| x >= ACTION_SWITCH_0));
    }

    #[test]
    fn test_gravity_blocks_fly() {
        let mut s = make_state();
        // Move slot 0 = 85, slot 1 = 521, slot 2 = 447, slot 3 = 417
        // Replace slot 0 with Fly (19)
        s.sides[0].team[0].moves[0] = 19;
        s.field.gravity_turns = 5;
        let a = legal_actions(&s, 0);
        // Fly should be blocked, 3 other moves + 2 switches
        assert!(!a.as_slice().iter().any(|&x| x == ACTION_MOVE_0));
    }

    #[test]
    fn test_gravity_blocks_high_jump_kick() {
        let mut s = make_state();
        s.sides[0].team[0].moves[0] = 136; // High Jump Kick
        s.field.gravity_turns = 5;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().iter().any(|&x| x == ACTION_MOVE_0));
    }

    #[test]
    fn test_all_restricted_struggle_only() {
        let mut s = make_state();
        // Disable all moves via Taunt (blocks Status) + make all moves Status category
        // Easier: set all PP to 0
        s.sides[0].team[0].pp = [0; 4];
        let a = legal_actions(&s, 0);
        // Should have Struggle + 2 switches
        assert!(a.as_slice().contains(&ACTION_STRUGGLE));
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert!(move_actions.is_empty());
    }

    #[test]
    fn test_trapped_no_switch() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_TRAPPED;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0 && x <= ACTION_SWITCH_5));
    }

    #[test]
    fn test_assault_vest_no_status() {
        let mut s = make_state();
        // Moves: [85]=Thunderbolt, [521]=Volt Switch, [447]=Grass Knot, [417]=Nasty Plot (Status)
        s.sides[0].team[0].item_id = 581; // Assault Vest
        let a = legal_actions(&s, 0);
        // Nasty Plot (slot 3, Status) should be blocked
        assert!(!a.as_slice().contains(&3u8)); // ACTION_MOVE_3 = 3
        // Other 3 moves should be available
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 3);
    }

    #[test]
    fn test_phase_switch_only_switches() {
        let mut s = make_state();
        s.phase = PHASE_SWITCH_P1;
        let a = legal_actions(&s, 0);
        assert!(a.as_slice().iter().all(|&x| x >= ACTION_SWITCH_0));
        assert!(!a.as_slice().contains(&ACTION_TERA));
    }

    #[test]
    fn test_heal_block_excludes_healing_moves() {
        let mut s = make_state();
        // Replace slot 0 with Recover (105, has HEAL flag)
        s.sides[0].team[0].moves[0] = 105;
        s.sides[0].active.heal_block_turns = 3;
        let a = legal_actions(&s, 0);
        // Recover (slot 0) should be blocked
        assert!(!a.as_slice().contains(&0u8));
        // Other moves still available
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 3);
    }

    #[test]
    fn test_bound_no_switch() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_BOUND;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0 && x <= ACTION_SWITCH_5));
    }

    #[test]
    fn test_ingrain_no_switch() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_INGRAIN;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().iter().any(|&x| x >= ACTION_SWITCH_0 && x <= ACTION_SWITCH_5));
    }

    #[test]
    fn test_game_over_no_actions() {
        let mut s = make_state();
        s.phase = PHASE_GAME_OVER;
        let a = legal_actions(&s, 0);
        assert!(a.is_empty());
    }

    #[test]
    fn test_struggle_plus_trapped() {
        // All moves restricted + trapped = Struggle only, no switches
        let mut s = make_state();
        s.sides[0].team[0].pp = [0; 4];
        s.sides[0].active.volatile_flags |= VOL_TRAPPED;
        let a = legal_actions(&s, 0);
        assert_eq!(a.count, 1);
        assert_eq!(a.actions[0], ACTION_STRUGGLE);
    }

    #[test]
    fn test_disable_excludes_move() {
        let mut s = make_state();
        s.sides[0].active.disabled_move = 85; // Disable Thunderbolt (slot 0)
        s.sides[0].active.disable_turns = 3;
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().contains(&0u8)); // slot 0 blocked
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 3);
    }

    #[test]
    fn test_torment_excludes_last_move() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_TORMENT;
        s.sides[0].active.last_move = 521; // Last used Volt Switch (slot 1)
        let a = legal_actions(&s, 0);
        assert!(!a.as_slice().contains(&1u8)); // slot 1 blocked
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 3);
    }

    #[test]
    fn test_encore_restricts_to_one_move() {
        let mut s = make_state();
        s.sides[0].active.encore_turns = 3;
        s.sides[0].active.encore_move = 85; // Encore'd into Thunderbolt (slot 0)
        let a = legal_actions(&s, 0);
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 1);
        assert_eq!(move_actions[0], 0);
    }

    #[test]
    fn test_recharging_struggle_only() {
        let mut s = make_state();
        s.sides[0].active.volatile_flags |= VOL_RECHARGING;
        let a = legal_actions(&s, 0);
        // Recharging: forced to pass (Struggle action), no switches
        assert_eq!(a.count, 1);
        assert_eq!(a.actions[0], ACTION_STRUGGLE);
    }

    #[test]
    fn test_imprison_blocks_shared_moves() {
        let mut s = make_state();
        // Set up opponent with Imprison and sharing Thunderbolt (85)
        s.sides[1].team[0] = MonSlot { species_id: 2, current_hp: 200, max_hp: 200,
            moves: [85, 100, 200, 300], pp: [24; 4], ..Default::default() };
        s.sides[1].active.volatile_flags |= VOL_IMPRISON;
        let a = legal_actions(&s, 0);
        // Thunderbolt (slot 0) shared with opponent -> blocked
        assert!(!a.as_slice().contains(&0u8));
    }

    #[test]
    fn test_magic_room_suspends_choice_lock() {
        let mut s = make_state();
        s.sides[0].active.choice_locked_move = 521; // locked to slot 1
        // Without Magic Room: only slot 1 is legal
        let a = legal_actions(&s, 0);
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 1);
        assert_eq!(move_actions[0], 1);

        // With Magic Room: choice lock suspended, all moves available
        s.field.set_magic_room_turns(5);
        let a = legal_actions(&s, 0);
        let move_actions: Vec<u8> = a.as_slice().iter().copied().filter(|&x| x <= ACTION_MOVE_3).collect();
        assert_eq!(move_actions.len(), 4);
    }
}
