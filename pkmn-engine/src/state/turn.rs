//! Turn executor: the single function the MCTS loop calls to advance the game.
//!
//! Takes two player actions (chosen simultaneously), resolves turn order,
//! executes both actions, runs end-of-turn, checks for faints, and updates
//! the phase.
//!
//! Zero heap allocations.  All integer math.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag, MoveCategory};
use crate::state::accessors::*;
use crate::state::mutations::*;
use crate::state::switch::{perform_switch, perform_switch_forced, perform_double_switch};
use crate::state::end_of_turn::end_of_turn;
use crate::state::move_exec::execute_move;
use crate::data::moves::MoveFlags;
use crate::data::types::Type;

/// Decoded action. Move variant carries the resolved move_id so we
/// never call `effective_moves` twice for the same action.
#[derive(Clone, Copy, Debug)]
pub enum ActionKind {
    Move { slot: u8, move_id: u16 },
    Switch { target: u8 },
    Tera { move_id: u16 },
    Struggle,
}

/// Decode a `u8` action, resolving the move_id eagerly.
#[inline]
fn decode_action(state: &BattleState, side: usize, action: u8) -> ActionKind {
    match action {
        0..=3 => {
            let move_id = effective_moves(state, side)[action as usize];
            // If the only legal action is Struggle (e.g. choice-locked move
            // got Disabled, or all PP exhausted), redirect this move action.
            // Showdown enforces this at the validation layer.
            if crate::state::legal_moves::must_struggle(state, side) {
                return ActionKind::Struggle;
            }
            ActionKind::Move { slot: action, move_id }
        }
        4..=9 => ActionKind::Switch { target: action - 4 },
        ACTION_TERA => {
            // MCTS simplification: tera is combined with move 0
            let move_id = effective_moves(state, side)[0];
            if crate::state::legal_moves::must_struggle(state, side) {
                return ActionKind::Struggle;
            }
            ActionKind::Tera { move_id }
        }
        _ => ActionKind::Struggle,
    }
}

#[derive(Clone, Copy)]
struct OrderedAction {
    side: usize,
    action: ActionKind,
}

/// Compute effective speed for turn order comparison.
#[inline]
pub(crate) fn resolve_speed(state: &BattleState, side: usize) -> u32 {
    let mon = state.active_mon(side);
    let ability = effective_ability(state, side);
    let mut speed = boosted_stat(
        effective_stat(state, side, SPE),
        state.sides[side].active.boosts[SPE],
    ) as u32;

    if mon.status == STATUS_PARALYSIS && ability != data_bridge::ABILITY_QUICK_FEET {
        speed /= 2;
    }

    if state.field.magic_room_turns() == 0 {
        let item = data_bridge::item(mon.item_id);
        if item.has(ItemFlag::CHOICE_SPE) {
            speed = speed * 3 / 2;
        }
        if item.has(ItemFlag::HALF_SPEED) {
            speed /= 2;
        }
    }

    if state.sides[side].active.has_volatile(VOL_UNBURDEN) {
        speed *= 2;
    }

    if state.sides[side].side_conditions.tailwind_turns > 0 {
        speed *= 2;
    }

    let weather = effective_weather_for(state, side);
    match ability {
        data_bridge::ABILITY_CHLOROPHYLL
            if matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN)
            => { speed *= 2; }
        data_bridge::ABILITY_SWIFT_SWIM
            if matches!(weather, WEATHER_RAIN | WEATHER_HEAVY_RAIN)
            => { speed *= 2; }
        data_bridge::ABILITY_SAND_RUSH if weather == WEATHER_SAND
            => { speed *= 2; }
        data_bridge::ABILITY_SLUSH_RUSH if weather == WEATHER_SNOW
            => { speed *= 2; }
        data_bridge::ABILITY_SURGE_SURFER if state.field.terrain == TERRAIN_ELECTRIC
            => { speed *= 2; }
        data_bridge::ABILITY_SLOW_START
            if state.sides[side].active.turns_active < 5
            => { speed /= 2; }
        data_bridge::ABILITY_QUICK_FEET if mon.status != STATUS_NONE
            => { speed = speed * 3 / 2; }
        _ => {}
    }

    // Protosynthesis/Quark Drive Spe boost (suppressed by Neutralizing Gas)
    let paradox_stat = state.sides[side].active.paradox_stat();
    if paradox_stat == (SPE as u8 + 1) && !state.sides[side].active.has_volatile(VOL_ABILITY_SUPPRESSED) {
        speed = speed * 3 / 2; // 1.5× for Spe
    }

    speed
}

