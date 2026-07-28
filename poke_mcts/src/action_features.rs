use std::hint::black_box;

use pkmn_engine::data::types::{dual_type_effectiveness, Type};
use pkmn_engine::state::calc::calc_damage;
use pkmn_engine::state::data_bridge::{self, ItemFlag, MoveCategory, MoveData, MoveEffect};
use pkmn_engine::state::*;

use crate::policies::median_roll;

pub const ACTION_DENSE_DIM: usize = 8;
const ACTION_SPACE: usize = 14;

// Shed Tail and Chilly Reception carry no self-switch MoveEffect in the engine's move table.
const MOVE_SHED_TAIL: u16 = 880;
const MOVE_CHILLY_RECEPTION: u16 = 881;

const TYPE_BY_ID: [Type; 19] = [
    Type::Normal, Type::Fire, Type::Water, Type::Electric, Type::Grass, Type::Ice,
    Type::Fighting, Type::Poison, Type::Ground, Type::Flying, Type::Psychic, Type::Bug,
    Type::Rock, Type::Ghost, Type::Dragon, Type::Dark, Type::Steel, Type::Fairy, Type::Stellar,
];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ActionDenseMode {
    Full,
    Priced,
}

pub fn action_features(state: &BattleState, side: usize) -> [[f32; ACTION_DENSE_DIM]; ACTION_SPACE] {
    action_features_with(state, side, ActionDenseMode::Full)
}

pub fn action_features_with(
    state: &BattleState,
    side: usize,
    mode: ActionDenseMode,
) -> [[f32; ACTION_DENSE_DIM]; ACTION_SPACE] {
    let mut out = [[0.0f32; ACTION_DENSE_DIM]; ACTION_SPACE];
    if mode == ActionDenseMode::Priced {
        black_box(priced_pass(state, side));
        return out;
    }

    let mut legal = [false; ACTION_SPACE];
    for &a in legal_actions(state, side).as_slice() {
        if (a as usize) < ACTION_SPACE {
            legal[a as usize] = true;
        }
    }

    let moves = effective_moves(state, side);

    for slot in 0..4 {
        if legal[slot] {
            out[slot] = move_row(state, side, moves[slot]);
        }
    }

    if (ACTION_TERA_0 as usize..ACTION_SPACE).any(|b| legal[b]) {
        let mut tera = *state;
        tera.active_mon_mut(side).flags |= MON_FLAG_TERASTALLIZED;
        for slot in 0..4 {
            let byte = ACTION_TERA_0 as usize + slot;
            if legal[byte] {
                out[byte] = move_row(&tera, side, moves[slot]);
            }
        }
    }

    for slot in 0..6 {
        let byte = ACTION_SWITCH_0 as usize + slot;
        if legal[byte] {
            out[byte] = switch_row(state, side, slot);
        }
    }

    out
}

fn move_row(state: &BattleState, side: usize, move_id: u16) -> [f32; ACTION_DENSE_DIM] {
    if move_id == 0 {
        return [0.0; ACTION_DENSE_DIM];
    }
    let opp = 1 - side;
    let md = data_bridge::move_hot(move_id);
    let their_hp = state.active_mon(opp).current_hp;
    let res = calc_damage(state, side, move_id, 100, &mut median_roll);
    let is_status = md.category == MoveCategory::Status;
    let eff = if is_status {
        let (t1, t2) = battle_types(state, opp);
        dual_type_effectiveness(md.move_type, to_type(t1), to_type(t2))
    } else {
        res.effectiveness
    };
    let moves_first = if md.priority > 0 {
        1.0
    } else if md.priority < 0 {
        0.0
    } else {
        speed_order(state, side)
    };
    [
        dmg_frac(res.damage, their_hp),
        ko_flag(res.damage, their_hp),
        eff_encode(eff),
        moves_first,
        (md.priority as f32 / 5.0).clamp(-1.0, 1.0),
        md.accuracy as f32 / 100.0,
        if is_status { 1.0 } else { 0.0 },
        if is_pivot(move_id, md) { 1.0 } else { 0.0 },
    ]
}

fn switch_row(state: &BattleState, side: usize, slot: usize) -> [f32; ACTION_DENSE_DIM] {
    let opp = 1 - side;
    let hypo = swap_in(state, side, slot);
    let cand = *hypo.active_mon(side);
    let their_hp = hypo.active_mon(opp).current_hp;
    let incoming = best_damage(&hypo, opp);
    let outgoing = best_damage(&hypo, side);
    [
        dmg_frac(incoming, cand.current_hp),
        ko_flag(incoming, cand.current_hp),
        dmg_frac(outgoing, their_hp),
        ko_flag(outgoing, their_hp),
        speed_order(&hypo, side),
        hazard_frac(&hypo, side),
        cand.current_hp as f32 / cand.max_hp.max(1) as f32,
        best_eff(&hypo, side),
    ]
}

