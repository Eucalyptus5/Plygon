use pkmn_engine::data::types::{Type, NUM_TYPES};
use pkmn_engine::data::{GEN_ITEMS, GEN_MOVES, TOTAL_SPECIES};
use pkmn_engine::state::*;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExecutionMode {
    ExecuteTurns,
    CalcDamage,
    LegalActions,
}

#[derive(Deserialize)]
struct ScenarioInput {
    name: String,
    mode: ExecutionMode,
    teams: TeamsInput,
    #[serde(default)]
    state_overrides: Option<StateOverrides>,
    #[serde(default)]
    turns: Vec<TurnInput>,
    #[serde(default)]
    calc_damage_params: Option<CalcDamageParams>,
    #[serde(default)]
    legal_actions_params: Option<LegalActionsParams>,
    // When present, execute_turns threads one seeded LCG across all turns instead of the per-turn forcing closures.
    #[serde(default)]
    sample_seed: Option<u64>,
}

#[derive(Deserialize)]
struct TeamsInput {
    p1: Vec<MonInput>,
    p2: Vec<MonInput>,
}

#[derive(Deserialize, Clone)]
struct MonInput {
    species_id: u16,
    ability_id: u16,
    #[serde(default)]
    item_id: u16,
    moves: [u16; 4],
    #[serde(default = "default_ivs")]
    ivs: [u8; 6],
    #[serde(default)]
    evs: [u8; 6],
    #[serde(default)]
    nature: u8,
    #[serde(default = "default_level")]
    level: u8,
    #[serde(default)]
    tera_type: u8,
    #[serde(default)]
    is_female: bool,
}

fn default_ivs() -> [u8; 6] { [31; 6] }
fn default_level() -> u8 { 100 }

#[derive(Deserialize)]
struct TurnInput {
    p1_action: u8,
    p2_action: u8,
    #[serde(default = "default_rng_mode")]
    rng_mode: RngMode,
    #[serde(default)]
    rng_overrides: Option<RngOverrides>,
}

fn default_rng_mode() -> RngMode { RngMode::ForceAll }

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum RngMode {
    ForceAll,
    ForceNone,
    MinRoll,
    MaxRoll,
    AllRolls,
    Specific,
}

#[derive(Deserialize, Clone)]
struct RngOverrides {
    #[serde(default = "default_roll")]
    damage_roll: u32,
    #[serde(default)]
    crit: bool,
    #[serde(default = "default_true")]
    secondary_trigger: bool,
    #[serde(default = "default_true")]
    accuracy_hit: bool,
    #[serde(default)]
    speed_tie: u32,
    #[serde(default = "default_multi")]
    multi_hit_count: u32,
}

fn default_roll() -> u32 { 15 }
fn default_true() -> bool { true }
fn default_multi() -> u32 { 0 }

#[derive(Deserialize)]
struct CalcDamageParams {
    atk_side: usize,
    move_id: u16,
    #[serde(default = "default_true")]
    include_crit: bool,
    #[serde(default = "default_accuracy")]
    per_hit_accuracy: u32,
}

fn default_accuracy() -> u32 { 100 }

#[derive(Deserialize)]
struct LegalActionsParams {
    side: usize,
}

#[derive(Deserialize, Default)]
struct StateOverrides {
    #[serde(default)]
    field: Option<FieldOverrides>,
    #[serde(default)]
    p1: Option<SideOverrides>,
    #[serde(default)]
    p2: Option<SideOverrides>,
}

#[derive(Deserialize, Default)]
struct FieldOverrides {
    #[serde(default)]
    weather: Option<u8>,
    #[serde(default)]
    weather_turns: Option<u8>,
    #[serde(default)]
    terrain: Option<u8>,
    #[serde(default)]
    terrain_turns: Option<u8>,
    #[serde(default)]
    trick_room_turns: Option<u8>,
    #[serde(default)]
    gravity_turns: Option<u8>,
    #[serde(default)]
    magic_room_turns: Option<u8>,
    #[serde(default)]
    wonder_room_turns: Option<u8>,
}

#[derive(Deserialize, Default)]
struct SideOverrides {
    #[serde(default)]
    active_overrides: Option<ActiveOverrides>,
    #[serde(default)]
    side_conditions: Option<SideCondOverrides>,
    #[serde(default)]
    team_overrides: Option<Vec<TeamSlotOverride>>,
}

#[derive(Deserialize, Default)]
struct ActiveOverrides {
    #[serde(default)]
    boosts: Option<[i8; 7]>,
    #[serde(default)]
    status: Option<u8>,
    #[serde(default)]
    status_counter: Option<u8>,
    #[serde(default)]
    current_hp: Option<u16>,
    #[serde(default)]
    volatile_flags: Option<u32>,
    #[serde(default)]
    substitute_hp: Option<u16>,
    #[serde(default)]
    confusion_turns: Option<u8>,
    #[serde(default)]
    taunt_turns: Option<u8>,
    #[serde(default)]
    toxic_counter: Option<u8>,
    #[serde(default)]
    attracted: Option<bool>,
}

#[derive(Deserialize)]
struct SideCondOverrides {
    #[serde(default)]
    reflect_turns: Option<u8>,
    #[serde(default)]
    light_screen_turns: Option<u8>,
    #[serde(default)]
    aurora_veil_turns: Option<u8>,
    #[serde(default)]
    spikes: Option<u8>,
    #[serde(default)]
    toxic_spikes: Option<u8>,
    #[serde(default)]
    stealth_rock: Option<bool>,
    #[serde(default)]
    sticky_web: Option<bool>,
    #[serde(default)]
    tailwind_turns: Option<u8>,
    #[serde(default)]
    safeguard_turns: Option<u8>,
    #[serde(default)]
    mist_turns: Option<u8>,
    #[serde(default)]
    lucky_chant_turns: Option<u8>,
    #[serde(default)]
    wish_turns: Option<u8>,
    #[serde(default)]
    wish_hp: Option<u16>,
}

#[derive(Deserialize)]
struct TeamSlotOverride {
    slot: usize,
    #[serde(default)]
    current_hp: Option<u16>,
    #[serde(default)]
    status: Option<u8>,
    #[serde(default)]
    item_id: Option<u16>,
}

#[derive(Serialize)]
struct ErrorOutput {
    __req_id: serde_json::Value,
    name: String,
    success: bool,
    error: String,
}