#[inline(always)]
fn action_priority(state: &BattleState, side: usize, action: &ActionKind) -> i8 {
    match action {
        ActionKind::Switch { .. } => 7,
        ActionKind::Struggle => 0,
        ActionKind::Tera { move_id } | ActionKind::Move { move_id, .. } => {
            if *move_id == 0 { return 0; }
            // The dispatched move_id is the chooser's nominal pick, but a mon
            // locked into a multi-turn move (Outrage/Thrash/Petal Dance),
            // mid-charge (Fly/Dig turn 2 → recalls last_move), or encored will
            // execute a DIFFERENT move whose priority governs ordering. Mirror
            // execute_move's override so a slow locked mon doesn't borrow a
            // priority bracket from the move it can't actually pick this turn.
            let active = &state.sides[side].active;
            let eff_move_id = if active.has_volatile(VOL_CHARGING)
                || active.has_volatile(VOL_MOVE_LOCKED)
            {
                active.last_move
            } else if active.encore_turns > 0
                && active.encore_move != 0
                && *move_id != active.encore_move
            {
                active.encore_move
            } else {
                *move_id
            };
            if eff_move_id == 0 { return 0; }
            let md = data_bridge::move_hot(eff_move_id);
            let mut pri = md.priority;
            let ability = effective_ability(state, side);

            // Grassy Glide: +1 priority in Grassy Terrain if user is grounded
            if md.effect == data_bridge::MoveEffect::GrassyGlide
                && state.field.terrain == TERRAIN_GRASSY
                && is_grounded(state, side)
            {
                pri += 1;
            }

            // Prankster: +1 to Status moves
            if ability == data_bridge::ABILITY_PRANKSTER
                && md.category == MoveCategory::Status
            {
                pri += 1;
            }

            // Gale Wings: +1 to Flying moves at full HP
            if ability == data_bridge::ABILITY_GALE_WINGS
                && md.move_type == Type::Flying
            {
                let mon = state.active_mon(side);
                if mon.current_hp == mon.max_hp {
                    pri += 1;
                }
            }

            // Triage: +3 to healing moves
            if ability == data_bridge::ABILITY_TRIAGE
                && md.flags & MoveFlags::HEAL != 0
            {
                pri += 3;
            }

            // Stall / Mycelium Might: effectively go last (handled via fractional priority)
            // We model this as -1 priority shift for applicable moves
            if ability == data_bridge::ABILITY_STALL {
                pri -= 1;
            }
            if ability == data_bridge::ABILITY_MYCELIUM_MIGHT
                && md.category == MoveCategory::Status
            {
                pri -= 1;
            }

            pri
        }
    }
}

/// Result of `resolve_order`: the move ordering plus the side (if any) whose
/// Custap Berry fired this turn — the caller consumes it before move execution.
struct OrderResult {
    first: OrderedAction,
    second: OrderedAction,
    custap_side: Option<usize>,
}

