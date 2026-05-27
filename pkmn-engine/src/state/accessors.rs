//! Accessor functions: the mux logic for Transform / type changes / ability overrides.

use crate::state::structs::*;
use crate::state::data_bridge::{self, ItemFlag};

/// Underlying type pair, matching Showdown's `pokemon.types` array (which
/// terastallize() does NOT touch — sim/battle-actions.ts:1925-1961). Used by
/// the run_scenario snapshot. Engine internals that care about damage / move
/// behavior should use `battle_types` instead, which honors tera.
#[inline(always)]
pub fn effective_types(state: &BattleState, side: usize) -> (u8, u8) {
    let mon = state.active_mon(side);
    let active = &state.sides[side].active;

    if active.has_volatile(VOL_TYPES_OVERRIDDEN) || active.has_volatile(VOL_TRANSFORMED) {
        return (active.override_types[0], active.override_types[1]);
    }

    let sp = data_bridge::species(mon.species_id);
    (sp.type1 as u8, sp.type2 as u8)
}

/// Type pair as Showdown's `pokemon.getTypes()` resolves it: when terastallized,
/// returns (tera_type, tera_type). Used by damage calc, type-effectiveness
/// checks, STAB, type-based status immunity, grounding (Flying-type), etc.
#[inline(always)]
pub fn battle_types(state: &BattleState, side: usize) -> (u8, u8) {
    let mon = state.active_mon(side);
    if mon.is_terastallized() {
        return (mon.tera_type, mon.tera_type);
    }
    // Multitype/RKS System are cantsuppress and can't be swapped onto another
    // species, so the stored ability is a sound (allocation-free) discriminator;
    // Transform is the only way to lose them, and it overrides types wholesale.
    if matches!(mon.ability_id,
        data_bridge::ABILITY_MULTITYPE | data_bridge::ABILITY_RKS_SYSTEM)
        && !state.sides[side].active.has_volatile(VOL_TRANSFORMED)
    {
        let t = multitype_runtime_type(mon.item_id, mon.ability_id);
        return (t, t);
    }
    effective_types(state, side)
}

/// Showdown's `arceus`/`silvally` onType: Multitype/RKS System derive their
/// runtime type from the held Plate (Arceus) or Memory (Silvally), falling back
/// to Normal. The forme the item produces carries that type, so reuse the
/// item→forme table; a cross-family item (e.g. a Memory on Arceus) yields Normal.
#[inline]
fn multitype_runtime_type(item_id: u16, ability_id: u16) -> u8 {
    let base = if ability_id == data_bridge::ABILITY_MULTITYPE { 493 } else { 773 };
    let forme = data_bridge::item_forme(item_id);
    if forme != 0 && data_bridge::base_species(forme) == base {
        data_bridge::species(forme).type1 as u8
    } else {
        crate::data::types::Type::Normal as u8
    }
}

#[inline(always)]
pub fn effective_weather(state: &BattleState) -> u8 {
    if state.field.field_flags & FIELD_WEATHER_SUPPRESSED != 0 {
        WEATHER_NONE
    } else {
        state.field.weather
    }
}

#[inline(always)]
pub fn effective_weather_for(state: &BattleState, side: usize) -> u8 {
    let w = effective_weather(state);
    if matches!(w, WEATHER_SUN | WEATHER_HARSH_SUN | WEATHER_RAIN | WEATHER_HEAVY_RAIN) {
        let mon = state.active_mon(side);
        if mon.item_id != 0 && state.field.magic_room_turns() == 0
            && data_bridge::item(mon.item_id).has(ItemFlag::UTILITY_UMBRELLA)
        {
            return WEATHER_NONE;
        }
    }
    w
}

#[inline(always)]
pub fn effective_ability(state: &BattleState, side: usize) -> u16 {
    let mon = state.active_mon(side);
    // hp==0 is NOT Showdown's `!isActive`: a queued-but-unprocessed faint keeps the
    // ability live mid-action. Sites that read across the faint seam (KO-trigger hooks,
    // self-faint -ate moves) use effective_ability_ignoring_faint instead.
    if mon.is_fainted() { return 0; }
    effective_ability_ignoring_faint(state, side)
}