#[derive(Serialize)]
struct ExecuteTurnsOutput {
    __req_id: serde_json::Value,
    name: String,
    mode: String,
    success: bool,
    error: Option<String>,
    initial_state: StateSnapshot,
    turns: Vec<TurnResult>,
}

#[derive(Serialize)]
struct TurnResult {
    turn_number: usize,
    state_after: StateSnapshotOrRolls,
    legal_actions_after: LegalActionsSnapshot,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StateSnapshotOrRolls {
    Single(StateSnapshot),
    AllRolls(Vec<StateSnapshot>),
}

#[derive(Serialize)]
struct LegalActionsSnapshot {
    p1: Vec<u8>,
    p2: Vec<u8>,
}

#[derive(Serialize)]
struct CalcDamageOutput {
    __req_id: serde_json::Value,
    name: String,
    mode: String,
    success: bool,
    error: Option<String>,
    results: CalcDamageResults,
}

#[derive(Serialize)]
struct CalcDamageResults {
    all_rolls: Vec<DamageRollOut>,
    crit_results: Vec<DamageRollOut>,
    min_damage: u16,
    max_damage: u16,
    min_damage_crit: u16,
    max_damage_crit: u16,
}

#[derive(Serialize)]
struct DamageRollOut {
    roll: u32,
    damage: u16,
    effectiveness: u8,
    crit: bool,
    hits: u8,
    type_immune: bool,
    drain_heal: u16,
    recoil_damage: u16,
    hits_substitute: bool,
    item_consumed: bool,
}

#[derive(Serialize)]
struct LegalActionsOutput {
    __req_id: serde_json::Value,
    name: String,
    mode: String,
    success: bool,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actions: Option<DecodedActions>,
    both_sides: LegalActionsSnapshot,
}

#[derive(Serialize)]
struct DecodedActions {
    raw: Vec<u8>,
    decoded: Vec<DecodedAction>,
    count: usize,
}

#[derive(Serialize)]
struct DecodedAction {
    action: u8,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_slot: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_id: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_slot: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_species: Option<u16>,
}

#[derive(Serialize)]
struct StateSnapshot {
    phase: String,
    field: FieldSnapshot,
    p1: SideSnapshot,
    p2: SideSnapshot,
}

#[derive(Serialize)]
struct FieldSnapshot {
    turn: u16,
    weather: String,
    weather_turns: u8,
    terrain: String,
    terrain_turns: u8,
    trick_room_turns: u8,
    gravity_turns: u8,
    magic_room_turns: u8,
    wonder_room_turns: u8,
}

#[derive(Serialize)]
struct SideSnapshot {
    active_index: u8,
    team: Vec<MonSnapshot>,
    active: ActiveSnapshot,
    side_conditions: SideCondSnapshot,
    effective: EffectiveSnapshot,
}

#[derive(Serialize)]
struct MonSnapshot {
    slot: usize,
    species_id: u16,
    ability_id: u16,
    item_id: u16,
    current_hp: u16,
    max_hp: u16,
    stats: [u16; 5],
    moves: [u16; 4],
    pp: [u8; 4],
    status: String,
    status_counter: u8,
    tera_type: u8,
    is_terastallized: bool,
    is_fainted: bool,
}

#[derive(Serialize)]
struct ActiveSnapshot {
    boosts: [i8; 7],
    volatile_flags: u32,
    volatile_names: Vec<&'static str>,
    substitute_hp: u16,
    confusion_turns: u8,
    taunt_turns: u8,
    encore_turns: u8,
    disable_turns: u8,
    protect_consecutive: u8,
    toxic_counter: u8,
    turns_active: u8,
    last_move: u16,
    choice_locked_move: u16,
    disabled_move: u16,
    encore_move: u16,
    bind_turns: u8,
    is_attracted: bool,
    perish_count: u8,
}

#[derive(Serialize)]
struct SideCondSnapshot {
    spikes: u8,
    toxic_spikes: u8,
    stealth_rock: bool,
    sticky_web: bool,
    reflect_turns: u8,
    light_screen_turns: u8,
    aurora_veil_turns: u8,
    tailwind_turns: u8,
    wish_turns: u8,
    wish_hp: u16,
    safeguard_turns: u8,
    mist_turns: u8,
    lucky_chant_turns: u8,
    has_healing_wish: bool,
    has_lunar_dance: bool,
}

#[derive(Serialize)]
struct EffectiveSnapshot {
    types: [String; 2],
    ability: u16,
    species: u16,
    stats: [u16; 5],
    moves: [u16; 4],
    is_grounded: bool,
}

fn status_name(s: u8) -> &'static str {
    match s {
        STATUS_NONE => "none",
        STATUS_BURN => "burn",
        STATUS_PARALYSIS => "paralysis",
        STATUS_POISON => "poison",
        STATUS_BAD_POISON => "bad_poison",
        STATUS_SLEEP => "sleep",
        STATUS_FREEZE => "freeze",
        _ => "unknown",
    }
}

fn phase_name(p: u8) -> &'static str {
    match p {
        PHASE_ACTIONS => "actions",
        PHASE_SWITCH_P1 => "switch_p1",
        PHASE_SWITCH_P2 => "switch_p2",
        PHASE_SWITCH_BOTH => "switch_both",
        PHASE_GAME_OVER => "game_over",
        _ => "unknown",
    }
}

fn weather_name(w: u8) -> &'static str {
    match w {
        WEATHER_NONE => "none",
        WEATHER_SUN => "sun",
        WEATHER_RAIN => "rain",
        WEATHER_SAND => "sand",
        WEATHER_SNOW => "snow",
        WEATHER_HARSH_SUN => "harsh_sun",
        WEATHER_HEAVY_RAIN => "heavy_rain",
        WEATHER_STRONG_WINDS => "strong_winds",
        _ => "unknown",
    }
}

fn terrain_name(t: u8) -> &'static str {
    match t {
        TERRAIN_NONE => "none",
        TERRAIN_ELECTRIC => "electric",
        TERRAIN_GRASSY => "grassy",
        TERRAIN_PSYCHIC => "psychic",
        TERRAIN_MISTY => "misty",
        _ => "unknown",
    }
}

fn type_name(t: u8) -> String {
    if t < NUM_TYPES as u8 {
        format!("{:?}", unsafe { std::mem::transmute::<u8, Type>(t) })
    } else {
        format!("Unknown({})", t)
    }
}