#[inline]
fn resolve_order(
    state: &BattleState,
    side_a: usize, act_a: ActionKind,
    side_b: usize, act_b: ActionKind,
    rng: &mut impl FnMut(u32) -> u32,
) -> OrderResult {
    let a = OrderedAction { side: side_a, action: act_a };
    let b = OrderedAction { side: side_b, action: act_b };
    let no_custap = |first, second| OrderResult { first, second, custap_side: None };

    let pri_a = action_priority(state, side_a, &act_a);
    let pri_b = action_priority(state, side_b, &act_b);

    if pri_a != pri_b {
        return if pri_a > pri_b { no_custap(a, b) } else { no_custap(b, a) };
    }

    // Same-turn double switch: Showdown sorts the two `switch` actions by the
    // OUTGOING active's speed (the switch action's pokemon is the mon leaving),
    // and each switch's runSwitch fires its switch-in ability immediately —
    // before the slower side has switched. So the faster-outgoing side's
    // Intimidate targets the foe present at that instant, not the foe's
    // replacement. Order by current-active speed here; perform_double_switch
    // resolves each side fully in this order.
    if matches!(act_a, ActionKind::Switch { .. }) && matches!(act_b, ActionKind::Switch { .. }) {
        let spd_a = resolve_speed(state, side_a);
        let spd_b = resolve_speed(state, side_b);
        let trick_room = state.field.trick_room_turns > 0;
        let a_faster = if spd_a != spd_b {
            if trick_room { spd_a < spd_b } else { spd_a > spd_b }
        } else {
            rng(2) == 0
        };
        return if a_faster { no_custap(a, b) } else { no_custap(b, a) };
    }

    // Fractional-priority items bump a same/lower-priority move first within its
    // integer bracket when both sides are at the same priority and it's <= 0.
    // Magic Room suppresses item effects. A holder carries only one item, so the
    // Quick-Claw and Custap tests are mutually exclusive per side.
    let frac_eligible = pri_a <= 0 && state.field.magic_room_turns() == 0;

    // Quick Claw: 20% chance (mirrors items.ts:quickclaw — `priority <= 0 && randomChance(1,5)`).
    let a_qc = frac_eligible
        && matches!(act_a, ActionKind::Move { .. } | ActionKind::Tera { .. })
        && data_bridge::item(state.active_mon(side_a).item_id).has(data_bridge::ItemFlag::QUICK_CLAW)
        && rng(5) == 0;
    let b_qc = frac_eligible
        && matches!(act_b, ActionKind::Move { .. } | ActionKind::Tera { .. })
        && data_bridge::item(state.active_mon(side_b).item_id).has(data_bridge::ItemFlag::QUICK_CLAW)
        && rng(5) == 0;
    if a_qc && !b_qc { return no_custap(a, b); }
    if b_qc && !a_qc { return no_custap(b, a); }

    // Custap Berry: deterministic bump when the holder is at <= 25% HP (mirrors
    // items.ts:custapberry — `priority <= 0 && hp <= maxhp/4`; ½-with-Gluttony
    // not implemented). hp*4 <= max_hp avoids a divide. Consumed at the call site
    // via consume_berry so Harvest/Unburden see it.
    let a_custap = frac_eligible
        && matches!(act_a, ActionKind::Move { .. } | ActionKind::Tera { .. })
        && custap_ready(state.active_mon(side_a));
    let b_custap = frac_eligible
        && matches!(act_b, ActionKind::Move { .. } | ActionKind::Tera { .. })
        && custap_ready(state.active_mon(side_b));
    if a_custap && !b_custap {
        return OrderResult { first: a, second: b, custap_side: Some(side_a) };
    }
    if b_custap && !a_custap {
        return OrderResult { first: b, second: a, custap_side: Some(side_b) };
    }

    let a_quick = matches!(act_a, ActionKind::Move { move_id, .. } | ActionKind::Tera { move_id } if {
        let md = data_bridge::move_hot(move_id);
        md.category != MoveCategory::Status
    }) && effective_ability(state, side_a) == data_bridge::ABILITY_QUICK_DRAW
        && rng(10) < 3;
    let b_quick = matches!(act_b, ActionKind::Move { move_id, .. } | ActionKind::Tera { move_id } if {
        let md = data_bridge::move_hot(move_id);
        md.category != MoveCategory::Status
    }) && effective_ability(state, side_b) == data_bridge::ABILITY_QUICK_DRAW
        && rng(10) < 3;

    if a_quick && !b_quick { return no_custap(a, b); }
    if b_quick && !a_quick { return no_custap(b, a); }

    // Fractional-priority items (Lagging Tail / Full Incense, items.ts
    // onFractionalPriority -0.1): the holder's move goes LAST within its integer
    // bracket — ahead of the speed compare and regardless of Trick Room. Both
    // holding one cancels → fall through to speed. Item effect ⇒ Magic Room
    // suppresses it. Full Incense is isNonstandard:Past, so only Lagging Tail matches.
    if state.field.magic_room_turns() == 0 {
        let a_lag = matches!(act_a, ActionKind::Move { .. } | ActionKind::Tera { .. })
            && state.active_mon(side_a).item_id == data_bridge::ITEM_LAGGING_TAIL;
        let b_lag = matches!(act_b, ActionKind::Move { .. } | ActionKind::Tera { .. })
            && state.active_mon(side_b).item_id == data_bridge::ITEM_LAGGING_TAIL;
        if a_lag && !b_lag { return no_custap(b, a); }
        if b_lag && !a_lag { return no_custap(a, b); }
    }

    let spd_a = resolve_speed(state, side_a);
    let spd_b = resolve_speed(state, side_b);
    let trick_room = state.field.trick_room_turns > 0;

    let a_faster = if spd_a != spd_b {
        if trick_room { spd_a < spd_b } else { spd_a > spd_b }
    } else {
        rng(2) == 0
    };

    if a_faster { no_custap(a, b) } else { no_custap(b, a) }
}