/// `effective_ability` without the fainted short-circuit. KO-triggered defender
/// hooks (Aftermath / Innards Out / Rough Skin / Iron Barbs) read the ability at
/// the moment of the killing blow, matching Showdown's `target.ability` read in
/// `onDamagingHit`. Still honors Gastro Acid / Skill Swap / Transform.
#[inline(always)]
pub fn effective_ability_ignoring_faint(state: &BattleState, side: usize) -> u16 {
    let mon = state.active_mon(side);
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_ABILITY_SUPPRESSED) { return 0; }
    if active.has_volatile(VOL_ABILITY_OVERRIDDEN) || active.has_volatile(VOL_TRANSFORMED) {
        // Showdown ignores `notransform`-flagged abilities while transformed.
        if active.has_volatile(VOL_TRANSFORMED)
            && data_bridge::ability_notransform(active.override_ability)
        {
            return 0;
        }
        return active.override_ability;
    }
    mon.ability_id
}

/// True iff `ability_id` confers immunity to `status`. Covers the status-blocker
/// ability family. All of these carry Showdown's `breakable: 1` flag, so the
/// inflict site composes the result with `mold_breaks`.
#[inline(always)]
pub fn ability_blocks_status(ability_id: u16, status: u8) -> bool {
    match status {
        STATUS_BURN      => ability_id == data_bridge::ABILITY_WATER_VEIL,
        STATUS_PARALYSIS => ability_id == data_bridge::ABILITY_LIMBER,
        STATUS_FREEZE    => ability_id == data_bridge::ABILITY_MAGMA_ARMOR,
        STATUS_SLEEP     => ability_id == data_bridge::ABILITY_INSOMNIA
                            || ability_id == data_bridge::ABILITY_VITAL_SPIRIT,
        STATUS_POISON | STATUS_BAD_POISON =>
            ability_id == data_bridge::ABILITY_IMMUNITY
            || ability_id == data_bridge::ABILITY_PASTEL_VEIL,
        _ => false,
    }
}

#[inline(always)]
pub fn effective_stat(state: &BattleState, side: usize, stat_index: usize) -> u16 {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_stats[stat_index]; }
    // Forme-change stat overrides (Aegislash Blade, Zen Mode, etc.); Power/Guard Split
    // also write the full override_stats array on a non-Transform/non-forme mon, gated
    // by ACTIVE_PAD_STATS_SPLIT so a split mon reads the averaged stat pair.
    if active.override_stats[0] != 0 || active.stats_split_active() {
        return active.override_stats[stat_index];
    }
    state.active_mon(side).stats[stat_index]
}

#[inline(always)]
pub fn effective_moves(state: &BattleState, side: usize) -> [u16; 4] {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_moves; }
    state.active_mon(side).moves
}

#[inline(always)]
pub fn effective_pp(state: &BattleState, side: usize, move_slot: usize) -> u8 {
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRANSFORMED) { return active.override_pp[move_slot]; }
    state.active_mon(side).pp[move_slot]
}

#[inline(always)]
pub fn effective_species(state: &BattleState, side: usize) -> u16 {
    let active = &state.sides[side].active;
    if active.override_species != 0 {
        return active.override_species;
    }
    state.active_mon(side).species_id
}

#[inline]
pub fn has_type(state: &BattleState, side: usize, check_type: u8) -> bool {
    let (t1, t2) = battle_types(state, side);
    t1 == check_type || t2 == check_type
}

/// Gen 6+ type-based status immunities:
/// Fire→Burn, Electric→Paralysis, Poison/Steel→Poison/Toxic, Ice→Freeze
#[inline(always)]
pub fn type_immune_to_status(state: &BattleState, side: usize, status: u8) -> bool {
    use crate::data::types::Type;
    use crate::state::structs::*;
    let (t1, t2) = battle_types(state, side);
    match status {
        STATUS_BURN => t1 == Type::Fire as u8 || t2 == Type::Fire as u8,
        STATUS_PARALYSIS => t1 == Type::Electric as u8 || t2 == Type::Electric as u8,
        STATUS_POISON | STATUS_BAD_POISON => {
            t1 == Type::Poison as u8 || t2 == Type::Poison as u8
            || t1 == Type::Steel as u8 || t2 == Type::Steel as u8
        }
        STATUS_FREEZE => t1 == Type::Ice as u8 || t2 == Type::Ice as u8,
        _ => false,
    }
}