fn volatile_names(flags: u32) -> Vec<&'static str> {
    const NAMES: [(u32, &str); 32] = [
        (VOL_SUBSTITUTE, "substitute"),
        (VOL_LEECH_SEED, "leech_seed"),
        (VOL_TRAPPED, "trapped"),
        (VOL_CHARGING, "charging"),
        (VOL_SEMI_INVULNERABLE, "semi_invulnerable"),
        (VOL_RECHARGING, "recharging"),
        (VOL_FLINCHED, "flinched"),
        (VOL_MOVED_THIS_TURN, "moved_this_turn"),
        (VOL_PROTECT_THIS_TURN, "protect_this_turn"),
        (VOL_ENDURE, "endure"),
        (VOL_FOCUS_ENERGY, "focus_energy"),
        (VOL_TORMENT, "torment"),
        (VOL_IMPRISON, "imprison"),
        (VOL_ABILITY_SUPPRESSED, "ability_suppressed"),
        (VOL_UNBURDEN, "unburden"),
        (VOL_FLASH_FIRE, "flash_fire"),
        (VOL_MINIMIZE, "minimize"),
        (VOL_SMACKED_DOWN, "smacked_down"),
        (VOL_AQUA_RING, "aqua_ring"),
        (VOL_INGRAIN, "ingrain"),
        (VOL_MAGNET_RISE, "magnet_rise"),
        (VOL_PERISH_SONG, "perish_song"),
        (VOL_DESTINY_BOND, "destiny_bond"),
        (VOL_GRUDGE, "grudge"),
        (VOL_MOVE_LOCKED, "move_locked"),
        (VOL_TYPES_OVERRIDDEN, "types_overridden"),
        (VOL_TRANSFORMED, "transformed"),
        (VOL_ABILITY_OVERRIDDEN, "ability_overridden"),
        (VOL_MUST_SWITCH, "must_switch"),
        (VOL_BOUND, "bound"),
        (VOL_YAWN, "yawn"),
        (VOL_LASER_FOCUS, "laser_focus"),
    ];
    NAMES.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, n)| *n).collect()
}

fn extract_snapshot(state: &BattleState) -> StateSnapshot {
    StateSnapshot {
        phase: phase_name(state.phase).to_string(),
        field: FieldSnapshot {
            turn: state.field.turn,
            weather: weather_name(state.field.weather).to_string(),
            weather_turns: state.field.weather_turns,
            terrain: terrain_name(state.field.terrain).to_string(),
            terrain_turns: state.field.terrain_turns,
            trick_room_turns: state.field.trick_room_turns,
            gravity_turns: state.field.gravity_turns,
            magic_room_turns: state.field.magic_room_turns(),
            wonder_room_turns: state.field.wonder_room_turns(),
        },
        p1: extract_side(state, 0),
        p2: extract_side(state, 1),
    }
}

fn extract_side(state: &BattleState, side: usize) -> SideSnapshot {
    let s = &state.sides[side];
    let active = &s.active;
    let sc = &s.side_conditions;

    let team: Vec<MonSnapshot> = (0..6)
        .map(|i| {
            let m = &s.team[i];
            MonSnapshot {
                slot: i,
                species_id: m.species_id,
                ability_id: m.ability_id,
                item_id: m.item_id,
                current_hp: m.current_hp,
                max_hp: m.max_hp,
                stats: m.stats,
                moves: m.moves,
                pp: m.pp,
                status: status_name(m.status).to_string(),
                status_counter: m.status_counter,
                tera_type: m.tera_type,
                is_terastallized: m.is_terastallized(),
                is_fainted: m.is_fainted(),
            }
        })
        .collect();

    let (t1, t2) = effective_types(state, side);
    let eff_stats = [
        effective_stat(state, side, ATK),
        effective_stat(state, side, DEF),
        effective_stat(state, side, SPA),
        effective_stat(state, side, SPD),
        effective_stat(state, side, SPE),
    ];

    SideSnapshot {
        active_index: s.active_index,
        team,
        active: ActiveSnapshot {
            boosts: active.boosts,
            volatile_flags: active.volatile_flags,
            volatile_names: volatile_names(active.volatile_flags),
            substitute_hp: active.substitute_hp,
            confusion_turns: active.confusion_turns,
            taunt_turns: active.taunt_turns,
            encore_turns: active.encore_turns,
            disable_turns: active.disable_turns,
            protect_consecutive: active.protect_consecutive,
            toxic_counter: active.toxic_counter,
            turns_active: active.turns_active,
            last_move: active.last_move,
            choice_locked_move: active.choice_locked_move,
            disabled_move: active.disabled_move,
            encore_move: active.encore_move,
            bind_turns: active.bind_turns(),
            is_attracted: active.is_attracted(),
            perish_count: active.perish_count,
        },
        side_conditions: SideCondSnapshot {
            spikes: sc.spikes,
            toxic_spikes: sc.toxic_spikes,
            stealth_rock: sc.hazard_flags & HAZARD_STEALTH_ROCK != 0,
            sticky_web: sc.hazard_flags & HAZARD_STICKY_WEB != 0,
            reflect_turns: sc.reflect_turns,
            light_screen_turns: sc.light_screen_turns,
            aurora_veil_turns: sc.aurora_veil_turns,
            tailwind_turns: sc.tailwind_turns,
            wish_turns: sc.wish_turns,
            wish_hp: sc.wish_hp,
            safeguard_turns: sc.safeguard_turns(),
            mist_turns: sc.mist_turns(),
            lucky_chant_turns: sc.lucky_chant_turns(),
            has_healing_wish: sc.has_healing_wish(),
            has_lunar_dance: sc.has_lunar_dance(),
        },
        effective: EffectiveSnapshot {
            types: [type_name(t1), type_name(t2)],
            ability: effective_ability(state, side),
            species: effective_species(state, side),
            stats: eff_stats,
            moves: effective_moves(state, side),
            is_grounded: is_grounded(state, side),
        },
    }
}

fn extract_legal_actions_snapshot(state: &BattleState) -> LegalActionsSnapshot {
    LegalActionsSnapshot {
        p1: legal_actions(state, 0).as_slice().to_vec(),
        p2: legal_actions(state, 1).as_slice().to_vec(),
    }
}