// Entry abilities (Intimidate, weather/terrain setters, Download) are deliberately left unapplied.
fn swap_in(state: &BattleState, side: usize, slot: usize) -> BattleState {
    let mut hypo = *state;
    let active_slot = hypo.sides[side].active_index as usize;
    hypo.sides[side].team[active_slot] = state.sides[side].team[slot];
    hypo.sides[side].active = ActiveMon::default();
    hypo
}

fn best_damage(state: &BattleState, atk_side: usize) -> u16 {
    let mut best = 0u16;
    for &move_id in effective_moves(state, atk_side).iter() {
        if move_id == 0 {
            continue;
        }
        let dmg = calc_damage(state, atk_side, move_id, 100, &mut median_roll).damage;
        if dmg > best {
            best = dmg;
        }
    }
    best
}

fn best_eff(state: &BattleState, side: usize) -> f32 {
    let (t1, t2) = battle_types(state, 1 - side);
    let (def1, def2) = (to_type(t1), to_type(t2));
    let mut best: Option<u8> = None;
    for &move_id in effective_moves(state, side).iter() {
        if move_id == 0 {
            continue;
        }
        let eff = dual_type_effectiveness(data_bridge::move_hot(move_id).move_type, def1, def2);
        if best.map_or(true, |b| eff > b) {
            best = Some(eff);
        }
    }
    best.map_or(0.0, eff_encode)
}

// Replicates the engine's private entry-hazard arithmetic, gross of its current-HP cap;
// Toxic Spikes and Sticky Web cost no HP.
fn hazard_frac(state: &BattleState, side: usize) -> f32 {
    let slot = state.sides[side].active_index as usize;
    let mon = state.sides[side].team[slot];
    if state.field.magic_room_turns() == 0
        && data_bridge::item(mon.item_id).has(ItemFlag::HAZARD_IMMUNE)
    {
        return 0.0;
    }
    if effective_ability(state, side) == data_bridge::ABILITY_MAGIC_GUARD {
        return 0.0;
    }
    let sc = state.sides[side].side_conditions;
    let max_hp = mon.max_hp as u32;
    let mut damage = 0u32;
    if sc.hazard_flags & HAZARD_STEALTH_ROCK != 0 {
        let (t1, t2) = battle_types(state, side);
        let eff = dual_type_effectiveness(Type::Rock, to_type(t1), to_type(t2));
        damage += (max_hp * eff as u32 / 32).max(1);
    }
    if sc.spikes > 0 && is_grounded(state, side) {
        let layer = match sc.spikes {
            1 => max_hp / 8,
            2 => max_hp / 6,
            _ => max_hp / 4,
        };
        damage += layer.max(1);
    }
    (damage as f32 / max_hp.max(1) as f32).clamp(0.0, 1.0)
}

fn speed_order(state: &BattleState, side: usize) -> f32 {
    let ours = effective_speed(state, side);
    let theirs = effective_speed(state, 1 - side);
    if ours == theirs {
        return 0.5;
    }
    let first = if state.field.trick_room_turns > 0 {
        ours < theirs
    } else {
        ours > theirs
    };
    if first { 1.0 } else { 0.0 }
}

// Deliberate approximation, none of these modelled here or in m4/m5: Quick Claw, Custap, Quick Draw,
// Lagging Tail, lock/charge/Encore overrides, Prankster, Gale Wings, Triage, Grassy Glide, Stall, Mycelium Might.
fn effective_speed(state: &BattleState, side: usize) -> u32 {
    let mon = state.active_mon(side);
    let active = &state.sides[side].active;
    let ability = effective_ability(state, side);
    let mut speed = boosted_stat(effective_stat(state, side, SPE), active.boosts[SPE]) as u32;

    if mon.status == STATUS_PARALYSIS && ability != data_bridge::ABILITY_QUICK_FEET {
        speed /= 2;
    }

    if state.field.magic_room_turns() == 0 {
        let held = data_bridge::item(mon.item_id);
        if held.has(ItemFlag::CHOICE_SPE) {
            speed = speed * 3 / 2;
        }
        if held.has(ItemFlag::HALF_SPEED) {
            speed /= 2;
        }
    }

    if active.has_volatile(VOL_UNBURDEN) {
        speed *= 2;
    }
    if state.sides[side].side_conditions.tailwind_turns > 0 {
        speed *= 2;
    }

    let weather = effective_weather_for(state, side);
    match ability {
        data_bridge::ABILITY_CHLOROPHYLL if matches!(weather, WEATHER_SUN | WEATHER_HARSH_SUN) => {
            speed *= 2;
        }
        data_bridge::ABILITY_SWIFT_SWIM if matches!(weather, WEATHER_RAIN | WEATHER_HEAVY_RAIN) => {
            speed *= 2;
        }
        data_bridge::ABILITY_SAND_RUSH if weather == WEATHER_SAND => { speed *= 2; }
        data_bridge::ABILITY_SLUSH_RUSH if weather == WEATHER_SNOW => { speed *= 2; }
        data_bridge::ABILITY_SURGE_SURFER if state.field.terrain == TERRAIN_ELECTRIC => {
            speed *= 2;
        }
        data_bridge::ABILITY_SLOW_START if active.turns_active < 5 => { speed /= 2; }
        data_bridge::ABILITY_QUICK_FEET if mon.status != STATUS_NONE => {
            speed = speed * 3 / 2;
        }
        _ => {}
    }

    if active.paradox_stat() == SPE as u8 + 1 && !active.has_volatile(VOL_ABILITY_SUPPRESSED) {
        speed = speed * 3 / 2;
    }

    speed
}

