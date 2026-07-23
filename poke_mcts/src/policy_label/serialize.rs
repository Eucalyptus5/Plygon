use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use pkmn_engine::state::data_bridge;
use pkmn_engine::state::{
    ActiveMon, BattleState, FieldState, MonSlot, SideState, MON_FLAG_TERASTALLIZED,
    PHASE_SWITCH_BOTH, PHASE_SWITCH_P1, PHASE_SWITCH_P2, STATUS_SLEEP, TERA_TYPE_NORMAL,
    VOL_AQUA_RING, VOL_BOUND, VOL_DESTINY_BOND, VOL_FLASH_FIRE, VOL_FOCUS_ENERGY, VOL_GRUDGE,
    VOL_IMPRISON, VOL_INGRAIN, VOL_LASER_FOCUS, VOL_LEECH_SEED, VOL_MAGNET_RISE, VOL_MINIMIZE,
    VOL_PERISH_SONG, VOL_SMACKED_DOWN, VOL_SUBSTITUTE, VOL_TORMENT, VOL_UNBURDEN, VOL_YAWN,
};

fn inverted(json: &str) -> HashMap<u16, String> {
    let by_name: HashMap<String, u16> = serde_json::from_str(json).unwrap();
    by_name.into_iter().map(|(k, v)| (v, k)).collect()
}

fn species_name_table() -> &'static HashMap<u16, String> {
    static MAP: OnceLock<HashMap<u16, String>> = OnceLock::new();
    MAP.get_or_init(|| inverted(include_str!("../../../testing_plan/id_maps/species_map.json")))
}

fn move_name_table() -> &'static HashMap<u16, String> {
    static MAP: OnceLock<HashMap<u16, String>> = OnceLock::new();
    MAP.get_or_init(|| inverted(include_str!("../../../testing_plan/id_maps/move_map.json")))
}

fn ability_name_table() -> &'static HashMap<u16, String> {
    static MAP: OnceLock<HashMap<u16, String>> = OnceLock::new();
    MAP.get_or_init(|| inverted(include_str!("../../../testing_plan/id_maps/ability_map.json")))
}

fn item_name_table() -> &'static HashMap<u16, String> {
    static MAP: OnceLock<HashMap<u16, String>> = OnceLock::new();
    MAP.get_or_init(|| inverted(include_str!("../../../testing_plan/id_maps/item_map.json")))
}

const TYPE_TOKENS: [&str; 19] = [
    "NORMAL", "FIRE", "WATER", "ELECTRIC", "GRASS", "ICE", "FIGHTING", "POISON", "GROUND",
    "FLYING", "PSYCHIC", "BUG", "ROCK", "GHOST", "DRAGON", "DARK", "STEEL", "FAIRY", "STELLAR",
];

fn type_token(t: u8) -> &'static str {
    *TYPE_TOKENS.get(t as usize).unwrap_or(&"TYPELESS")
}

fn status_token(status: u8) -> &'static str {
    match status {
        0 => "NONE",
        1 => "BURN",
        2 => "PARALYZE",
        3 => "POISON",
        4 => "TOXIC",
        5 => "SLEEP",
        6 => "FREEZE",
        _ => "NONE",
    }
}

fn weather_token(w: u8) -> &'static str {
    match w {
        0 => "NONE",
        1 => "SUN",
        2 => "RAIN",
        3 => "SAND",
        4 => "SNOW",
        5 => "HARSHSUN",
        6 => "HEAVYRAIN",
        _ => "NONE",
    }
}

fn terrain_token(t: u8) -> &'static str {
    match t {
        0 => "NONE",
        1 => "ELECTRICTERRAIN",
        2 => "GRASSYTERRAIN",
        3 => "PSYCHICTERRAIN",
        4 => "MISTYTERRAIN",
        _ => "NONE",
    }
}

pub fn species_token(id: u16) -> String {
    match species_name_table().get(&id) {
        Some(n) => n.to_ascii_uppercase(),
        None => "NONE".to_string(),
    }
}

pub fn move_token(id: u16) -> String {
    if id == 0 {
        return "NONE".to_string();
    }
    match move_name_table().get(&id) {
        Some(n) => n.to_ascii_uppercase(),
        None => "NONE".to_string(),
    }
}

fn ability_token(id: u16) -> String {
    if id == 0 {
        return "NONE".to_string();
    }
    match ability_name_table().get(&id) {
        Some(n) => n.to_ascii_uppercase(),
        None => "NONE".to_string(),
    }
}

fn item_token(id: u16) -> String {
    if id == 0 {
        return "NONE".to_string();
    }
    match item_name_table().get(&id) {
        Some(n) => n.to_ascii_uppercase(),
        None => "NONE".to_string(),
    }
}