fn validate(scenario: &ScenarioInput) -> Result<(), String> {
    let validate_mon = |m: &MonInput, label: &str| -> Result<(), String> {
        if m.species_id == 0 || m.species_id as usize >= TOTAL_SPECIES {
            return Err(format!("{}: species_id {} out of range", label, m.species_id));
        }
        for (i, &mv) in m.moves.iter().enumerate() {
            if mv as usize >= GEN_MOVES.len() {
                return Err(format!("{}: move[{}] id {} out of range", label, i, mv));
            }
        }
        if m.item_id as usize >= GEN_ITEMS.len() && m.item_id != 0 {
            return Err(format!("{}: item_id {} out of range", label, m.item_id));
        }
        for (i, &iv) in m.ivs.iter().enumerate() {
            if iv > 31 { return Err(format!("{}: iv[{}]={} > 31", label, i, iv)); }
        }
        for (i, &ev) in m.evs.iter().enumerate() {
            if ev > 252 { return Err(format!("{}: ev[{}]={} > 252", label, i, ev)); }
        }
        let ev_sum: u16 = m.evs.iter().map(|&e| e as u16).sum();
        if ev_sum > 510 { return Err(format!("{}: EV sum {} > 510", label, ev_sum)); }
        if m.nature > 24 { return Err(format!("{}: nature {} > 24", label, m.nature)); }
        if m.level == 0 || m.level > 100 { return Err(format!("{}: level {} invalid", label, m.level)); }
        if m.tera_type > 17 { return Err(format!("{}: tera_type {} > 17", label, m.tera_type)); }
        Ok(())
    };

    if scenario.teams.p1.is_empty() { return Err("p1 team is empty".into()); }
    if scenario.teams.p2.is_empty() { return Err("p2 team is empty".into()); }
    if scenario.teams.p1.len() > 6 { return Err("p1 team > 6 members".into()); }
    if scenario.teams.p2.len() > 6 { return Err("p2 team > 6 members".into()); }

    for (i, m) in scenario.teams.p1.iter().enumerate() {
        validate_mon(m, &format!("p1[{}]", i))?;
    }
    for (i, m) in scenario.teams.p2.iter().enumerate() {
        validate_mon(m, &format!("p2[{}]", i))?;
    }

    match scenario.mode {
        ExecutionMode::CalcDamage => {
            if scenario.calc_damage_params.is_none() {
                return Err("calc_damage mode requires calc_damage_params".into());
            }
        }
        ExecutionMode::LegalActions => {}
        _ => {}
    }

    if let Some(ref params) = scenario.calc_damage_params {
        if params.atk_side > 1 { return Err("atk_side must be 0 or 1".into()); }
        if params.move_id as usize >= GEN_MOVES.len() {
            return Err(format!("calc_damage: move_id {} out of range", params.move_id));
        }
    }

    if let Some(ref params) = scenario.legal_actions_params {
        if params.side > 1 { return Err("legal_actions side must be 0 or 1".into()); }
    }

    if scenario.sample_seed.is_some() && !matches!(scenario.mode, ExecutionMode::ExecuteTurns) {
        return Err("sample_seed is only valid in execute_turns mode".into());
    }

    for (i, turn) in scenario.turns.iter().enumerate() {
        if turn.p1_action > 10 && turn.p1_action != 255 {
            return Err(format!("turn[{}]: p1_action {} out of range (0-10 or 255)", i, turn.p1_action));
        }
        if turn.p2_action > 10 && turn.p2_action != 255 {
            return Err(format!("turn[{}]: p2_action {} out of range (0-10 or 255)", i, turn.p2_action));
        }
    }

    Ok(())
}

fn build_inputs(mons: &[MonInput]) -> [MonBuildInput; 6] {
    let mut inputs: [MonBuildInput; 6] = std::array::from_fn(|_| MonBuildInput {
        species_id: 0, ability_id: 0, item_id: 0,
        moves: [0; 4], ivs: [0; 6], evs: [0; 6],
        nature: 0, level: 100, tera_type: 0, is_female: false,
    });
    for (i, m) in mons.iter().enumerate().take(6) {
        inputs[i] = MonBuildInput {
            species_id: m.species_id,
            ability_id: m.ability_id,
            item_id: m.item_id,
            moves: m.moves,
            ivs: m.ivs,
            evs: m.evs,
            nature: m.nature,
            level: m.level,
            tera_type: m.tera_type,
            is_female: m.is_female,
        };
    }
    inputs
}

fn construct_state(scenario: &ScenarioInput) -> (BattleState, TeamData) {
    let p1_inputs = build_inputs(&scenario.teams.p1);
    let p2_inputs = build_inputs(&scenario.teams.p2);

    let (p1_team, p1_build, p1_levels) = build_team(&p1_inputs);
    let (p2_team, p2_build, p2_levels) = build_team(&p2_inputs);

    let teams = TeamData {
        mons: [p1_build, p2_build],
        levels: [p1_levels, p2_levels],
    };

    let mut state = BattleState::default();
    state.sides[0].team = p1_team;
    state.sides[1].team = p2_team;
    state.phase = PHASE_ACTIONS;

    // Initial switch-in always processes side 0 first; speed ordering is not implemented.
    switch::switch_in(&mut state, &teams, 0, 0);
    switch::switch_in(&mut state, &teams, 1, 0);

    if let Some(ref overrides) = scenario.state_overrides {
        apply_overrides(&mut state, overrides);
    }

    (state, teams)
}

fn apply_overrides(state: &mut BattleState, overrides: &StateOverrides) {
    if let Some(ref f) = overrides.field {
        if let Some(w) = f.weather { state.field.weather = w; }
        if let Some(wt) = f.weather_turns { state.field.weather_turns = wt; }
        if let Some(t) = f.terrain { state.field.terrain = t; }
        if let Some(tt) = f.terrain_turns { state.field.terrain_turns = tt; }
        if let Some(tr) = f.trick_room_turns { state.field.trick_room_turns = tr; }
        if let Some(g) = f.gravity_turns { state.field.gravity_turns = g; }
        if let Some(mr) = f.magic_room_turns { state.field.set_magic_room_turns(mr); }
        if let Some(wr) = f.wonder_room_turns { state.field.set_wonder_room_turns(wr); }
    }

    if let Some(ref p) = overrides.p1 { apply_side_overrides(state, 0, p); }
    if let Some(ref p) = overrides.p2 { apply_side_overrides(state, 1, p); }
}

