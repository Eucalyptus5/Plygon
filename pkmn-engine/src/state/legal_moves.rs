//! Legal action generation.

use crate::state::structs::*;
use crate::state::data_bridge;
use crate::state::accessors::*;

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
    let move_count = generate_legal_moves(state, side, &mut list);
    if move_count == 0 { list.push(ACTION_STRUGGLE); }
    generate_legal_switches(state, side, &mut list);
    list
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

    if active.has_volatile(VOL_RECHARGING) { return 0; }

    if active.has_volatile(VOL_MOVE_LOCKED) {
        for i in 0..4 {
            if moves[i] == active.last_move && moves[i] != 0 && effective_pp(state, side, i) > 0 {
                list.push(i as u8); return 1;
            }
        }
        return 0;
    }

    if active.encore_turns > 0 && active.encore_move != 0 {
        for i in 0..4 {
            if moves[i] == active.encore_move && effective_pp(state, side, i) > 0 {
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
        if active.has_volatile(VOL_TORMENT) && moves[i] == active.last_move { continue; }
        if active.choice_locked_move != 0 && moves[i] != active.choice_locked_move { continue; }
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
    let active = &s.active;
    let current_idx = s.active_index as usize;

    if state.phase == PHASE_ACTIONS && !is_trap_immune(state, side) {
        if active.has_volatile(VOL_TRAPPED) || active.has_volatile(VOL_INGRAIN)
           || active.has_volatile(VOL_BOUND) || active.has_volatile(VOL_MOVE_LOCKED) {
            return;
        }
    }

    for j in 0..6 {
        if j == current_idx { continue; }
        if s.team[j].current_hp == 0 { continue; }
        if s.team[j].species_id == 0 { continue; }
        list.push(ACTION_SWITCH_0 + j as u8);
    }
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
}