fn tera_token(tera_type: u8) -> &'static str {
    if tera_type == 0 {
        "TYPELESS"
    } else if tera_type == TERA_TYPE_NORMAL {
        "NORMAL"
    } else {
        type_token(tera_type)
    }
}

fn weight_kg(weight_tenths: u16) -> String {
    format!("{}", weight_tenths as f32 / 10.0)
}

// Recovers per-stat EVs such that poke-engine's nature-blind 31-IV formula
// reproduces the recorded final stats; an absent boosting/reducing nature is
// folded into the recovered EV. Low-level rounding ties resolve toward the
// randbats-standard 85 (ev/4 = 21) so a neutral mon restats exactly post
// form change.
pub fn back_solve_evs(mon: &MonSlot) -> [u8; 6] {
    let sp = data_bridge::species(mon.species_id);
    let bases = [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe];
    let l = mon.level as u32;
    let mut evs = [0u8; 6];
    for i in 0..6 {
        let target = if i == 0 { mon.max_hp } else { mon.stats[i - 1] } as u32;
        let base = bases[i] as u32;
        let mut best_d = u32::MAX;
        let mut best_tie = u32::MAX;
        let mut best_ev = 0u8;
        for ev4 in 0..=63u32 {
            let raw = (2 * base + 31 + ev4) * l / 100;
            let s = if i == 0 { raw + l + 10 } else { raw + 5 };
            let d = s.abs_diff(target);
            let tie = ev4.abs_diff(21);
            if d < best_d || (d == best_d && tie < best_tie) {
                best_d = d;
                best_tie = tie;
                best_ev = (ev4 * 4) as u8;
            }
        }
        evs[i] = best_ev;
    }
    evs
}

fn pokemon(out: &mut String, mon: &MonSlot, active: Option<&ActiveMon>, tera_mark: bool) {
    let sp = data_bridge::species(mon.species_id);
    let type1 = sp.type1 as u8;
    let type2 = sp.type2 as u8;
    let type2_tok = if type2 == type1 { "TYPELESS" } else { type_token(type2) };

    let _ = write!(out, "{},{},", species_token(mon.species_id), mon.level);
    let _ = write!(out, "{},{},", type_token(type1), type2_tok);
    let _ = write!(out, "{},{},", type_token(type1), type2_tok);
    let _ = write!(out, "{},{},", mon.current_hp, mon.max_hp);
    let _ = write!(out, "{},{},", ability_token(mon.ability_id), ability_token(mon.ability_id));
    let _ = write!(out, "{},", item_token(mon.item_id));
    let _ = write!(out, "SERIOUS,");
    let e = back_solve_evs(mon);
    let _ = write!(out, "{};{};{};{};{};{},", e[0], e[1], e[2], e[3], e[4], e[5]);
    let s = mon.stats;
    let _ = write!(out, "{},{},{},{},{},", s[0], s[1], s[2], s[3], s[4]);
    let _ = write!(out, "{},", status_token(mon.status));
    let sleep_turns = if mon.status == STATUS_SLEEP { mon.status_counter } else { 0 };
    let _ = write!(out, "0,{},", sleep_turns);
    let _ = write!(out, "{},", weight_kg(sp.weight));

    for slot in 0..4 {
        let mid = mon.moves[slot];
        if mid == 0 {
            let _ = write!(out, "NONE;false;32,");
        } else {
            let disabled = match active {
                Some(a) => a.disabled_move != 0 && a.disabled_move == mid,
                None => false,
            };
            let _ = write!(out, "{};{};{},", move_token(mid), disabled, mon.pp[slot]);
        }
    }

    // raw flag, not the hp-gated accessor: poke-engine's can_use_tera needs the
    // spent tera visible even on a fainted mon
    let terastallized = tera_mark || mon.flags & MON_FLAG_TERASTALLIZED != 0;
    let _ = write!(out, "{},{}", terastallized, tera_token(mon.tera_type));
}