fn apply_side_overrides(state: &mut BattleState, side: usize, overrides: &SideOverrides) {
    if let Some(ref ao) = overrides.active_overrides {
        let idx = state.sides[side].active_index as usize;
        if let Some(boosts) = ao.boosts { state.sides[side].active.boosts = boosts; }
        if let Some(s) = ao.status {
            state.sides[side].team[idx].status = s;
            // Auto-set sleep counter if not explicitly provided (Showdown uses random(2,5))
            if s == STATUS_SLEEP && ao.status_counter.is_none() {
                state.sides[side].team[idx].status_counter = 3;
            }
        }
        if let Some(sc) = ao.status_counter { state.sides[side].team[idx].status_counter = sc; }
        if let Some(hp) = ao.current_hp { state.sides[side].team[idx].current_hp = hp; }
        if let Some(vf) = ao.volatile_flags { state.sides[side].active.volatile_flags = vf; }
        if let Some(sub) = ao.substitute_hp { state.sides[side].active.substitute_hp = sub; }
        if let Some(ct) = ao.confusion_turns { state.sides[side].active.confusion_turns = ct; }
        if let Some(tt) = ao.taunt_turns { state.sides[side].active.taunt_turns = tt; }
        if let Some(tc) = ao.toxic_counter { state.sides[side].active.toxic_counter = tc; }
        if let Some(at) = ao.attracted { state.sides[side].active.set_attracted(at); }
    }

    if let Some(ref sc) = overrides.side_conditions {
        let cond = &mut state.sides[side].side_conditions;
        if let Some(r) = sc.reflect_turns { cond.reflect_turns = r; }
        if let Some(l) = sc.light_screen_turns { cond.light_screen_turns = l; }
        if let Some(a) = sc.aurora_veil_turns { cond.aurora_veil_turns = a; }
        if let Some(s) = sc.spikes { cond.spikes = s; }
        if let Some(ts) = sc.toxic_spikes { cond.toxic_spikes = ts; }
        if let Some(sr) = sc.stealth_rock {
            if sr { cond.hazard_flags |= HAZARD_STEALTH_ROCK; }
            else { cond.hazard_flags &= !HAZARD_STEALTH_ROCK; }
        }
        if let Some(sw) = sc.sticky_web {
            if sw { cond.hazard_flags |= HAZARD_STICKY_WEB; }
            else { cond.hazard_flags &= !HAZARD_STICKY_WEB; }
        }
        if let Some(tw) = sc.tailwind_turns { cond.tailwind_turns = tw; }
        if let Some(sg) = sc.safeguard_turns { cond.set_safeguard_turns(sg); }
        if let Some(mt) = sc.mist_turns { cond.set_mist_turns(mt); }
        if let Some(lc) = sc.lucky_chant_turns { cond.set_lucky_chant_turns(lc); }
        if let Some(wt) = sc.wish_turns { cond.wish_turns = wt; }
        if let Some(wh) = sc.wish_hp { cond.wish_hp = wh; }
    }

    if let Some(ref team_ov) = overrides.team_overrides {
        for ov in team_ov {
            if ov.slot < 6 {
                if let Some(hp) = ov.current_hp { state.sides[side].team[ov.slot].current_hp = hp; }
                if let Some(s) = ov.status { state.sides[side].team[ov.slot].status = s; }
                if let Some(it) = ov.item_id { state.sides[side].team[ov.slot].item_id = it; }
            }
        }
    }
}

// Must stay bit-identical to the LCG the engine's battle driver uses.
fn sampled_rng(seed: u64) -> impl FnMut(u32) -> u32 {
    let mut state = seed;
    move |max| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as u32) % max.max(1)
    }
}

fn handle_execute_turns(scenario: &ScenarioInput, req_id: serde_json::Value) -> String {
    let (mut state, teams) = construct_state(scenario);
    let initial_state = extract_snapshot(&state);
    let mut turn_results = Vec::new();

    // The stream must persist across turns, so this path ignores the per-turn rng_mode.
    if let Some(seed) = scenario.sample_seed {
        let mut rng = sampled_rng(seed);
        for (idx, turn) in scenario.turns.iter().enumerate() {
            if state.phase == PHASE_GAME_OVER {
                turn_results.push(TurnResult {
                    turn_number: idx + 1,
                    state_after: StateSnapshotOrRolls::Single(extract_snapshot(&state)),
                    legal_actions_after: extract_legal_actions_snapshot(&state),
                });
                continue;
            }
            dispatch_turn(&mut state, &teams, turn.p1_action, turn.p2_action, &mut rng);
            turn_results.push(TurnResult {
                turn_number: idx + 1,
                state_after: StateSnapshotOrRolls::Single(extract_snapshot(&state)),
                legal_actions_after: extract_legal_actions_snapshot(&state),
            });
        }
        let output = ExecuteTurnsOutput {
            __req_id: req_id,
            name: scenario.name.clone(),
            mode: "execute_turns".to_string(),
            success: true,
            error: None,
            initial_state,
            turns: turn_results,
        };
        return serde_json::to_string(&output).unwrap();
    }

    for (idx, turn) in scenario.turns.iter().enumerate() {
        if state.phase == PHASE_GAME_OVER {
            turn_results.push(TurnResult {
                turn_number: idx + 1,
                state_after: StateSnapshotOrRolls::Single(extract_snapshot(&state)),
                legal_actions_after: extract_legal_actions_snapshot(&state),
            });
            continue;
        }

        let is_all_rolls = matches!(turn.rng_mode, RngMode::AllRolls);

        if is_all_rolls {
            let mut snapshots = Vec::with_capacity(16);
            let mut canonical_state = state;

            for roll in 0..16u32 {
                let mut clone = state;
                let mut rng = move |max: u32| -> u32 {
                    match max {
                        16 => roll.min(15),
                        24 => 23,
                        100 => 0,
                        2 => 0,
                        4 => 3,   // paralysis: can move
                        5 => 4,   // freeze: stays frozen
                        3 => 1,   // Protect stall ladder: no trigger
                        9 => 1,   // Protect 3rd consecutive
                        _ => 0,
                    }
                };
                dispatch_turn(&mut clone, &teams, turn.p1_action, turn.p2_action, &mut rng);
                snapshots.push(extract_snapshot(&clone));
                if roll == 15 { canonical_state = clone; }
            }

            state = canonical_state;
            turn_results.push(TurnResult {
                turn_number: idx + 1,
                state_after: StateSnapshotOrRolls::AllRolls(snapshots),
                legal_actions_after: extract_legal_actions_snapshot(&state),
            });
        } else {
            if let Err(e) = run_single_turn(&mut state, &teams, turn) {
                return error_json(req_id, &scenario.name, e);
            }

            turn_results.push(TurnResult {
                turn_number: idx + 1,
                state_after: StateSnapshotOrRolls::Single(extract_snapshot(&state)),
                legal_actions_after: extract_legal_actions_snapshot(&state),
            });
        }
    }

    let output = ExecuteTurnsOutput {
        __req_id: req_id,
        name: scenario.name.clone(),
        mode: "execute_turns".to_string(),
        success: true,
        error: None,
        initial_state,
        turns: turn_results,
    };
    serde_json::to_string(&output).unwrap()
}

