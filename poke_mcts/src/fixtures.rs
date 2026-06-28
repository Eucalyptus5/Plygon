use pkmn_engine::state::*;
use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct MonJson {
    pub species_id: u16, pub ability_id: u16, pub item_id: u16,
    pub moves: Vec<u16>, pub ivs: Vec<u8>, pub evs: Vec<u8>,
    pub nature: u8, pub level: u8, pub tera_type: u8, pub is_female: bool,
}

#[derive(Deserialize)]
pub struct Fixture { pub teams: Vec<Vec<MonJson>> }

pub fn to_input(m: &MonJson) -> MonBuildInput {
    let arr4 = |v: &Vec<u16>| { let mut a = [0u16; 4]; for (i, &x) in v.iter().take(4).enumerate() { a[i] = x; } a };
    let arr6 = |v: &Vec<u8>| { let mut a = [0u8; 6]; for (i, &x) in v.iter().take(6).enumerate() { a[i] = x; } a };
    MonBuildInput {
        species_id: m.species_id, ability_id: m.ability_id, item_id: m.item_id,
        moves: arr4(&m.moves), ivs: arr6(&m.ivs), evs: arr6(&m.evs),
        nature: m.nature, level: m.level, tera_type: m.tera_type, is_female: m.is_female,
    }
}

pub fn build(team: &[MonJson]) -> ([MonSlot; 6], [MonBuildData; 6], [u8; 6]) {
    let mut inputs: [MonBuildInput; 6] = std::array::from_fn(|_| MonBuildInput {
        species_id: 0, ability_id: 0, item_id: 0, moves: [0; 4], ivs: [0; 6], evs: [0; 6],
        nature: 0, level: 100, tera_type: 0, is_female: false,
    });
    for (i, m) in team.iter().take(6).enumerate() { inputs[i] = to_input(m); }
    let (mut mons, bd, levels) = build_team(&inputs);
    // showdown_type_to_engine is 18-wide and clamps Stellar (18) to Normal; restore it post-build.
    for (i, inp) in inputs.iter().enumerate() {
        if inp.tera_type == 18 { mons[i].tera_type = 18; }
    }
    (mons, bd, levels)
}