#[inline]
fn custap_ready(mon: &MonSlot) -> bool {
    !mon.is_fainted()
        && data_bridge::item(mon.item_id).has(data_bridge::ItemFlag::CUSTAP)
        && (mon.current_hp as u32) * 4 <= mon.max_hp as u32
}

#[inline]
fn execute_action(
    state: &mut BattleState,
    teams: &TeamData,
    side: usize,
    action: &ActionKind,
    rng: &mut impl FnMut(u32) -> u32,
) {
    match *action {
        ActionKind::Switch { target } => {
            // perform_switch honors trapping abilities: returns false if user is
            // trapped (Magnet Pull / Arena Trap / Shadow Tag / binding volatiles).
            // The turn then proceeds without the switch — matches Showdown's
            // validation-layer rejection (though Showdown aborts the whole turn,
            // the engine here silently no-ops to keep the MCTS loop deterministic).
            let _ = perform_switch(state, teams, side, target as usize);
        }
        ActionKind::Move { slot, move_id } => {
            execute_move(state, teams, side, move_id, slot, rng);
            state.sides[side].set_acted_since_switch_in();
        }
        ActionKind::Tera { move_id } => {
            apply_tera(state, teams, side);
            execute_move(state, teams, side, move_id, 0, rng);
            state.sides[side].set_acted_since_switch_in();
        }
        ActionKind::Struggle => {
            execute_move(state, teams, side, 0, 0, rng);
            state.sides[side].set_acted_since_switch_in();
        }
    }
}