/// Check if the active Pokémon is grounded.
/// Grounded = NOT (Flying-type OR Levitate OR Air Balloon OR Magnet Rise OR Telekinesis)
/// unless Gravity is active (overrides all).
#[inline]
pub fn is_grounded(state: &BattleState, side: usize) -> bool {
    let field = &state.field;
    if field.gravity_turns > 0 { return true; }

    let active = &state.sides[side].active;
    if active.has_volatile(VOL_SMACKED_DOWN) { return true; }
    if active.has_volatile(VOL_INGRAIN) { return true; }
    if has_type(state, side, Type::Flying as u8) { return false; }
    if effective_ability(state, side) == data_bridge::ABILITY_LEVITATE { return false; }
    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::AIR_BALLOON) { return false; }
    if active.has_volatile(VOL_MAGNET_RISE) { return false; }
    if active.telekinesis_turns > 0 { return false; }

    true
}

/// Same as is_grounded but ignores the defender's ability (for Mold Breaker bypass).
/// Skips the Levitate check so that Mold Breaker can hit Ground-immune Levitate mons.
#[inline]
pub fn is_grounded_ignore_ability(state: &BattleState, side: usize) -> bool {
    let field = &state.field;
    if field.gravity_turns > 0 { return true; }

    let active = &state.sides[side].active;
    if active.has_volatile(VOL_SMACKED_DOWN) { return true; }
    if active.has_volatile(VOL_INGRAIN) { return true; }
    if has_type(state, side, Type::Flying as u8) { return false; }
    // Levitate check SKIPPED: Mold Breaker suppresses it
    if state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::AIR_BALLOON) { return false; }
    if active.has_volatile(VOL_MAGNET_RISE) { return false; }
    if active.telekinesis_turns > 0 { return false; }

    true
}

#[inline]
pub fn is_trap_immune(state: &BattleState, side: usize) -> bool {
    if has_type(state, side, Type::Ghost as u8) { return true; }
    state.field.magic_room_turns() == 0
        && data_bridge::item(state.active_mon(side).item_id).has(ItemFlag::TRAP_IMMUNE)
}