#[deny(dead_code)]
fn priced_pass(state: &BattleState, side: usize) -> (u32, u32) {
    let opp = 1 - side;
    let ours = state.active_mon(side).moves;
    let theirs = state.active_mon(opp).moves;
    let mut calls = 0u32;
    let mut clones = 0u32;
    for &move_id in ours.iter() {
        calls += 1;
        black_box(calc_damage(state, side, move_id, 100, &mut median_roll).damage);
    }
    for &move_id in ours.iter() {
        calls += 1;
        black_box(calc_damage(state, side, move_id, 100, &mut median_roll).damage);
    }
    for _ in 0..6 {
        clones += 1;
        let hypo = *state;
        for &move_id in theirs.iter() {
            calls += 1;
            black_box(calc_damage(&hypo, opp, move_id, 100, &mut median_roll).damage);
        }
        for &move_id in ours.iter() {
            calls += 1;
            black_box(calc_damage(&hypo, side, move_id, 100, &mut median_roll).damage);
        }
        black_box(&hypo);
    }
    (calls, clones)
}

fn is_pivot(move_id: u16, md: &MoveData) -> bool {
    matches!(
        md.effect,
        MoveEffect::ForceSwitch | MoveEffect::PartingShot | MoveEffect::Teleport
    ) || move_id == MOVE_SHED_TAIL
        || move_id == MOVE_CHILLY_RECEPTION
}

fn eff_encode(code: u8) -> f32 {
    match code {
        0 => -1.5,
        1 => -1.0,
        2 => -0.5,
        8 => 0.5,
        16 => 1.0,
        _ => 0.0,
    }
}

fn dmg_frac(damage: u16, hp: u16) -> f32 {
    (damage as f32 / hp.max(1) as f32).clamp(0.0, 2.0)
}

fn ko_flag(damage: u16, hp: u16) -> f32 {
    if damage >= hp.max(1) { 1.0 } else { 0.0 }
}