fn run_single_turn(state: &mut BattleState, teams: &TeamData, turn: &TurnInput) -> Result<(), &'static str> {
    match turn.rng_mode {
        RngMode::ForceAll => {
            let mut rng = |max: u32| -> u32 { match max {
                16 => 15, 24 => 23, 100 => 0, 2 => 0,
                4 => 0,   // paralysis: 0 = fully paralyzed
                5 => 0,   // freeze: 0 = thaw
                3 => 0,   // Protect consecutive: 0 = trigger
                9 => 0,   // Protect 3rd consecutive
                _ => 0
            }};
            dispatch_turn(state, teams, turn.p1_action, turn.p2_action, &mut rng);
        }
        RngMode::ForceNone => {
            // The rng(16) damage roll is what separates an accuracy rng(100) from a secondary rng(100).
            let mut saw_damage_roll = false;
            let mut rng = move |max: u32| -> u32 {
                match max {
                    16 => { saw_damage_roll = true; 15 }
                    24 => 23,
                    100 => if saw_damage_roll { 99 } else { 0 },
                    2 => 0,
                    4 => 3,   // paralysis: non-zero = can move
                    5 => 4,   // freeze: non-zero = stays frozen
                    3 => 1,   // Protect consecutive: non-zero = no trigger
                    9 => 1,   // Protect 3rd consecutive: non-zero = fail
                    _ => 0,
                }
            };
            dispatch_turn(state, teams, turn.p1_action, turn.p2_action, &mut rng);
        }
        RngMode::MinRoll => {
            let mut rng = |max: u32| -> u32 { match max {
                16 => 0, 24 => 23, 100 => 0, 2 => 0,
                4 => 3, 5 => 4, 3 => 1, 9 => 1, // same as force_none for status/protect
                _ => 0
            }};
            dispatch_turn(state, teams, turn.p1_action, turn.p2_action, &mut rng);
        }
        RngMode::MaxRoll => {
            let mut rng = |max: u32| -> u32 { match max {
                16 => 15, 24 => 23, 100 => 0, 2 => 0,
                4 => 3, 5 => 4, 3 => 1, 9 => 1, // same as force_none for status/protect
                _ => 0
            }};
            dispatch_turn(state, teams, turn.p1_action, turn.p2_action, &mut rng);
        }
        RngMode::AllRolls => return Err("AllRolls must be handled by handle_execute_turns"),
        RngMode::Specific => {
            let ov = turn.rng_overrides.clone().unwrap_or(RngOverrides {
                damage_roll: 15, crit: false, secondary_trigger: true,
                accuracy_hit: true, speed_tie: 0, multi_hit_count: 0,
            });
            let mut saw_damage_roll = false;
            let mut rng = move |max: u32| -> u32 {
                match max {
                    16 => { saw_damage_roll = true; ov.damage_roll.min(15) }
                    24 => if ov.crit { 0 } else { 23 },
                    100 => {
                        if saw_damage_roll {
                            if ov.secondary_trigger { 0 } else { 99 }
                        } else {
                            if ov.accuracy_hit { 0 } else { 99 }
                        }
                    }
                    2 => ov.speed_tie.min(1),
                    3 | 4 => ov.multi_hit_count.min(max - 1),
                    _ => 0,
                }
            };
            dispatch_turn(state, teams, turn.p1_action, turn.p2_action, &mut rng);
        }
    }
    Ok(())
}

fn dispatch_turn(
    state: &mut BattleState,
    teams: &TeamData,
    p1_action: u8,
    p2_action: u8,
    rng: &mut impl FnMut(u32) -> u32,
) {
    match state.phase {
        PHASE_ACTIONS => {
            execute_turn(state, teams, p1_action, p2_action, rng);
        }
        PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
            execute_switch_turn(state, teams, p1_action, p2_action, rng);
        }
        _ => {}
    }
}

fn handle_calc_damage(scenario: &ScenarioInput, req_id: serde_json::Value) -> String {
    let params = scenario.calc_damage_params.as_ref().unwrap();
    let (state, _teams) = construct_state(scenario);

    let mut all_rolls = Vec::with_capacity(16);
    let mut min_dmg = u16::MAX;
    let mut max_dmg = 0u16;

    for roll in 0..16u32 {
        let mut rng = |max: u32| -> u32 {
            match max {
                16 => roll.min(15),
                24 => 23, // no crit
                100 => 0, // hit + trigger
                _ => 0,
            }
        };
        let res = pkmn_engine::state::calc::calc_damage(
            &state, params.atk_side, params.move_id,
            params.per_hit_accuracy, &mut rng,
        );
        min_dmg = min_dmg.min(res.damage);
        max_dmg = max_dmg.max(res.damage);
        all_rolls.push(DamageRollOut {
            roll, damage: res.damage, effectiveness: res.effectiveness,
            crit: res.crit, hits: res.hits, type_immune: res.type_immune,
            drain_heal: res.drain_heal, recoil_damage: res.recoil_damage,
            hits_substitute: res.hits_substitute, item_consumed: res.item_consumed,
        });
    }

    let mut crit_results = Vec::new();
    let mut min_crit = u16::MAX;
    let mut max_crit = 0u16;

    if params.include_crit {
        for roll in 0..16u32 {
            let mut rng = |max: u32| -> u32 {
                match max {
                    16 => roll.min(15),
                    24 => 0, // force crit
                    100 => 0,
                    _ => 0,
                }
            };
            let res = pkmn_engine::state::calc::calc_damage(
                &state, params.atk_side, params.move_id,
                params.per_hit_accuracy, &mut rng,
            );
            min_crit = min_crit.min(res.damage);
            max_crit = max_crit.max(res.damage);
            crit_results.push(DamageRollOut {
                roll, damage: res.damage, effectiveness: res.effectiveness,
                crit: res.crit, hits: res.hits, type_immune: res.type_immune,
                drain_heal: res.drain_heal, recoil_damage: res.recoil_damage,
                hits_substitute: res.hits_substitute, item_consumed: res.item_consumed,
            });
        }
    } else {
        min_crit = 0;
        max_crit = 0;
    }

    let output = CalcDamageOutput {
        __req_id: req_id,
        name: scenario.name.clone(),
        mode: "calc_damage".to_string(),
        success: true,
        error: None,
        results: CalcDamageResults {
            all_rolls,
            crit_results,
            min_damage: min_dmg,
            max_damage: max_dmg,
            min_damage_crit: min_crit,
            max_damage_crit: max_crit,
        },
    };
    serde_json::to_string(&output).unwrap()
}