/// Return true if `side` is prevented from switching out.
/// Shared by legal_moves (choice generation) and switch execution (rejection).
///
/// Conditions that trap:
///   - Ghost-type / Shed Shell: NOT trapped (is_trap_immune)
///   - VOL_TRAPPED, VOL_INGRAIN, VOL_BOUND, VOL_MOVE_LOCKED, VOL_CHARGING
///   - Opponent ability: Magnet Pull (Steel only), Arena Trap (grounded only),
///     Shadow Tag (unless user also has Shadow Tag)
///
/// Hot path: checked on every switch attempt. Quick-reject via raw ability id.
#[inline(always)]
pub fn is_trapped(state: &BattleState, side: usize) -> bool {
    if is_trap_immune(state, side) { return false; }

    let active = &state.sides[side].active;
    if active.has_volatile(VOL_TRAPPED)
        || active.has_volatile(VOL_INGRAIN)
        || active.has_volatile(VOL_BOUND)
        || active.has_volatile(VOL_MOVE_LOCKED)
        || active.has_volatile(VOL_CHARGING)
    {
        return true;
    }

    // Opponent's trapping ability. Quick-reject via raw ability id to avoid
    // effective_ability overhead in the common case.
    let opp = 1 - side;
    let opp_raw = state.active_mon(opp).ability_id;
    let opp_volatiles = state.sides[opp].active.volatile_flags;
    let maybe_trapper = opp_raw == data_bridge::ABILITY_MAGNET_PULL
        || opp_raw == data_bridge::ABILITY_ARENA_TRAP
        || opp_raw == data_bridge::ABILITY_SHADOW_TAG
        || (opp_volatiles & (VOL_ABILITY_OVERRIDDEN | VOL_TRANSFORMED)) != 0;
    if !maybe_trapper { return false; }

    // Opponent ability may be suppressed (Neutralizing Gas / Gastro Acid).
    let opp_ability = effective_ability(state, opp);
    match opp_ability {
        data_bridge::ABILITY_MAGNET_PULL => has_type(state, side, Type::Steel as u8),
        data_bridge::ABILITY_ARENA_TRAP => is_grounded(state, side),
        data_bridge::ABILITY_SHADOW_TAG => {
            effective_ability(state, side) != data_bridge::ABILITY_SHADOW_TAG
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_transform_overrides() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 132;
        state.sides[0].team[0].stats = [48, 48, 48, 48, 48];
        state.sides[0].active.volatile_flags |= VOL_TRANSFORMED;
        state.sides[0].active.override_stats = [84, 78, 109, 85, 100];
        state.sides[0].active.override_moves = [53, 126, 257, 394];
        state.sides[0].active.override_pp = [5, 5, 5, 5];
        assert_eq!(effective_stat(&state, 0, ATK), 84);
        assert_eq!(effective_moves(&state, 0), [53, 126, 257, 394]);
    }

    #[test]
    fn test_transform_suppresses_notransform_ability() {
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 132;
        state.sides[0].team[0].current_hp = 100;
        state.sides[0].active.volatile_flags |= VOL_TRANSFORMED;
        state.sides[0].active.override_ability = data_bridge::ABILITY_PROTOSYNTHESIS;
        assert_eq!(effective_ability(&state, 0), 0);
        assert_eq!(effective_ability_ignoring_faint(&state, 0), 0);
        // Copied abilities without the notransform flag stay live.
        state.sides[0].active.override_ability = data_bridge::ABILITY_INTIMIDATE;
        assert_eq!(effective_ability(&state, 0), data_bridge::ABILITY_INTIMIDATE);
    }

    #[test]
    fn test_gravity_grounds_flying() {
        use crate::data::types::Type;
        let mut state = BattleState::default();
        state.sides[0].team[0].species_id = 18; // Pidgeot (Flying type)
        // Without Gravity: Flying type is not grounded
        assert!(!is_grounded(&state, 0));
        // With Gravity: everything is grounded
        state.field.gravity_turns = 5;
        assert!(is_grounded(&state, 0));
    }

    #[test]
    fn test_multitype_runtime_type_reverts_without_plate() {
        use crate::data::types::Type;
        let mut state = BattleState::default();
        let mon = &mut state.sides[0].team[0];
        mon.species_id = 1113; // Arceus-Bug
        mon.ability_id = data_bridge::ABILITY_MULTITYPE;
        mon.item_id = 707; // Clover Sweet (non-Plate)
        mon.current_hp = 209;
        // Snapshot type stays Bug; getTypes-equivalent reverts to Normal.
        assert_eq!(effective_types(&state, 0), (Type::Bug as u8, Type::Bug as u8));
        assert_eq!(battle_types(&state, 0), (Type::Normal as u8, Type::Normal as u8));
        // Matching Plate keeps the plate type.
        state.sides[0].team[0].item_id = 223; // Insect Plate
        assert_eq!(battle_types(&state, 0), (Type::Bug as u8, Type::Bug as u8));
        // A mismatched Plate re-types to that plate (Showdown's onPlate).
        state.sides[0].team[0].item_id = 105; // Draco Plate
        assert_eq!(battle_types(&state, 0), (Type::Dragon as u8, Type::Dragon as u8));
    }

    #[test]
    fn test_rks_system_runtime_type() {
        use crate::data::types::Type;
        let mut state = BattleState::default();
        let mon = &mut state.sides[0].team[0];
        mon.species_id = 1371; // Silvally-Bug
        mon.ability_id = data_bridge::ABILITY_RKS_SYSTEM;
        mon.item_id = 707; // non-Memory
        mon.current_hp = 200;
        assert_eq!(battle_types(&state, 0), (Type::Normal as u8, Type::Normal as u8));
        state.sides[0].team[0].item_id = 673; // Bug Memory
        assert_eq!(battle_types(&state, 0), (Type::Bug as u8, Type::Bug as u8));
        // A Plate (Arceus item) on Silvally is cross-family → Normal.
        state.sides[0].team[0].item_id = 223; // Insect Plate
        assert_eq!(battle_types(&state, 0), (Type::Normal as u8, Type::Normal as u8));
    }
}