fn volatile_set(active: &ActiveMon) -> String {
    let mut toks: Vec<&str> = Vec::new();
    let f = active.volatile_flags;
    if f & VOL_SUBSTITUTE != 0 { toks.push("SUBSTITUTE"); }
    if f & VOL_LEECH_SEED != 0 { toks.push("LEECHSEED"); }
    if f & VOL_FOCUS_ENERGY != 0 { toks.push("FOCUSENERGY"); }
    if f & VOL_TORMENT != 0 { toks.push("TORMENT"); }
    if f & VOL_IMPRISON != 0 { toks.push("IMPRISON"); }
    if f & VOL_MINIMIZE != 0 { toks.push("MINIMIZE"); }
    if f & VOL_AQUA_RING != 0 { toks.push("AQUARING"); }
    if f & VOL_INGRAIN != 0 { toks.push("INGRAIN"); }
    if f & VOL_MAGNET_RISE != 0 { toks.push("MAGNETRISE"); }
    if f & VOL_DESTINY_BOND != 0 { toks.push("DESTINYBOND"); }
    if f & VOL_YAWN != 0 { toks.push("YAWN"); }
    if f & VOL_GRUDGE != 0 { toks.push("GRUDGE"); }
    if f & VOL_LASER_FOCUS != 0 { toks.push("LASERFOCUS"); }
    if f & VOL_FLASH_FIRE != 0 { toks.push("FLASHFIRE"); }
    if f & VOL_UNBURDEN != 0 { toks.push("UNBURDEN"); }
    if f & VOL_SMACKED_DOWN != 0 { toks.push("SMACKDOWN"); }
    if f & VOL_BOUND != 0 { toks.push("PARTIALLYTRAPPED"); }
    if active.confusion_turns > 0 { toks.push("CONFUSION"); }
    if active.taunt_turns > 0 { toks.push("TAUNT"); }
    if active.encore_turns > 0 && active.encore_move != 0 { toks.push("ENCORE"); }
    if active.disable_turns > 0 { toks.push("DISABLE"); }
    if f & VOL_PERISH_SONG != 0 {
        match active.perish_count {
            1 => toks.push("PERISH1"),
            2 => toks.push("PERISH2"),
            3 => toks.push("PERISH3"),
            _ => toks.push("PERISH4"),
        }
    }
    if toks.is_empty() {
        String::new()
    } else {
        let mut s = toks.join(":");
        s.push(':');
        s
    }
}

// taunt/encore counters are turns-remaining; poke-engine stores turns-elapsed
fn dur_elapsed(remaining: u8, total: u8) -> u8 {
    if remaining == 0 { 0 } else { total.saturating_sub(remaining) }
}

fn side(out: &mut String, side: &SideState, force_switch: bool) {
    let ai = side.active_index as usize;
    // world reconstruction copies the tera flag hp-gated, so a fainted tera mon
    // loses it; the side-level tera-used bit is the surviving witness, carried
    // here by marking one fainted mon (inert beyond can_use_tera)
    let tera_lost = side._padding[0] & 1 != 0
        && !side.team.iter().any(|m| m.flags & MON_FLAG_TERASTALLIZED != 0);
    let mark_slot = if tera_lost {
        side.team.iter().position(|m| m.species_id != 0 && m.current_hp == 0)
    } else {
        None
    };
    for i in 0..6 {
        if i > 0 {
            out.push('=');
        }
        let active = if i == ai { Some(&side.active) } else { None };
        pokemon(out, &side.team[i], active, mark_slot == Some(i));
    }

    let _ = write!(out, "={}", side.active_index);

    let sc = &side.side_conditions;
    let aurora_veil = sc.aurora_veil_turns;
    let light_screen = sc.light_screen_turns;
    let reflect = sc.reflect_turns;
    let safeguard = sc.safeguard_turns();
    let mist = sc.mist_turns();
    let spikes = sc.spikes;
    let stealth_rock = (sc.hazard_flags & pkmn_engine::state::HAZARD_STEALTH_ROCK != 0) as u8;
    let sticky_web = (sc.hazard_flags & pkmn_engine::state::HAZARD_STICKY_WEB != 0) as u8;
    let tailwind = sc.tailwind_turns;
    let toxic_spikes = sc.toxic_spikes;
    let lucky_chant = sc.lucky_chant_turns();
    let healing_wish = sc.has_healing_wish() as u8;
    let lunar_dance = sc.has_lunar_dance() as u8;
    let toxic_count = side.active.toxic_counter;
    let _ = write!(
        out,
        "={};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{}",
        aurora_veil,
        0,
        healing_wish,
        light_screen,
        lucky_chant,
        lunar_dance,
        0,
        mist,
        0,
        0,
        reflect,
        safeguard,
        spikes,
        stealth_rock,
        sticky_web,
        tailwind,
        toxic_count,
        toxic_spikes,
        0,
    );

    let _ = write!(out, "={}", volatile_set(&side.active));

    let a = &side.active;
    let _ = write!(
        out,
        "={};{};{};{};{};{}",
        a.confusion_turns, dur_elapsed(a.encore_turns, 3), 0, 0, dur_elapsed(a.taunt_turns, 3), 0,
    );

    let _ = write!(out, "={}", side.active.substitute_hp);

    let b = side.active.boosts;
    let _ = write!(out, "={}={}={}={}={}={}={}", b[0], b[1], b[2], b[3], b[4], b[5], b[6]);

    let _ = write!(out, "={}={}", sc.wish_turns, sc.wish_hp);

    let _ = write!(out, "=0=0");

    let _ = write!(out, "={force_switch}");
    let _ = write!(out, "=NONE");
    let _ = write!(out, "=false=false=false");
    let _ = write!(out, "={}", last_used_move(side));
    let _ = write!(out, "=false");
}