fn handle_legal_actions(scenario: &ScenarioInput, req_id: serde_json::Value) -> String {
    let (state, _teams) = construct_state(scenario);
    let both_sides = extract_legal_actions_snapshot(&state);

    let actions = match scenario.legal_actions_params.as_ref() {
        Some(params) => {
            let raw: Vec<u8> = legal_actions(&state, params.side).as_slice().to_vec();
            let decoded: Vec<DecodedAction> = raw.iter().map(|&a| {
                decode_action_for_output(&state, params.side, a)
            }).collect();
            let count = raw.len();
            Some(DecodedActions { raw, decoded, count })
        }
        None => None,
    };

    let output = LegalActionsOutput {
        __req_id: req_id,
        name: scenario.name.clone(),
        mode: "legal_actions".to_string(),
        success: true,
        error: None,
        actions,
        both_sides,
    };
    serde_json::to_string(&output).unwrap()
}

fn decode_action_for_output(state: &BattleState, side: usize, action: u8) -> DecodedAction {
    match action {
        0..=3 => {
            let moves = effective_moves(state, side);
            DecodedAction {
                action, kind: "move".to_string(),
                move_slot: Some(action), move_id: Some(moves[action as usize]),
                target_slot: None, target_species: None,
            }
        }
        4..=9 => {
            let slot = (action - 4) as usize;
            DecodedAction {
                action, kind: "switch".to_string(),
                move_slot: None, move_id: None,
                target_slot: Some(slot as u8),
                target_species: Some(state.sides[side].team[slot].species_id),
            }
        }
        ACTION_TERA => {
            let moves = effective_moves(state, side);
            DecodedAction {
                action, kind: "tera".to_string(),
                move_slot: None, move_id: Some(moves[0]),
                target_slot: None, target_species: None,
            }
        }
        _ => {
            DecodedAction {
                action, kind: "struggle".to_string(),
                move_slot: None, move_id: None,
                target_slot: None, target_species: None,
            }
        }
    }
}

fn output_error(name: &str, msg: &str) {
    let err = ErrorOutput {
        __req_id: serde_json::Value::Null,
        name: name.to_string(),
        success: false,
        error: msg.to_string(),
    };
    eprintln!("{}", serde_json::to_string(&err).unwrap());
    print!("{}", serde_json::to_string(&err).unwrap());
}

// The returned string must stay single-line; the server loop is newline-delimited.
fn error_json(req_id: serde_json::Value, name: &str, msg: &str) -> String {
    let err = ErrorOutput {
        __req_id: req_id,
        name: name.to_string(),
        success: false,
        error: msg.to_string(),
    };
    let s = serde_json::to_string(&err).unwrap();
    eprintln!("{}", s);
    s
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn spawn_stdin_watchdog(idle_timeout_secs: u64, last_input_ms: Arc<AtomicU64>) {
    let timeout_ms = idle_timeout_secs.saturating_mul(1000);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let now = now_ms();
        let last = last_input_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) > timeout_ms {
            std::process::exit(0);
        }
    });
}

fn run_one(input: &str) -> String {
    let raw: serde_json::Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => return error_json(serde_json::Value::Null, "parse",
                                    &format!("JSON parse error: {}", e)),
    };
    let req_id = raw.get("__req_id").cloned().unwrap_or(serde_json::Value::Null);

    let scenario: ScenarioInput = match serde_json::from_value(raw) {
        Ok(s) => s,
        Err(e) => return error_json(req_id, "parse",
                                    &format!("JSON parse error: {}", e)),
    };
    if let Err(e) = validate(&scenario) {
        return error_json(req_id, &scenario.name, &e);
    }
    match scenario.mode {
        ExecutionMode::ExecuteTurns => handle_execute_turns(&scenario, req_id),
        ExecutionMode::CalcDamage   => handle_calc_damage(&scenario, req_id),
        ExecutionMode::LegalActions => handle_legal_actions(&scenario, req_id),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let server_mode = args.iter().any(|a| a == "--server-mode");


    if server_mode {
        let mut idle_timeout_secs: u64 = 60;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--idle-timeout-secs" && i + 1 < args.len() {
                if let Ok(n) = args[i + 1].parse::<u64>() { idle_timeout_secs = n; }
            }
            i += 1;
        }

        let last_input_ms = Arc::new(AtomicU64::new(now_ms()));
        spawn_stdin_watchdog(idle_timeout_secs, Arc::clone(&last_input_ms));

        let stdin = std::io::stdin();
        let mut stdin_locked = stdin.lock();
        let stdout = std::io::stdout();
        let mut stdout_locked = stdout.lock();
        let mut line = String::new();

        loop {
            line.clear();
            match stdin_locked.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => last_input_ms.store(now_ms(), Ordering::Relaxed),
                Err(_) => std::process::exit(1),
            }
            let trimmed = line.trim_end_matches(&['\n', '\r'][..]);
            if trimmed.is_empty() { continue; }
            let response = run_one(trimmed);
            let _ = writeln!(stdout_locked, "{}", response);
            let _ = stdout_locked.flush();
        }
    } else {
        let input = std::io::read_to_string(std::io::stdin()).unwrap_or_else(|e| {
            output_error("stdin", &format!("Failed to read stdin: {}", e));
            std::process::exit(1);
        });

        let scenario: ScenarioInput = serde_json::from_str(&input).unwrap_or_else(|e| {
            output_error("parse", &format!("JSON parse error: {}", e));
            std::process::exit(1);
        });

        if let Err(e) = validate(&scenario) {
            output_error(&scenario.name, &e);
            std::process::exit(1);
        }

        let json = match scenario.mode {
            ExecutionMode::ExecuteTurns => handle_execute_turns(&scenario, serde_json::Value::Null),
            ExecutionMode::CalcDamage => handle_calc_damage(&scenario, serde_json::Value::Null),
            ExecutionMode::LegalActions => handle_legal_actions(&scenario, serde_json::Value::Null),
        };

        println!("{}", json);
    }
}

#[cfg(test)]
mod tests {
    use super::sampled_rng;