fn to_type(id: u8) -> Type {
    TYPE_BY_ID[id as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{build_state, mon};
    use pkmn_engine::data::types::Type;
    use pkmn_engine::state::data_bridge;

    const M_SWORDS_DANCE: u16 = 14;
    const M_TACKLE: u16 = 33;
    const M_FLAMETHROWER: u16 = 53;
    const M_HYDRO_PUMP: u16 = 56;
    const M_SURF: u16 = 57;
    const M_ICE_BEAM: u16 = 58;
    const M_THUNDERBOLT: u16 = 85;
    const M_EARTHQUAKE: u16 = 89;
    const M_QUICK_ATTACK: u16 = 98;
    const M_TELEPORT: u16 = 100;
    const M_SPLASH: u16 = 150;
    const M_AERIAL_ACE: u16 = 332;
    const M_UTURN: u16 = 369;
    const M_FIRE_BLAST: u16 = 126;

    const S_CHARIZARD: u16 = 6;
    const S_BLASTOISE: u16 = 9;
    const S_PIKACHU: u16 = 25;
    const S_ALAKAZAM: u16 = 65;
    const S_GYARADOS: u16 = 130;
    const S_SNORLAX: u16 = 143;
    const S_KINGDRA: u16 = 230;
    const S_GARCHOMP: u16 = 445;

    const I_CHOICE_SCARF: u16 = 69;
    const I_HEAVY_DUTY_BOOTS: u16 = 715;

    const ZERO: [f32; ACTION_DENSE_DIM] = [0.0; ACTION_DENSE_DIM];

    fn held(species_id: u16, ability_id: u16, item_id: u16, moves: [u16; 4]) -> MonBuildInput {
        let mut m = mon(species_id, ability_id, moves);
        m.item_id = item_id;
        m
    }

    fn move_byte(slot: usize) -> usize { slot }
    fn switch_byte(slot: usize) -> usize { ACTION_SWITCH_0 as usize + slot }
    fn tera_byte(slot: usize) -> usize { ACTION_TERA_0 as usize + slot }

    #[test]
    fn super_effective_ko_move() {
        let (mut s, _t) = build_state(
            vec![mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, 0, 0, 0])],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        s.sides[1].team[0].current_hp = 1;
        let f = action_features(&s, 0);
        let row = f[move_byte(0)];
        assert_eq!(row[1], 1.0, "m2 ko must be 1.0 on a 1-HP target, got {}", row[1]);
        assert_eq!(row[2], 1.0, "m3 eff for Electric vs Water/Flying must be +1.0, got {}", row[2]);
        assert_eq!(row[0], 2.0, "m1 dmg_frac must clamp to 2.0, got {}", row[0]);
    }

    #[test]
    fn resisted_move_reads_negative_eff() {
        let (s, _t) = build_state(
            vec![mon(S_CHARIZARD, data_bridge::ABILITY_NONE, [M_FLAMETHROWER, 0, 0, 0])],
            vec![mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let row = action_features(&s, 0)[move_byte(0)];
        assert_eq!(row[2], -0.5, "m3 eff for Fire vs Water must be -0.5, got {}", row[2]);
        assert_eq!(row[1], 0.0, "m2 ko must be 0.0 for a resisted hit at full HP, got {}", row[1]);
    }

    #[test]
    fn tera_variant_changes_stab() {
        let (mut s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_ICE_BEAM, 0, 0, 0])],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        s.sides[0].team[0].tera_type = Type::Ice as u8;
        let f = action_features(&s, 0);
        let plain = f[move_byte(0)][0];
        let tera = f[tera_byte(0)][0];
        assert!(
            tera > plain,
            "tera Ice must add STAB to Ice Beam: tera m1 {} should exceed plain m1 {}",
            tera, plain
        );
    }

    #[test]
    fn priority_move_moves_first() {
        let (s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_QUICK_ATTACK, M_TACKLE, 0, 0])],
            vec![mon(S_ALAKAZAM, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let f = action_features(&s, 0);
        assert_eq!(f[move_byte(0)][4], 0.2, "m5 priority +1 must encode 0.2, got {}", f[move_byte(0)][4]);
        assert_eq!(f[move_byte(1)][4], 0.0, "m5 priority 0 must encode 0.0, got {}", f[move_byte(1)][4]);
        assert_eq!(f[move_byte(0)][3], 1.0, "m4 must be 1.0 for a +1 move vs a faster foe, got {}", f[move_byte(0)][3]);
        assert_eq!(f[move_byte(1)][3], 0.0, "m4 must be 0.0 for a +0 move vs a faster foe, got {}", f[move_byte(1)][3]);
    }

    #[test]
    fn pivot_move_flagged() {
        let (s, _t) = build_state(
            vec![
                mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_UTURN, M_TACKLE, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let f = action_features(&s, 0);
        assert_eq!(f[move_byte(0)][7], 1.0, "m8 must be 1.0 for U-turn, got {}", f[move_byte(0)][7]);
        assert_eq!(f[move_byte(1)][7], 0.0, "m8 must be 0.0 for Tackle, got {}", f[move_byte(1)][7]);
    }

    #[test]
    fn switch_in_eats_a_ko() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        let healthy = action_features(&s, 0)[switch_byte(1)];
        s.sides[0].team[1].current_hp = 1;
        let row = action_features(&s, 0)[switch_byte(1)];
        assert_eq!(row[1], 1.0, "s2 in_ko must be 1.0 for a 1-HP switch-in, got {}", row[1]);
        assert_eq!(healthy[1], 0.0, "s2 in_ko must be 0.0 for a full-HP switch-in, got {}", healthy[1]);
        assert_eq!(healthy[6], 1.0, "s7 must be 1.0 for a full-HP switch-in, got {}", healthy[6]);
        assert!(
            healthy[0] > 0.0 && healthy[0] < 1.0,
            "s1 must be a proper fraction for a survivable hit, got {}",
            healthy[0]
        );
    }

    #[test]
    fn switch_in_behind_stealth_rock() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_CHARIZARD, data_bridge::ABILITY_NONE, [M_FLAMETHROWER, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        let clear = action_features(&s, 0)[switch_byte(1)][5];
        s.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
        let max_hp = s.sides[0].team[1].max_hp as u32;
        let expected = (max_hp * 16 / 32).max(1) as f32 / max_hp as f32;
        let row = action_features(&s, 0)[switch_byte(1)];
        assert!(row[5] > 0.0, "s6 hazard_frac must be > 0 behind Stealth Rock, got {}", row[5]);
        assert_eq!(row[5], expected, "s6 must equal {} of max HP, got {}", expected, row[5]);
        assert_eq!(clear, 0.0, "s6 must be 0.0 with no hazards set, got {}", clear);
        s.sides[0].team[1].item_id = I_HEAVY_DUTY_BOOTS;
        let booted = action_features(&s, 0)[switch_byte(1)][5];
        assert_eq!(booted, 0.0, "s6 must be 0.0 for a Heavy-Duty Boots holder, got {}", booted);
    }

    #[test]
    fn trick_room_inverts_speed_order() {
        let (s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_ALAKAZAM, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let plain = action_features(&s, 0)[move_byte(0)][3];
        let mut tr = s;
        tr.field.trick_room_turns = 5;
        let inverted = action_features(&tr, 0)[move_byte(0)][3];
        assert_eq!(inverted, 1.0, "m4 must be 1.0 for the slower mon under Trick Room, got {}", inverted);
        assert_eq!(plain, 0.0, "m4 must be 0.0 for the slower mon without Trick Room, got {}", plain);
    }

    #[test]
    fn choice_scarf_flips_speed_order() {
        let bare = build_state(
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        ).0;
        let scarfed = build_state(
            vec![held(S_GYARADOS, data_bridge::ABILITY_NONE, I_CHOICE_SCARF, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        ).0;
        let a = action_features(&bare, 0)[move_byte(0)][3];
        let b = action_features(&scarfed, 0)[move_byte(0)][3];
        assert_eq!(a, 0.0, "m4 must be 0.0 without Choice Scarf, got {}", a);
        assert_eq!(b, 1.0, "m4 must be 1.0 with Choice Scarf, got {}", b);
    }

    #[test]
    fn paralysis_flips_speed_order_quick_feet_exempt() {
        let (s, _t) = build_state(
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let healthy = action_features(&s, 0)[move_byte(0)][3];
        let mut para = s;
        para.sides[0].team[0].status = STATUS_PARALYSIS;
        let paralyzed = action_features(&para, 0)[move_byte(0)][3];
        let mut quick = para;
        quick.sides[0].team[0].ability_id = data_bridge::ABILITY_QUICK_FEET;
        let exempt = action_features(&quick, 0)[move_byte(0)][3];
        assert_eq!(healthy, 1.0, "m4 must be 1.0 for the faster mon, got {}", healthy);
        assert_eq!(paralyzed, 0.0, "paralysis must flip m4 to 0.0, got {}", paralyzed);
        assert_eq!(exempt, 1.0, "Quick Feet must keep m4 at 1.0 while paralyzed, got {}", exempt);
    }

    #[test]
    fn empty_and_fainted_bench_slots_are_zero() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_CHARIZARD, data_bridge::ABILITY_NONE, [M_FLAMETHROWER, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        s.sides[0].team[2].current_hp = 0;
        let f = action_features(&s, 0);
        assert_ne!(f[switch_byte(1)], ZERO, "an alive bench slot must not be all-zero");
        assert_eq!(f[switch_byte(2)], ZERO, "a fainted bench slot must be all-zero, got {:?}", f[switch_byte(2)]);
        assert_eq!(f[switch_byte(3)], ZERO, "an empty bench slot must be all-zero, got {:?}", f[switch_byte(3)]);
    }

    #[test]
    fn illegal_byte_is_zero() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        s.phase = PHASE_SWITCH_P1;
        let f = action_features(&s, 0);
        assert_ne!(f[switch_byte(1)], ZERO, "a legal switch byte must not be all-zero");
        assert_eq!(f[move_byte(0)], ZERO, "a move byte is illegal in a switch phase, got {:?}", f[move_byte(0)]);
    }

    #[test]
    fn entry_abilities_not_applied() {
        let build = |ability: u16| {
            build_state(
                vec![
                    mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                    mon(S_GYARADOS, ability, [M_TACKLE, 0, 0, 0]),
                ],
                vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
            ).0
        };
        let intimidate = build(data_bridge::ABILITY_INTIMIDATE);
        let inert = build(data_bridge::ABILITY_NONE);
        let a = action_features(&intimidate, 0)[switch_byte(1)];
        let b = action_features(&inert, 0)[switch_byte(1)];
        assert_ne!(a, ZERO, "the switch block must not be all-zero");
        assert_eq!(a, b, "Intimidate must not change the switch block: {:?} vs {:?}", a, b);
    }

    #[test]
    fn mirror_is_not_a_state_symmetry() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![
                mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0]),
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
        );
        s.phase = PHASE_SWITCH_P1;
        let mirrored = action_features(&crate::features::mirror(&s), 0);
        let direct = action_features(&s, 1);
        assert_ne!(
            mirrored, direct,
            "mirror() leaves phase unremapped, so it is not a state symmetry"
        );
    }

    #[test]
    fn repeated_calls_are_identical() {
        let (s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, M_SWORDS_DANCE, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        let a = action_features(&s, 0);
        let b = action_features(&s, 0);
        assert_ne!(a[move_byte(0)], ZERO, "the fixture must produce a non-zero block");
        assert_eq!(a, b, "action_features must be deterministic");
    }

    #[test]
    fn priced_budget_is_invariant() {
        let team = |m: [u16; 4]| {
            vec![
                mon(S_GARCHOMP, data_bridge::ABILITY_NONE, m),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, m),
                mon(S_CHARIZARD, data_bridge::ABILITY_NONE, m),
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, m),
                mon(S_ALAKAZAM, data_bridge::ABILITY_NONE, m),
                mon(S_GYARADOS, data_bridge::ABILITY_NONE, m),
            ]
        };
        let four = [M_TACKLE, M_SURF, M_ICE_BEAM, M_THUNDERBOLT];
        let (actions, _t) = build_state(team(four), team(four));
        assert_eq!(priced_pass(&actions, 0), (56, 6), "6v6 PHASE_ACTIONS, side 0");
        assert_eq!(priced_pass(&actions, 1), (56, 6), "6v6 PHASE_ACTIONS, side 1");

        let mut switching = actions;
        switching.phase = PHASE_SWITCH_BOTH;
        switching.sides[0].team[0].current_hp = 0;
        switching.sides[1].team[0].current_hp = 0;
        assert_eq!(priced_pass(&switching, 0), (56, 6), "PHASE_SWITCH_BOTH");

        let mut moveless = actions;
        moveless.sides[0].team[0].moves = [0; 4];
        moveless.sides[1].team[0].moves = [0; 4];
        assert_eq!(priced_pass(&moveless, 0), (56, 6), "both actives with empty move slots");

        let (duel, _t2) = build_state(
            vec![mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, 0, 0, 0])],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        assert_eq!(priced_pass(&duel, 0), (56, 6), "1v1 with one move each");
    }

    #[test]
    fn side_argument_selects_the_decider() {
        let (s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_UTURN, 0, 0, 0])],
        );
        let p1 = action_features(&s, 0)[move_byte(0)];
        let p2 = action_features(&s, 1)[move_byte(0)];
        assert_eq!(p1[7], 0.0, "side 0 slot 0 is Tackle, m8 must be 0.0, got {}", p1[7]);
        assert_eq!(p2[7], 1.0, "side 1 slot 0 is U-turn, m8 must be 1.0, got {}", p2[7]);
        assert_eq!(p1[3], 0.0, "side 0 is the slower mon, m4 must be 0.0, got {}", p1[3]);
        assert_eq!(p2[3], 1.0, "side 1 is the faster mon, m4 must be 1.0, got {}", p2[3]);
    }

    #[test]
    fn switch_row_uses_the_hypothetical_state() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_CHARIZARD, data_bridge::ABILITY_NONE, [M_FLAMETHROWER, 0, 0, 0]),
            ],
            vec![mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0])],
        );
        let max_hp = s.sides[0].team[1].max_hp;
        s.sides[0].team[1].current_hp = max_hp / 2;
        let row = action_features(&s, 0)[switch_byte(1)];
        assert_eq!(row[6], 0.5, "s7 must read the switch-in's HP, got {}", row[6]);
        assert_eq!(row[7], -0.5, "s8 must read the switch-in's moves, got {}", row[7]);
        assert_eq!(row[4], 1.0, "s5 must compare the switch-in's speed, got {}", row[4]);
    }

    #[test]
    fn swap_in_clears_boosts_and_volatiles() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0]),
            ],
            vec![mon(S_ALAKAZAM, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        s.sides[0].active.boosts[SPE] = 6;
        s.sides[0].active.volatile_flags |= VOL_UNBURDEN;
        let row = action_features(&s, 0)[switch_byte(1)];
        assert_eq!(
            row[4], 0.0,
            "s5 must ignore the outgoing mon's boosts and volatiles, got {}",
            row[4]
        );
    }

    #[test]
    fn type_immune_move_reads_minus_one_and_a_half() {
        let (s, _t) = build_state(
            vec![mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, 0, 0, 0])],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let row = action_features(&s, 0)[move_byte(0)];
        assert_eq!(row[2], -1.5, "m3 must be -1.5 for a type-immune move, got {}", row[2]);
        assert_eq!(row[0], 0.0, "m1 must be 0.0 for a type-immune move, got {}", row[0]);
        assert_eq!(row[1], 0.0, "m2 must be 0.0 for a type-immune move, got {}", row[1]);
    }

    #[test]
    fn quarter_effective_move_reads_minus_one() {
        let (s, _t) = build_state(
            vec![mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0])],
            vec![mon(S_KINGDRA, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let row = action_features(&s, 0)[move_byte(0)];
        assert_eq!(row[2], -1.0, "m3 must be -1.0 for a 0.25x move, got {}", row[2]);
    }

    #[test]
    fn status_move_uses_the_type_chart() {
        let (s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_SWORDS_DANCE, M_TACKLE, 0, 0])],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let f = action_features(&s, 0);
        assert_eq!(f[move_byte(0)][6], 1.0, "m7 must be 1.0 for a Status move, got {}", f[move_byte(0)][6]);
        assert_eq!(f[move_byte(0)][2], 0.0, "m3 must be 0.0 for a neutral Status move, got {}", f[move_byte(0)][2]);
        assert_eq!(f[move_byte(1)][6], 0.0, "m7 must be 0.0 for a damaging move, got {}", f[move_byte(1)][6]);
    }

    #[test]
    fn ko_flag_is_inclusive_at_the_boundary() {
        let (mut s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0])],
        );
        let exact = calc_damage(&s, 0, M_TACKLE, 100, &mut median_roll).damage;
        assert!(exact > 0, "fixture guard: Tackle must deal damage, got {}", exact);
        s.sides[1].team[0].current_hp = exact;
        assert_eq!(
            calc_damage(&s, 0, M_TACKLE, 100, &mut median_roll).damage, exact,
            "fixture guard: damage must not depend on the target's remaining HP"
        );
        let at_boundary = action_features(&s, 0)[move_byte(0)][1];
        s.sides[1].team[0].current_hp = exact + 1;
        let one_short = action_features(&s, 0)[move_byte(0)][1];
        assert_eq!(at_boundary, 1.0, "m2 must be 1.0 when damage equals current HP exactly, got {}", at_boundary);
        assert_eq!(one_short, 0.0, "m2 must be 0.0 when damage is one short, got {}", one_short);
    }

    #[test]
    fn speed_tie_reads_one_half() {
        let (s, _t) = build_state(
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let row = action_features(&s, 0)[move_byte(0)];
        assert_eq!(row[3], 0.5, "m4 must be 0.5 on an exact speed tie, got {}", row[3]);
    }

    #[test]
    fn switch_damage_takes_the_best_move() {
        let (s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_CHARIZARD, data_bridge::ABILITY_NONE, [M_SPLASH, M_FLAMETHROWER, 0, 0]),
            ],
            vec![mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_SPLASH, M_TACKLE, 0, 0])],
        );
        let hypo = swap_in(&s, 0, 1);
        let cand_hp = hypo.active_mon(0).current_hp as f32;
        let their_hp = hypo.active_mon(1).current_hp as f32;
        let incoming = calc_damage(&hypo, 1, M_TACKLE, 100, &mut median_roll).damage as f32;
        let outgoing = calc_damage(&hypo, 0, M_FLAMETHROWER, 100, &mut median_roll).damage as f32;
        assert!(incoming > 0.0 && outgoing > 0.0, "fixture guard: slot-1 moves must deal damage");
        let row = action_features(&s, 0)[switch_byte(1)];
        assert_eq!(row[0], incoming / cand_hp, "s1 must take the opponent's best move, got {}", row[0]);
        assert_eq!(row[2], outgoing / their_hp, "s3 must take the switch-in's best move, got {}", row[2]);
    }

    #[test]
    fn hazard_frac_counts_spikes() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        let max_hp = s.sides[0].team[1].max_hp as u32;
        s.sides[0].side_conditions.spikes = 1;
        let one = action_features(&s, 0)[switch_byte(1)][5];
        s.sides[0].side_conditions.spikes = 3;
        let three = action_features(&s, 0)[switch_byte(1)][5];
        assert_eq!(
            one, (max_hp / 8) as f32 / max_hp as f32,
            "one Spikes layer must cost 1/8 max HP, got {}", one
        );
        assert_eq!(
            three, (max_hp / 4) as f32 / max_hp as f32,
            "three Spikes layers must cost 1/4 max HP, got {}", three
        );
    }

    #[test]
    fn hazard_frac_respects_boots_and_magic_guard() {
        let probe = |ability: u16, item: u16| {
            let (mut s, _t) = build_state(
                vec![
                    mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                    held(S_BLASTOISE, ability, item, [M_SURF, 0, 0, 0]),
                ],
                vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
            );
            s.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
            s.sides[0].side_conditions.spikes = 3;
            action_features(&s, 0)[switch_byte(1)][5]
        };
        let bare = probe(data_bridge::ABILITY_NONE, 0);
        let boots = probe(data_bridge::ABILITY_NONE, I_HEAVY_DUTY_BOOTS);
        let guard = probe(data_bridge::ABILITY_MAGIC_GUARD, 0);
        assert!(bare > 0.0, "fixture guard: a bare candidate must take hazard damage, got {}", bare);
        assert_eq!(boots, 0.0, "Heavy-Duty Boots must zero s6, got {}", boots);
        assert_eq!(guard, 0.0, "Magic Guard must zero s6, got {}", guard);
    }

    #[test]
    fn move_row_scalar_fields() {
        let (s, _t) = build_state(
            vec![
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, M_TELEPORT, M_AERIAL_ACE, M_FIRE_BLAST]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_SURF, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let f = action_features(&s, 0);
        assert_eq!(f[move_byte(0)][4], 0.0, "m5 must be 0.0 for a +0 move, got {}", f[move_byte(0)][4]);
        assert_eq!(f[move_byte(1)][4], -1.0, "m5 must clamp Teleport's -6 to -1.0, got {}", f[move_byte(1)][4]);
        assert_eq!(f[move_byte(0)][5], 1.0, "m6 must be 1.0 at 100 accuracy, got {}", f[move_byte(0)][5]);
        assert_eq!(f[move_byte(3)][5], 0.85, "m6 must be 0.85 at 85 accuracy, got {}", f[move_byte(3)][5]);
        assert_eq!(
            f[move_byte(2)][5], 0.0,
            "m6 reads the raw accuracy field, so an always-hit move reads 0.0, got {}",
            f[move_byte(2)][5]
        );
        assert_eq!(f[move_byte(1)][6], 1.0, "m7 must be 1.0 for Teleport, got {}", f[move_byte(1)][6]);
        assert_eq!(f[move_byte(2)][6], 0.0, "m7 must be 0.0 for Aerial Ace, got {}", f[move_byte(2)][6]);
        assert_eq!(f[move_byte(1)][7], 1.0, "m8 must be 1.0 for Teleport, got {}", f[move_byte(1)][7]);
        assert_eq!(f[move_byte(3)][7], 0.0, "m8 must be 0.0 for Fire Blast, got {}", f[move_byte(3)][7]);
    }

    #[test]
    fn switch_row_scalar_fields() {
        let (mut s, _t) = build_state(
            vec![
                mon(S_SNORLAX, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, 0, 0, 0]),
            ],
            vec![mon(S_GYARADOS, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0])],
        );
        let healthy = action_features(&s, 0)[switch_byte(1)];
        s.sides[1].team[0].current_hp = 1;
        let finishable = action_features(&s, 0)[switch_byte(1)];
        assert_eq!(healthy[6], 1.0, "s7 must be 1.0 for a full-HP switch-in, got {}", healthy[6]);
        assert_eq!(healthy[7], 1.0, "s8 must be +1.0 for Electric vs Water/Flying, got {}", healthy[7]);
        assert_eq!(healthy[4], 1.0, "s5 must be 1.0 for a faster switch-in, got {}", healthy[4]);
        assert_eq!(healthy[3], 0.0, "s4 must be 0.0 when the switch-in cannot KO, got {}", healthy[3]);
        assert!(
            healthy[2] > 0.0 && healthy[2] < 1.0,
            "s3 must be a proper fraction against a healthy target, got {}",
            healthy[2]
        );
        assert_eq!(finishable[3], 1.0, "s4 must be 1.0 against a 1-HP target, got {}", finishable[3]);
        assert_eq!(finishable[2], 2.0, "s3 must clamp to 2.0 against a 1-HP target, got {}", finishable[2]);
    }

    #[test]
    fn priced_mode_returns_zeros() {
        let (s, _t) = build_state(
            vec![
                mon(S_PIKACHU, data_bridge::ABILITY_NONE, [M_THUNDERBOLT, 0, 0, 0]),
                mon(S_BLASTOISE, data_bridge::ABILITY_NONE, [M_TACKLE, 0, 0, 0]),
            ],
            vec![mon(S_GARCHOMP, data_bridge::ABILITY_NONE, [M_EARTHQUAKE, 0, 0, 0])],
        );
        let full = action_features_with(&s, 0, ActionDenseMode::Full);
        let priced = action_features_with(&s, 0, ActionDenseMode::Priced);
        assert_ne!(full[move_byte(0)], ZERO, "Full must produce a non-zero block");
        assert_eq!(priced, [[0.0; ACTION_DENSE_DIM]; ACTION_SPACE], "Priced must return all zeros");
    }
}