fn last_used_move(side: &SideState) -> String {
    let lm = side.active.last_move;
    if lm == 0 {
        return "move:none".to_string();
    }
    let active = &side.team[side.active_index as usize];
    for slot in 0..4 {
        if active.moves[slot] == lm {
            return format!("move:{slot}");
        }
    }
    "move:none".to_string()
}

fn field_tail(out: &mut String, field: &FieldState) {
    let _ = write!(
        out,
        "{};{}/{};{}/{};{}/false",
        weather_token(field.weather),
        field.weather_turns as i16,
        terrain_token(field.terrain),
        field.terrain_turns as i16,
        if field.trick_room_turns > 0 { "true" } else { "false" },
        field.trick_room_turns as i16,
    );
}

fn forced(phase: u8, physical_side: usize) -> bool {
    phase == PHASE_SWITCH_BOTH
        || (phase == PHASE_SWITCH_P1 && physical_side == 0)
        || (phase == PHASE_SWITCH_P2 && physical_side == 1)
}

pub fn serialize_state(state: &BattleState, our_side: usize) -> String {
    let opp = 1 - our_side;
    let mut out = String::with_capacity(4096);

    side(&mut out, &state.sides[our_side], forced(state.phase, our_side));
    out.push('/');
    side(&mut out, &state.sides[opp], forced(state.phase, opp));
    out.push('/');
    field_tail(&mut out, &state.field);

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkmn_engine::state::team_builder::{build_mon, MonBuildInput};

    fn ids(species: &str, ability: &str, item: &str, moves: [&str; 4]) -> MonBuildInput {
        let by_name = |json: &str, k: &str| -> u16 {
            let m: HashMap<String, u16> = serde_json::from_str(json).unwrap();
            m[k]
        };
        MonBuildInput {
            species_id: by_name(include_str!("../../../testing_plan/id_maps/species_map.json"), species),
            ability_id: by_name(include_str!("../../../testing_plan/id_maps/ability_map.json"), ability),
            item_id: by_name(include_str!("../../../testing_plan/id_maps/item_map.json"), item),
            moves: moves.map(|n| by_name(include_str!("../../../testing_plan/id_maps/move_map.json"), n)),
            ivs: [31; 6],
            evs: [85; 6],
            nature: 24,
            level: 100,
            tera_type: 0,
            is_female: false,
        }
    }

    #[test]
    fn neutral_record_serializes_with_equivalent_backsolved_evs() {
        let input = ids(
            "greattusk",
            "protosynthesis",
            "boosterenergy",
            ["headlongrush", "closecombat", "rapidspin", "knockoff"],
        );
        let (mon, _) = build_mon(&input);
        let mut out = String::new();
        pokemon(&mut out, &mon, None, false);
        assert_eq!(
            out,
            "GREATTUSK,100,GROUND,FIGHTING,GROUND,FIGHTING,392,392,PROTOSYNTHESIS,\
PROTOSYNTHESIS,BOOSTERENERGY,SERIOUS,84;84;84;84;84;84,319,319,163,163,231,NONE,0,0,320,\
HEADLONGRUSH;false;8,CLOSECOMBAT;false;8,RAPIDSPIN;false;64,KNOCKOFF;false;32,false,TYPELESS"
        );
    }

    #[test]
    fn backsolve_folds_boosting_nature_into_recovered_ev() {
        let mut input = ids(
            "palafin",
            "zerotohero",
            "leftovers",
            ["jetpunch", "closecombat", "icepunch", "bulkup"],
        );
        input.nature = 2;
        let (mon, _) = build_mon(&input);
        assert_eq!(mon.stats[0], 216);
        let e = back_solve_evs(&mon);
        assert_eq!(e[1], 160);
        let atk = (2 * 70 + 31 + e[1] as u32 / 4) * 100 / 100 + 5;
        assert_eq!(atk as u16, mon.stats[0]);
        let hero_atk = (2 * 160 + 31 + e[1] as u32 / 4) * 100 / 100 + 5;
        assert_eq!(hero_atk, 396);
        let hp = (2 * 100 + 31 + e[0] as u32 / 4) * 100 / 100 + 100 + 10;
        assert_eq!(hp as u16, mon.max_hp);
    }

    #[test]
    fn weight_kg_matches_rust_float_fmt() {
        assert_eq!(weight_kg(3200), "320");
        assert_eq!(weight_kg(3807), "380.7");
        assert_eq!(weight_kg(51), "5.1");
        assert_eq!(weight_kg(795), "79.5");
    }

    #[test]
    fn dur_elapsed_maps_remaining_to_elapsed() {
        assert_eq!(dur_elapsed(3, 3), 0);
        assert_eq!(dur_elapsed(2, 3), 1);
        assert_eq!(dur_elapsed(1, 3), 2);
        assert_eq!(dur_elapsed(0, 3), 0);
    }
}