    // Inlined copy of the engine battle driver's canonical LCG.
    fn reference(seed: u64) -> impl FnMut(u32) -> u32 {
        let mut s = seed;
        move |max| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as u32) % max.max(1)
        }
    }

    // χ²(0.999) upper-tail critical values by df (α=0.001).
    fn chi2_crit(df: usize) -> f64 {
        match df {
            1 => 10.828,
            2 => 13.816,
            3 => 16.266,
            4 => 18.467,
            15 => 37.697,
            23 => 49.728,
            99 => 148.30,
            _ => panic!("no critical value pinned for df={}", df),
        }
    }

    fn chi2_uniform(counts: &[u64]) -> f64 {
        let n: u64 = counts.iter().sum();
        let k = counts.len() as f64;
        let expected = n as f64 / k;
        counts
            .iter()
            .map(|&c| {
                let d = c as f64 - expected;
                d * d / expected
            })
            .sum()
    }

    #[test]
    fn lcg_first_draws_match_reference() {
        for &seed in &[0u64, 1, 42, u64::MAX] {
            let mut a = sampled_rng(seed);
            let mut b = reference(seed);
            for &max in [2u32, 3, 4, 5, 9, 10, 16, 24, 100, 1, 0].iter().cycle().take(64) {
                assert_eq!(a(max), b(max), "divergence at seed {} max {}", seed, max);
            }
        }
    }

    #[test]
    fn lcg_matches_engine_battle_driver_exactly() {
        let mut a = sampled_rng(0xDEAD_BEEF_CAFE_F00D);
        let mut b = reference(0xDEAD_BEEF_CAFE_F00D);
        for i in 0..10_000u32 {
            let max = (i % 64) + 1;
            assert_eq!(a(max), b(max), "draw {} diverged", i);
        }
    }

    #[test]
    fn lcg_advance_then_mod_order() {
        let seed = 12345u64;
        let mut r = sampled_rng(seed);
        let first = r(1000);
        let advanced = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        assert_eq!(first, ((advanced >> 33) as u32) % 1000);
        assert_ne!(first, ((seed >> 33) as u32) % 1000, "first draw must be post-advance");
    }

    #[test]
    fn lcg_max_zero_and_range() {
        let mut r = sampled_rng(7);
        for _ in 0..1000 {
            assert_eq!(r(0), 0);
        } // max=0 guard, no panic
        let mut a = sampled_rng(7);
        let mut b = reference(7);
        for i in 0..2000u32 {
            let max = if i % 3 == 0 { 0 } else { 16 };
            assert_eq!(a(max), b(max), "rng(0) interleave desynced at {}", i);
        }
    }

    #[test]
    fn lcg_range_bounds_representative_d() {
        for d in [2u32, 3, 4, 5, 9, 10, 16, 24, 100] {
            let mut r = sampled_rng(0xABCD_1234);
            let (mut lo, mut hi) = (u32::MAX, 0u32);
            for _ in 0..200_000 {
                let v = r(d);
                assert!(v < d, "value {} >= D {}", v, d);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert_eq!(lo, 0, "min not 0 for D={}", d);
            assert_eq!(hi, d - 1, "max not D-1 for D={}", d);
        }
    }

    #[test]
    fn lcg_no_modulo_bias_visible_at_used_d() {
        for d in [2u32, 3, 4, 5, 16, 24] {
            let mut r = sampled_rng(0x5EED_0001);
            let mut counts = vec![0u64; d as usize];
            for _ in 0..2_000_000 {
                counts[r(d) as usize] += 1;
            }
            let chi2 = chi2_uniform(&counts);
            assert!(
                chi2 < chi2_crit(d as usize - 1),
                "D={} chi2={} exceeded crit",
                d,
                chi2
            );
        }
    }

    fn first_draw(seed: u64, d: u32) -> u32 {
        sampled_rng(seed)(d)
    }

    #[test]
    fn seed_sweep_low_d_uniform_at_used_granularity() {
        const K: u64 = 50_000;
        for d in [2u32, 3, 100] {
            let mut counts = vec![0u64; d as usize];
            for k in 0..K {
                counts[first_draw(k, d) as usize] += 1;
            }
            let chi2 = chi2_uniform(&counts);
            assert!(
                chi2 < chi2_crit(d as usize - 1),
                "D={} first-draw chi2={} exceeded crit",
                d,
                chi2
            );
        }
    }

    // Sequential seeds share a first-draw low-bit lattice, so the comparator mixes before seeding.
    #[test]
    fn adjacent_seeds_decorrelate() {
        const N: u64 = 200_000;
        let ones: u64 = (0..N).map(|s| first_draw(s, 2) as u64).sum();
        let p = ones as f64 / N as f64;
        assert!((p - 0.5).abs() < 0.01, "marginal first rng(2) {} not ~0.5", p);

        fn sig(seed: u64) -> u32 {
            let mut r = sampled_rng(seed);
            let mut h = 0u64;
            for _ in 0..8 {
                h ^= r(1_000_000) as u64;
            }
            (h & 1) as u32
        }
        let mut eq = 0u64;
        let mut marg = 0u64;
        for s in 0..N {
            let a = sig(s);
            marg += a as u64;
            if a == sig(s + 1) {
                eq += 1;
            }
        }
        let frac = eq as f64 / N as f64;
        let sm = marg as f64 / N as f64;
        assert!((sm - 0.5).abs() < 0.01, "signature marginal {} not ~0.5", sm);
        assert!((frac - 0.5).abs() < 0.01, "stream-signature lag-1 match {} not ~0.5", frac);
    }

    #[test]
    fn seed_sweep_no_period_within_k() {
        use std::collections::HashSet;
        const K: u64 = 100_000;
        const L: usize = 4;
        let mut seen = HashSet::with_capacity(K as usize);
        for k in 0..K {
            let mut r = sampled_rng(k);
            let mut h = 0u64;
            for _ in 0..L {
                h = h.wrapping_mul(1_000_003).wrapping_add(r(1_000_000) as u64);
            }
            assert!(seen.insert(h), "seed {} aliased an earlier stream", k);
        }
        assert_eq!(seen.len(), K as usize);
    }

    #[test]
    fn multi_draw_joint_uniformity() {
        const K: u64 = 200_000;
        let mut cells = [0u64; 4];
        for k in 0..K {
            let mut r = sampled_rng(k);
            let a = r(2);
            let b = r(2);
            cells[(a * 2 + b) as usize] += 1;
        }
        let chi2 = chi2_uniform(&cells);
        assert!(chi2 < chi2_crit(3), "joint chi2={} exceeded crit", chi2);
    }
}