/// Apply Terastallization to the active Pokémon.
#[inline]
fn apply_tera(state: &mut BattleState, teams: &TeamData, side: usize) {
    let slot = state.sides[side].active_index as usize;
    // Showdown's `chooseMove` rejects terastallize when the user is locked
    // into a multi-turn move (Fly/Dig/Dive/etc. charge state, Outrage/Petal
    // Dance lock, post-Hyper Beam recharge). Mirror that here — the action
    // queue still resolves to Tera in the engine because decode_action runs
    // before the lock is observed, but applying tera under those volatiles
    // diverges the snapshot vs Showdown.
    {
        let active = &state.sides[side].active;
        if active.has_volatile(VOL_CHARGING)
            || active.has_volatile(VOL_RECHARGING)
            || active.has_volatile(VOL_MOVE_LOCKED)
            || active.has_volatile(VOL_SEMI_INVULNERABLE)
        {
            return;
        }
    }
    let mon = &mut state.sides[side].team[slot];
    if mon.is_fainted() || mon.is_terastallized() { return; }
    // Ogerpon softlocks (silent no-op) in Showdown unless its Tera type is one
    // of Fire/Grass/Rock/Water (battle-actions.ts:1926 — the set, not the forme
    // mask: a Wellspring Ogerpon may Tera into Fire/Grass/Rock too). tera_type
    // is engine-order here (team_builder converts on ingest).
    if matches!(mon.species_id, 1017 | 1300 | 1302 | 1305)
        && !matches!(
            mon.tera_type,
            x if x == Type::Fire as u8
                || x == Type::Grass as u8
                || x == Type::Rock as u8
                || x == Type::Water as u8
        )
    {
        return;
    }
    mon.flags |= MON_FLAG_TERASTALLIZED;
    state.sides[side]._padding[0] |= 1;

    // Ogerpon Tera forme-change overwrites the held ability with the
    // mask-specific Embody Aspect (Showdown `formeChange(..., isPermanent=true)`
    // at sim/pokemon.ts:1467 calls setAbility with the Tera-forme abilities[0]).
    // Stats are identical between Ogerpon's pre- and post-Tera formes, so no
    // override_stats install is needed — only the ability swap, which the
    // Embody Aspect arm below then sees.
    match mon.species_id {
        1017 => { mon.ability_id = data_bridge::ABILITY_EMBODY_ASPECT_TEAL; }
        1300 => { mon.ability_id = data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE; }
        1302 => { mon.ability_id = data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME; }
        1305 => { mon.ability_id = data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING; }
        _ => {}
    }

    // Embody Aspect: boost stat on Terastallization
    let ability = effective_ability(state, side);
    match ability {
        data_bridge::ABILITY_EMBODY_ASPECT_TEAL => { apply_boost(state, side, SPE, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING => { apply_boost(state, side, SPD, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_HEARTHFLAME => { apply_boost(state, side, ATK, 1); }
        data_bridge::ABILITY_EMBODY_ASPECT_CORNERSTONE => { apply_boost(state, side, DEF, 1); }
        _ => {}
    }

    // Terapagos-Terastal -> Teraform Zero (Terapagos-Stellar): reads the just-set
    // tera flag, so must run after MON_FLAG_TERASTALLIZED is committed above.
    crate::state::forme::check_tera_shift(state, teams, side);
}

fn either_side_wiped(state: &BattleState) -> bool {
    let p1_alive = (0..6).any(|i| {
        let m = &state.sides[0].team[i];
        m.species_id != 0 && m.current_hp > 0
    });
    let p2_alive = (0..6).any(|i| {
        let m = &state.sides[1].team[i];
        m.species_id != 0 && m.current_hp > 0
    });
    !p1_alive || !p2_alive
}

fn faint_sweep(state: &mut BattleState) {
    let p1_fainted = state.active_mon(0).is_fainted();
    let p2_fainted = state.active_mon(1).is_fainted();

    // Fainted mon's active state (boosts) clears. In Showdown the mon leaves
    // the field and its battle-scoped state ceases to apply. We clear boosts
    // explicitly here so comparisons against Showdown match when a fainted mon
    // has had stat drops (e.g. BUG-P5-M-201 Rough Skin KO + prior Def drop).
    for &(fainted, side) in &[(p1_fainted, 0usize), (p2_fainted, 1usize)] {
        if !fainted { continue; }
        for stat_idx in 0..7 {
            if state.sides[side].active.boosts[stat_idx] != 0 {
                state.sides[side].active.boosts[stat_idx] = 0;
            }
        }
        // Showdown's checkFainted overwrites status with 'fnt' (battle.ts:2523),
        // which compare_results normalizes to STATUS_NONE. Mirror by clearing.
        let slot = state.sides[side].active_index as usize;
        clear_status(state, side, slot);
        // Showdown's clearVolatile (pokemon.ts:1505) on faint wipes all volatiles.
        // Zero the per-mon counters the comparator surfaces (mirrors switch-out's active.zero).
        let active = &mut state.sides[side].active;
        active.confusion_turns = 0;
        active.taunt_turns = 0;
        active.encore_turns = 0;
        active.encore_move = 0;
    }

    let p1_alive = (0..6).any(|i| {
        let m = &state.sides[0].team[i];
        m.species_id != 0 && m.current_hp > 0
    });
    let p2_alive = (0..6).any(|i| {
        let m = &state.sides[1].team[i];
        m.species_id != 0 && m.current_hp > 0
    });

    if !p1_alive || !p2_alive {
        set_phase(state, PHASE_GAME_OVER);
        return;
    }

    let p1_must = state.sides[0].active.has_volatile(VOL_MUST_SWITCH) && !p1_fainted;
    let p2_must = state.sides[1].active.has_volatile(VOL_MUST_SWITCH) && !p2_fainted;

    if p1_must { clear_volatile(state, 0, VOL_MUST_SWITCH); }
    if p2_must { clear_volatile(state, 1, VOL_MUST_SWITCH); }

    let phase = match (p1_fainted || p1_must, p2_fainted || p2_must) {
        (true, true)   => PHASE_SWITCH_BOTH,
        (true, false)  => PHASE_SWITCH_P1,
        (false, true)  => PHASE_SWITCH_P2,
        (false, false) => PHASE_ACTIONS,
    };
    set_phase(state, phase);
}

pub fn execute_turn(
    state: &mut BattleState,
    teams: &TeamData,
    action_p1: u8,
    action_p2: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    // Showdown fires onUpdate (which runs berry auto-cure / auto-heal) repeatedly,
    // including at turn start. Mirror that here so that status-cure berries and
    // healing berries trigger on any state set outside a move (e.g. preset HP /
    // preset status via state_overrides) before actions resolve.
    // Fast path: skip the call unless the active mon could possibly activate a
    // berry (must be holding an item and have either a status to cure or HP
    // loss to heal). This keeps the hot path tight for MCTS rollouts where
    // most turns have no pending berry trigger.
    // Only run at turn 0: mirrors Showdown's onUpdate firing after initial
    // state is materialized. On subsequent turns, berries already trigger from
    // their natural event hooks (after damage, after status application, after
    // item-swap, etc.) — no need to re-scan every turn. This keeps the MCTS
    // hot path free of per-turn berry scans.
    if state.field.turn == 0 {
        for side in 0..2 {
            let slot = state.sides[side].active_index as usize;
            let mon = &state.sides[side].team[slot];
            if mon.item_id == 0 || mon.is_fainted() { continue; }
            if mon.status == STATUS_NONE && mon.current_hp >= mon.max_hp { continue; }
            if !data_bridge::item(mon.item_id).has(ItemFlag::IS_BERRY) { continue; }
            crate::state::move_exec::check_berry_activation(state, teams, side, slot, rng);
        }
    }

    let act0 = decode_action(state, 0, action_p1);
    let act1 = decode_action(state, 1, action_p2);

    // Apply Terastallization BEFORE any move resolves this turn. Showdown commits
    // the Tera flag at turn start so defensive type effectiveness uses the Tera
    // type even if the opponent moves first. apply_tera() is idempotent, so the
    // ActionKind::Tera arm in execute_action will no-op on the second call.
    if matches!(act0, ActionKind::Tera { .. }) { apply_tera(state, teams, 0); }
    if matches!(act1, ActionKind::Tera { .. }) { apply_tera(state, teams, 1); }

    let OrderResult { first, second, custap_side } = resolve_order(state, 0, act0, 1, act1, rng);

    // Custap Berry was eaten to win the bracket — consume it (routes through
    // set_last_consumed_berry for correct Harvest / Unburden) before moves run.
    if let Some(side) = custap_side {
        let slot = state.sides[side].active_index as usize;
        crate::state::move_exec::consume_berry(state, side, slot);
    }

    let second_raw = if second.side == 0 { action_p1 } else { action_p2 };

    // Publish each side's raw action so move-execute logic can introspect the
    // opponent's queued action (Sucker Punch onTry).
    state.pending_actions[0] = action_p1;
    state.pending_actions[1] = action_p2;

    // Double-switch: resolve each side fully (switch-out + install + ability +
    // item) in outgoing-speed order. The faster-switching side's switch-in
    // ability fires before the slower side switches, matching Showdown's
    // per-switch runSwitch (queued via insertChoice, order 101 < switch 103).
    if let (ActionKind::Switch { target: t_first }, ActionKind::Switch { target: t_second })
        = (first.action, second.action)
    {
        perform_double_switch(
            state, teams,
            first.side, t_first as usize,
            second.side, t_second as usize,
            rng,
        );
        state.pending_actions[0] = 0xFF;
        state.pending_actions[1] = 0xFF;
    } else {
        execute_action(state, teams, first.side, &first.action, rng);

        // First mover has resolved — invalidate their entry so the second mover's
        // Sucker Punch sees "defender already moved".
        state.pending_actions[first.side] = 0xFF;

        // Showdown runs checkWin after every action; if move 1 wiped a side the
        // battle ends before move 2 (and before residuals). Mirror that — a
        // self-KO that wipes the mover's own side must not let the opponent move.
        if either_side_wiped(state) {
            faint_sweep(state);
            return;
        }

        // A first-mover pivot (U-turn / Volt Switch / Parting Shot / Baton Pass /
        // Teleport) raises a mid-turn switch request: Showdown's turnLoop returns
        // on `this.requestState` before the slower mon's queued action ever pops
        // (sim/battle.ts), so the second mover does not move and residuals do not
        // run until the replacement lands. Mirror that — skip the second action
        // when the first mover set VOL_MUST_SWITCH on its own side.
        let first_pivot = state.sides[first.side].active.has_volatile(VOL_MUST_SWITCH);

        // Showdown runs the second mover's action and EOT residuals before any
        // forced-replacement prompt: a mid-turn faint after move 1 does NOT block
        // the survivor from moving or from receiving end-of-turn ticks (burn /
        // toxic counter / weather chip / Leftovers / etc.). Skip the second
        // action only when the second mover itself is fainted; execute_move and
        // execute_action are no-ops on fainted attackers, but a queued Switch
        // would still resolve, so gate explicitly.
        if !first_pivot && !state.active_mon(second.side).is_fainted() {
            execute_action(state, teams, second.side, &second.action, rng);
        }
    }

    // Pivot moves: transition to switch phase before end-of-turn. A pending
    // pivot supersedes residuals (Showdown defers residuals to after the
    // pivot replacement lands).
    let p1_pivot = state.sides[0].active.has_volatile(VOL_MUST_SWITCH);
    let p2_pivot = state.sides[1].active.has_volatile(VOL_MUST_SWITCH);
    if p1_pivot || p2_pivot {
        faint_sweep(state);
        return;
    }

    // Showdown's turnLoop returns on `this.ended` before the queued residual
    // action runs, so EOT never fires once a side is wiped by the move phase.
    // Without this the engine's unconditional EOT would poison/burn/Sticky-Barb
    // the move-phase survivor into a spurious tie.
    if either_side_wiped(state) {
        faint_sweep(state);
        return;
    }

    end_of_turn(state, teams, &mut crate::state::BattleRng::from_closure(rng));

    // After residuals: if anyone fainted (mid-turn or from EOT), pause for
    // forced replacement before next turn. Otherwise the turn closes cleanly.
    if state.active_mon(0).is_fainted() || state.active_mon(1).is_fainted() {
        state.set_turn_resume(SUBPHASE_AFTER_MOVE2, 0, 0);
        faint_sweep(state);
        return;
    }

    faint_sweep(state);
    state.clear_turn_resume();

    // second_raw was only meaningful for the deprecated SUBPHASE_AFTER_MOVE1
    // resume path; bind explicitly to silence unused-variable warnings.
    let _ = second_raw;
}

pub fn execute_switch_turn(
    state: &mut BattleState,
    teams: &TeamData,
    action_p1: u8,
    action_p2: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    let phase = state.phase;

    if phase == PHASE_SWITCH_P1 || phase == PHASE_SWITCH_BOTH {
        if let ActionKind::Switch { target } = decode_action(state, 0, action_p1) {
            // Forced replacement (faint) bypasses trapping abilities.
            perform_switch_forced(state, teams, 0, target as usize);
        }
    }
    if phase == PHASE_SWITCH_P2 || phase == PHASE_SWITCH_BOTH {
        if let ActionKind::Switch { target } = decode_action(state, 1, action_p2) {
            perform_switch_forced(state, teams, 1, target as usize);
        }
    }

    let subphase = state.turn_subphase();

    match subphase {
        SUBPHASE_AFTER_MOVE1 => {
            faint_sweep(state);
            if state.phase != PHASE_ACTIONS { return; }

            let second_side = state.second_mover_side();
            let second_raw = state.pending_action();

            let second_replaced = match phase {
                PHASE_SWITCH_P1   => second_side == 0,
                PHASE_SWITCH_P2   => second_side == 1,
                PHASE_SWITCH_BOTH => true,
                _ => false,
            };

            if !second_replaced && !state.active_mon(second_side).is_fainted() {
                let second_action = decode_action(state, second_side, second_raw);
                execute_action(state, teams, second_side, &second_action, rng);
            }

            if state.active_mon(0).is_fainted() || state.active_mon(1).is_fainted() {
                state.set_turn_resume(SUBPHASE_AFTER_MOVE2, 0, 0);
                faint_sweep(state);
                return;
            }

            let p1_pivot = state.sides[0].active.has_volatile(VOL_MUST_SWITCH);
            let p2_pivot = state.sides[1].active.has_volatile(VOL_MUST_SWITCH);
            if p1_pivot || p2_pivot {
                state.clear_turn_resume();
                faint_sweep(state);
                return;
            }

            end_of_turn(state, teams, &mut crate::state::BattleRng::from_closure(rng));
            faint_sweep(state);
            state.clear_turn_resume();
        }

        SUBPHASE_AFTER_MOVE2 => {
            // EOT already ran in execute_turn before the replacement pause;
            // here we only need to clear the resume marker after the switch-in.
            faint_sweep(state);
            if state.phase != PHASE_ACTIONS { return; }
            state.clear_turn_resume();
        }

        _ => {
            faint_sweep(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locked_move_priority_uses_actual_move() {
        // A mon locked into Thrash (priority 0) whose chooser picked Helping
        // Hand (priority +5) must be ordered by Thrash's priority, not the
        // borrowed +5 of the move it cannot actually select this turn.
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot { species_id: 25, current_hp: 100, max_hp: 100, ..Default::default() };
        state.sides[0].active.last_move = 37; // Thrash
        state.sides[0].active.set_volatile(VOL_MOVE_LOCKED);
        // Chosen action: Helping Hand (270, +5) — but the lock forces Thrash.
        let locked = ActionKind::Move { slot: 0, move_id: 270 };
        assert_eq!(action_priority(&state, 0, &locked), 0);

        // Without the lock, the nominal +5 move governs.
        state.sides[0].active = ActiveMon::default();
        assert_eq!(action_priority(&state, 0, &locked), 5);
    }

    #[test]
    fn test_first_mover_pivot_suppresses_second_mover_queued_move() {
        use crate::data::MOVE_U_TURN;
        let mut state = BattleState::default();
        state.phase = PHASE_ACTIONS;
        // p1 faster, U-turns with a live bench mon → mid-turn switch request.
        state.sides[0].team[0] = MonSlot {
            species_id: 25, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 150],
            moves: [MOVE_U_TURN as u16, 0, 0, 0], pp: [24, 0, 0, 0],
            level: 100, ..Default::default()
        };
        state.sides[0].team[1] = MonSlot {
            species_id: 6, current_hp: 200, max_hp: 200,
            stats: [100, 100, 100, 100, 80], level: 100, ..Default::default()
        };
        // p2 slower, with a damaging move queued in slot 0.
        state.sides[1].team[0] = MonSlot {
            species_id: 50, current_hp: 300, max_hp: 300,
            stats: [150, 100, 150, 100, 80],
            moves: [1, 0, 0, 0], pp: [24, 0, 0, 0],
            level: 100, ..Default::default()
        };
        let p1_hp_before = state.sides[0].team[0].current_hp;
        execute_turn(&mut state, &TeamData::default(), 0, 0, &mut fixed_rng(0));
        // Showdown halts at the switch request: the slower p2 mon never fires its
        // queued move → its PP is untouched and the faster (now pivot-pending) mon
        // takes no further damage from a second action.
        assert_eq!(state.sides[1].team[0].pp[0], 24, "p2 queued move must not consume PP");
        assert_eq!(state.sides[0].team[0].current_hp, p1_hp_before, "p2 queued move must not hit p1");
    }

    fn fixed_rng(v: u32) -> impl FnMut(u32) -> u32 {
        move |_| v
    }

    #[test]
    fn test_ogerpon_illegal_tera_type_no_ops() {
        // Ogerpon-Wellspring (1305) may only Tera into Fire/Grass/Rock/Water.
        // An out-of-set Tera type (Ghost) must be a silent no-op (mirror SD).
        let mut illegal = BattleState::default();
        illegal.sides[1].team[0] = MonSlot {
            species_id: 1305, ability_id: 11, current_hp: 100, max_hp: 100,
            tera_type: Type::Ghost as u8, ..Default::default()
        };
        apply_tera(&mut illegal, &TeamData::default(), 1);
        assert!(!illegal.sides[1].team[0].is_terastallized(), "Ghost-Tera Ogerpon must not terastallize");
        assert_eq!(illegal.sides[1].team[0].ability_id, 11, "ability must stay Water Absorb");
        assert_eq!(illegal.sides[1].active.boosts[SPD], 0, "no Embody Aspect boost on illegal Tera");

        // In-set Tera (Water = the forme mask) must still go through.
        let mut legal = BattleState::default();
        legal.sides[1].team[0] = MonSlot {
            species_id: 1305, ability_id: 11, current_hp: 100, max_hp: 100,
            tera_type: Type::Water as u8, ..Default::default()
        };
        apply_tera(&mut legal, &TeamData::default(), 1);
        assert!(legal.sides[1].team[0].is_terastallized(), "Water-Tera Ogerpon must terastallize");
        assert_eq!(legal.sides[1].team[0].ability_id, data_bridge::ABILITY_EMBODY_ASPECT_WELLSPRING);
        assert_eq!(legal.sides[1].active.boosts[SPD], 1, "Embody Aspect SpD +1 on legal Tera");
    }

    #[test]
    fn test_faint_sweep_clears_boosts() {
        // Guards the faint_sweep de-thread: dropping the boost-clear write fails here.
        let mut state = BattleState::default();
        state.sides[0].team[0] = MonSlot { species_id: 25, current_hp: 0, max_hp: 200, ..Default::default() };
        state.sides[0].team[1] = MonSlot { species_id: 143, current_hp: 200, max_hp: 200, ..Default::default() };
        state.sides[0].active.boosts = [-2, -1, 0, 3, 0, 0, 0];
        state.sides[1].team[0] = MonSlot { species_id: 6, current_hp: 200, max_hp: 200, ..Default::default() };
        faint_sweep(&mut state);
        assert_eq!(state.sides[0].active.boosts, [0i8; 7]);
    }
}
