use pkmn_engine::data::moves::{MoveFlags, MoveTarget};
use pkmn_engine::data::items::ItemFlag;
use pkmn_engine::data::types::{EFFECTIVENESS, NUM_TYPES};
use pkmn_engine::data::{GEN_ITEMS, GEN_MOVES, GEN_MOVE_META, GEN_SPECIES, TOTAL_SPECIES};
use serde::Serialize;

#[derive(Serialize)]
struct DumpOutput {
    moves: Vec<MoveOut>,
    species: Vec<SpeciesOut>,
    items: Vec<ItemOut>,
    type_chart: [[u8; NUM_TYPES]; NUM_TYPES],
    constants: Constants,
}

#[derive(Serialize)]
struct MoveOut {
    id: usize,
    base_power: u8,
    accuracy: u8,
    category: String,
    move_type: String,
    priority: i8,
    pp: u8,
    flags: u16,
    flag_names: Vec<&'static str>,
    crit_ratio: u8,
    drain: i8,
    multihit_lo: u8,
    multihit_hi: u8,
    secondary_chance: u8,
    secondary_stat: i8,
    secondary_status: u8,
    effect: String,
    self_effect: String,
    var_power: String,
    target: String,
}

#[derive(Serialize)]
struct SpeciesOut {
    id: usize,
    hp: u8,
    atk: u8,
    def: u8,
    spa: u8,
    spd: u8,
    spe: u8,
    type1: String,
    type2: String,
    weight: u16,
}

#[derive(Serialize)]
struct ItemOut {
    id: usize,
    flags: u64,
    flag_names: Vec<&'static str>,
    type_param: u8,
    power_param: u8,
    forme_species: u16,
    is_none: bool,
}

#[derive(Serialize)]
struct Constants {
    total_moves: usize,
    total_species: usize,
    total_items: usize,
    forme_offset: usize,
}

fn target_name(t: MoveTarget) -> &'static str {
    match t {
        MoveTarget::Normal => "Normal",
        MoveTarget::Self_ => "Self",
        MoveTarget::AllAdjacentFoes => "AllAdjacentFoes",
        MoveTarget::AllAdjacent => "AllAdjacent",
        MoveTarget::AllyOrSelf => "AllyOrSelf",
        MoveTarget::Any => "Any",
        MoveTarget::FoeSide => "FoeSide",
        MoveTarget::AllySide => "AllySide",
        MoveTarget::All => "All",
    }
}

fn move_flag_names(flags: u16) -> Vec<&'static str> {
    const NAMES: [(u16, &str); 16] = [
        (MoveFlags::CONTACT, "contact"),
        (MoveFlags::SOUND, "sound"),
        (MoveFlags::PUNCH, "punch"),
        (MoveFlags::PULSE, "pulse"),
        (MoveFlags::BITE, "bite"),
        (MoveFlags::POWDER, "powder"),
        (MoveFlags::DANCE, "dance"),
        (MoveFlags::WIND, "wind"),
        (MoveFlags::SLICE, "slice"),
        (MoveFlags::PROTECT, "protect"),
        (MoveFlags::REFLECTABLE, "reflectable"),
        (MoveFlags::RECHARGE, "recharge"),
        (MoveFlags::CHARGE, "charge"),
        (MoveFlags::HEAL, "heal"),
        (MoveFlags::BYPASSSUB, "bypasssub"),
        (MoveFlags::BULLET, "bullet"),
    ];
    NAMES.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, name)| *name).collect()
}

fn item_flag_names(flags: u64) -> Vec<&'static str> {
    const NAMES: [(u64, &str); 41] = [
        (ItemFlag::CHOICE_ATK, "choice_atk"),
        (ItemFlag::CHOICE_SPA, "choice_spa"),
        (ItemFlag::CHOICE_SPE, "choice_spe"),
        (ItemFlag::ASSAULT_VEST, "assault_vest"),
        (ItemFlag::EVIOLITE, "eviolite"),
        (ItemFlag::LIFE_ORB, "life_orb"),
        (ItemFlag::EXPERT_BELT, "expert_belt"),
        (ItemFlag::TYPE_BOOST, "type_boost"),
        (ItemFlag::RESIST_BERRY, "resist_berry"),
        (ItemFlag::METRONOME, "metronome"),
        (ItemFlag::CRIT_BOOST, "crit_boost"),
        (ItemFlag::WIDE_LENS, "wide_lens"),
        (ItemFlag::FOCUS_SASH, "focus_sash"),
        (ItemFlag::AIR_BALLOON, "air_balloon"),
        (ItemFlag::SAFETY_GOGGLES, "safety_goggles"),
        (ItemFlag::ROCKY_HELMET, "rocky_helmet"),
        (ItemFlag::LEFTOVERS, "leftovers"),
        (ItemFlag::BLACK_SLUDGE, "black_sludge"),
        (ItemFlag::FLAME_ORB, "flame_orb"),
        (ItemFlag::TOXIC_ORB, "toxic_orb"),
        (ItemFlag::HAZARD_IMMUNE, "hazard_immune"),
        (ItemFlag::TRAP_IMMUNE, "trap_immune"),
        (ItemFlag::EXTENDS_SCREENS, "extends_screens"),
        (ItemFlag::BINDING_BOOST, "binding_boost"),
        (ItemFlag::TERRAIN_SEED, "terrain_seed"),
        (ItemFlag::IS_BERRY, "is_berry"),
        (ItemFlag::PINCH_BERRY, "pinch_berry"),
        (ItemFlag::MEGA_STONE, "mega_stone"),
        (ItemFlag::Z_CRYSTAL, "z_crystal"),
        (ItemFlag::CONSUMABLE, "consumable"),
        (ItemFlag::GEM, "gem"),
        (ItemFlag::POWER_HERB, "power_herb"),
        (ItemFlag::LOADED_DICE, "loaded_dice"),
        (ItemFlag::COVERT_CLOAK, "covert_cloak"),
        (ItemFlag::CLEAR_AMULET, "clear_amulet"),
        (ItemFlag::ABILITY_SHIELD, "ability_shield"),
        (ItemFlag::PUNCHING_GLOVE, "punching_glove"),
        (ItemFlag::MIRROR_HERB, "mirror_herb"),
        (ItemFlag::UTILITY_UMBRELLA, "utility_umbrella"),
        (ItemFlag::THROAT_SPRAY, "throat_spray"),
        (ItemFlag::PROTECTIVE_PADS, "protective_pads"),
    ];
    NAMES.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, name)| *name).collect()
}

fn main() {
    let total_moves = GEN_MOVES.len();

    let moves: Vec<MoveOut> = (0..total_moves)
        .map(|i| {
            let md = &GEN_MOVES[i];
            let meta = &GEN_MOVE_META[i];
            MoveOut {
                id: i,
                base_power: md.base_power,
                accuracy: md.accuracy,
                category: format!("{:?}", md.category),
                move_type: format!("{:?}", md.move_type),
                priority: md.priority,
                pp: meta.pp,
                flags: md.flags,
                flag_names: move_flag_names(md.flags),
                crit_ratio: md.crit_ratio,
                drain: md.drain,
                multihit_lo: md.multihit_lo(),
                multihit_hi: md.multihit_hi(),
                secondary_chance: md.secondary_chance,
                secondary_stat: md.secondary_stat,
                secondary_status: md.secondary_status,
                effect: format!("{:?}", md.effect),
                self_effect: format!("{:?}", md.self_effect),
                var_power: format!("{:?}", md.var_power),
                target: target_name(meta.target).to_string(),
            }
        })
        .collect();

    let species: Vec<SpeciesOut> = (0..TOTAL_SPECIES)
        .map(|i| {
            let sp = &GEN_SPECIES[i];
            SpeciesOut {
                id: i,
                hp: sp.hp,
                atk: sp.atk,
                def: sp.def,
                spa: sp.spa,
                spd: sp.spd,
                spe: sp.spe,
                type1: format!("{:?}", sp.type1),
                type2: format!("{:?}", sp.type2),
                weight: sp.weight,
            }
        })
        .collect();

    let items: Vec<ItemOut> = (0..GEN_ITEMS.len())
        .map(|i| {
            let it = &GEN_ITEMS[i];
            ItemOut {
                id: i,
                flags: it.flags,
                flag_names: item_flag_names(it.flags),
                type_param: it.type_param,
                power_param: it.power_param,
                forme_species: it.forme_species,
                is_none: it.is_none(),
            }
        })
        .collect();

    let output = DumpOutput {
        moves,
        species,
        items,
        type_chart: EFFECTIVENESS,
        constants: Constants {
            total_moves,
            total_species: TOTAL_SPECIES,
            total_items: GEN_ITEMS.len(),
            forme_offset: pkmn_engine::data::FORME_OFFSET,
        },
    };

    serde_json::to_writer(std::io::stdout().lock(), &output).unwrap();
}
