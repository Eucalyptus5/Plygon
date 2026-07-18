use pkmn_engine::data::{GEN_ITEMS, GEN_MOVES, TOTAL_SPECIES};
use pkmn_engine::state::*;
use std::sync::OnceLock;

pub const FEATURE_SPEC_VERSION: u32 = 2;

// Dense material order: [s0 hp-sum, s1 hp-sum, s0 alive, s1 alive, hp diff, alive diff]
pub const DENSE_DIM: usize = 6;

// No public ability-count in the engine; codegen sizes its per-ability bit
// table as [u64; 5], bounding ability ids at 320.
const ABILITY_SPACE: u32 = 320;

// Durable volatiles only: EOT-cleared bits are provably 0 at evaluated leaves,
// and YAWN is invisible to the Handcrafted baseline.
const VOL_EMIT_MASK: u32 = !(VOL_PER_TURN_MASK | VOL_YAWN);
const VOL_KEPT: u32 = VOL_EMIT_MASK.count_ones();

pub struct GroupSpec {
    pub name: &'static str,
    pub offset: u32,
    pub size: u32,
}

struct Layout {
    s: u32,
    m: u32,
    i: u32,
    a: u32,
    f1: u32,
    f2: u32,
    f3: u32,
    f4: u32,
    f5: u32,
    f6: u32,
    f7: u32,
    f8: u32,
    f8b: u32,
    f9: u32,
    f10: u32,
    f11: u32,
    f12: u32,
    f13: u32,
    total: u32,
}

fn layout() -> &'static Layout {
    static L: OnceLock<Layout> = OnceLock::new();
    L.get_or_init(|| {
        let s = TOTAL_SPECIES as u32;
        let m = GEN_MOVES.len() as u32;
        let i = GEN_ITEMS.len() as u32;
        let a = ABILITY_SPACE;
        let f1 = 0;
        let f2 = f1 + 2 * 2 * s;
        let f3 = f2 + 2 * s * 17;
        let f4 = f3 + 2 * s * 6;
        let f5 = f4 + 2 * 2 * m;
        let f6 = f5 + 2 * 2 * i;
        let f7 = f6 + 2 * 2 * a;
        let f8 = f7 + 2 * 5 * 12;
        let f8b = f8 + 2 * VOL_KEPT;
        let f9 = f8b + 2;
        let f10 = f9 + 2 * 7;
        let f11 = f10 + 2 * 6;
        let f12 = f11 + 2 + 2 * 19;
        let f13 = f12 + 13;
        let total = f13 + 8;
        Layout { s, m, i, a, f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12, f13, f8b, total }
    })
}

#[inline]
fn vol_rank(bit: u32) -> u32 {
    (VOL_EMIT_MASK & ((1u32 << bit) - 1)).count_ones()
}

pub fn extract(state: &BattleState, out: &mut Vec<u32>) {
    let l = layout();
    for side in 0..2usize {
        let sd = &state.sides[side];
        let s32 = side as u32;
        let active_idx = (sd.active_index as usize).min(5);
        for slot in 0..6 {
            let mon = &sd.team[slot];
            if mon.species_id == 0 {
                continue;
            }
            let act = (slot != active_idx) as u32;
            let sp = mon.species_id as u32;
            out.push(l.f1 + s32 * (2 * l.s) + act * l.s + sp);
            let bucket = if mon.max_hp == 0 {
                0
            } else {
                (16 * mon.current_hp as u32 + mon.max_hp as u32 - 1) / mon.max_hp as u32
            };
            out.push(l.f2 + s32 * (l.s * 17) + sp * 17 + bucket);
            if mon.status != STATUS_NONE {
                out.push(l.f3 + s32 * (l.s * 6) + sp * 6 + (mon.status as u32 - 1));
            }
            for &mv in &mon.moves {
                if mv != 0 {
                    out.push(l.f4 + s32 * (2 * l.m) + act * l.m + mv as u32);
                }
            }
            if mon.item_id != 0 {
                out.push(l.f5 + s32 * (2 * l.i) + act * l.i + mon.item_id as u32);
            }
            if mon.ability_id != 0 {
                out.push(l.f6 + s32 * (2 * l.a) + act * l.a + mon.ability_id as u32);
            }
        }
        let a = &sd.active;
        for stat in 0..5usize {
            let b = a.boosts[stat];
            if b != 0 {
                let idx = if b < 0 { (b + 6) as u32 } else { (b + 5) as u32 };
                out.push(l.f7 + s32 * 60 + stat as u32 * 12 + idx);
            }
        }
        let mut bits = a.volatile_flags & VOL_EMIT_MASK;
        while bits != 0 {
            let bit = bits.trailing_zeros();
            out.push(l.f8 + s32 * VOL_KEPT + vol_rank(bit));
            bits &= bits - 1;
        }
        if a.confusion_turns > 0 {
            out.push(l.f8b + s32);
        }
        let sc = &sd.side_conditions;
        if sc.hazard_flags & HAZARD_STEALTH_ROCK != 0 {
            out.push(l.f9 + s32 * 7);
        }
        if sc.hazard_flags & HAZARD_STICKY_WEB != 0 {
            out.push(l.f9 + s32 * 7 + 1);
        }
        if sc.spikes > 0 {
            out.push(l.f9 + s32 * 7 + 1 + sc.spikes.min(3) as u32);
        }
        if sc.toxic_spikes > 0 {
            out.push(l.f9 + s32 * 7 + 4 + sc.toxic_spikes.min(2) as u32);
        }
        if sc.reflect_turns > 0 {
            out.push(l.f10 + s32 * 6);
        }
        if sc.light_screen_turns > 0 {
            out.push(l.f10 + s32 * 6 + 1);
        }
        if sc.aurora_veil_turns > 0 {
            out.push(l.f10 + s32 * 6 + 2);
        }
        if sc.safeguard_turns() > 0 {
            out.push(l.f10 + s32 * 6 + 3);
        }
        if sc.tailwind_turns > 0 {
            out.push(l.f10 + s32 * 6 + 4);
        }
        if sc.has_healing_wish() {
            out.push(l.f10 + s32 * 6 + 5);
        }
        // battle-lifetime Tera-used bit, not the active mon's per-slot flag
        if sd._padding[0] & 1 != 0 {
            out.push(l.f11 + s32);
        }
        let active_mon = &sd.team[active_idx];
        // gate on is_terastallized only: determinizer-installed guessed tera_type
        // on non-tera'd mons must not leak through a value-based gate
        if active_mon.is_terastallized() && (active_mon.tera_type as u32) < 19 {
            out.push(l.f11 + 2 + s32 * 19 + active_mon.tera_type as u32);
        }
    }
    let f = &state.field;
    if f.weather != WEATHER_NONE {
        out.push(l.f12 + f.weather as u32 - 1);
    }
    if f.terrain != TERRAIN_NONE {
        out.push(l.f12 + 7 + f.terrain as u32 - 1);
    }
    if f.trick_room_turns > 0 {
        out.push(l.f12 + 11);
    }
    if f.gravity_turns > 0 {
        out.push(l.f12 + 12);
    }
    out.push(l.f13 + (f.turn as u32 / 5).min(7));
}

pub fn extract_dense(state: &BattleState) -> [f32; DENSE_DIM] {
    let hp_sum = |side: usize| -> f32 {
        let mut sum = 0.0f32;
        for slot in 0..6 {
            let mon = &state.sides[side].team[slot];
            if mon.species_id == 0 {
                continue;
            }
            sum += if mon.max_hp == 0 {
                0.0
            } else {
                mon.current_hp as f32 / mon.max_hp as f32
            };
        }
        sum
    };
    let alive = |side: usize| -> f32 {
        let mut n = 0u32;
        for slot in 0..6 {
            let mon = &state.sides[side].team[slot];
            if mon.species_id != 0 && mon.current_hp > 0 {
                n += 1;
            }
        }
        n as f32
    };
    let d0 = hp_sum(0);
    let d1 = hp_sum(1);
    let d2 = alive(0);
    let d3 = alive(1);
    [d0, d1, d2, d3, d0 - d1, d2 - d3]
}

pub fn flip_side(id: u32) -> u32 {
    let l = layout();
    let flip = |off: u32, block: u32| {
        let g = id - off;
        off + (1 - g / block) * block + g % block
    };
    if id < l.f2 {
        flip(l.f1, 2 * l.s)
    } else if id < l.f3 {
        flip(l.f2, l.s * 17)
    } else if id < l.f4 {
        flip(l.f3, l.s * 6)
    } else if id < l.f5 {
        flip(l.f4, 2 * l.m)
    } else if id < l.f6 {
        flip(l.f5, 2 * l.i)
    } else if id < l.f7 {
        flip(l.f6, 2 * l.a)
    } else if id < l.f8 {
        flip(l.f7, 60)
    } else if id < l.f8b {
        flip(l.f8, VOL_KEPT)
    } else if id < l.f9 {
        flip(l.f8b, 1)
    } else if id < l.f10 {
        flip(l.f9, 7)
    } else if id < l.f11 {
        flip(l.f10, 6)
    } else if id < l.f12 {
        let g = id - l.f11;
        if g < 2 {
            l.f11 + (1 - g)
        } else {
            let t = g - 2;
            l.f11 + 2 + (1 - t / 19) * 19 + t % 19
        }
    } else {
        id
    }
}

pub fn mirror(state: &BattleState) -> BattleState {
    let mut m = *state;
    m.sides.swap(0, 1);
    m
}

pub fn spec() -> Vec<GroupSpec> {
    let l = layout();
    vec![
        GroupSpec { name: "F1 identity (side,act,species)", offset: l.f1, size: l.f2 - l.f1 },
        GroupSpec { name: "F2 hp (side,species,bucket)", offset: l.f2, size: l.f3 - l.f2 },
        GroupSpec { name: "F3 status (side,species,status)", offset: l.f3, size: l.f4 - l.f3 },
        GroupSpec { name: "F4 moves (side,act,move)", offset: l.f4, size: l.f5 - l.f4 },
        GroupSpec { name: "F5 item (side,act,item)", offset: l.f5, size: l.f6 - l.f5 },
        GroupSpec { name: "F6 ability (side,act,ability)", offset: l.f6, size: l.f7 - l.f6 },
        GroupSpec { name: "F7 boosts (side,stat,stage)", offset: l.f7, size: l.f8 - l.f7 },
        GroupSpec { name: "F8 volatiles (side,durable_bit)", offset: l.f8, size: l.f8b - l.f8 },
        GroupSpec { name: "F8b confusion (side)", offset: l.f8b, size: l.f9 - l.f8b },
        GroupSpec { name: "F9 hazards (side,kind,layer)", offset: l.f9, size: l.f10 - l.f9 },
        GroupSpec { name: "F10 side conds (side,cond)", offset: l.f10, size: l.f11 - l.f10 },
        GroupSpec { name: "F11 tera used+type", offset: l.f11, size: l.f12 - l.f11 },
        GroupSpec { name: "F12 field", offset: l.f12, size: l.f13 - l.f12 },
        GroupSpec { name: "F13 turn bucket", offset: l.f13, size: l.total - l.f13 },
    ]
}

pub fn vocab_size() -> u32 {
    layout().total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{build_state, mon};

    fn golden_state() -> BattleState {
        let (mut s, _t) = build_state(
            vec![
                mon(445, 24, [89, 14, 200, 328]),
                mon(25, 9, [85, 150, 0, 0]),
                mon(143, 47, [34, 0, 0, 0]),
            ],
            vec![mon(248, 45, [89, 242, 0, 0]), mon(6, 66, [53, 394, 0, 0])],
        );
        s.sides[0].team[0].max_hp = 160;
        s.sides[0].team[0].current_hp = 45;
        s.sides[0].team[0].item_id = 242;
        s.sides[0].team[0].flags |= MON_FLAG_TERASTALLIZED;
        s.sides[0].team[0].tera_type = 9;
        s.sides[0].team[1].max_hp = 100;
        s.sides[0].team[1].current_hp = 100;
        s.sides[0].team[1].status = STATUS_BURN;
        s.sides[0].team[2].max_hp = 200;
        s.sides[0].team[2].current_hp = 0;
        s.sides[0].active.boosts[SPA] = 2;
        s.sides[0].active.boosts[SPE] = -1;
        s.sides[0].active.volatile_flags = VOL_LEECH_SEED | VOL_FLINCHED;
        s.sides[0].active.confusion_turns = 2;
        s.sides[0].side_conditions.reflect_turns = 3;
        s.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
        s.sides[0]._padding[0] |= 1;
        s.sides[1].team[0].max_hp = 175;
        s.sides[1].team[0].current_hp = 175;
        s.sides[1].team[0].status = STATUS_BAD_POISON;
        s.sides[1].team[0].item_id = 17;
        s.sides[1].team[1].max_hp = 150;
        s.sides[1].team[1].current_hp = 3;
        s.sides[1].active.boosts[ATK] = -2;
        s.sides[1].active.volatile_flags = VOL_SUBSTITUTE;
        s.sides[1].side_conditions.spikes = 2;
        s.sides[1].side_conditions.tailwind_turns = 2;
        s.field.weather = WEATHER_SAND;
        s.field.weather_turns = 4;
        s.field.terrain = TERRAIN_GRASSY;
        s.field.terrain_turns = 3;
        s.field.trick_room_turns = 3;
        s.field.turn = 12;
        s
    }

    fn sidecar_ids() -> Vec<u32> {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/golden/lv0_golden_features.json"))
                .expect("sidecar must parse");
        assert_eq!(doc["feature_spec_version"].as_u64().unwrap() as u32, FEATURE_SPEC_VERSION);
        doc["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_u64().unwrap() as u32)
            .collect()
    }

    #[test]
    fn lv0_golden_fixture_exact_feature_set() {
        let s = golden_state();
        let mut got = Vec::new();
        extract(&s, &mut got);
        let mut got_sorted = got.clone();
        got_sorted.sort_unstable();
        got_sorted.dedup();
        assert_eq!(got_sorted.len(), got.len(), "extractor emitted duplicate ids");
        let mut expected = sidecar_ids();
        expected.sort_unstable();
        assert_eq!(got_sorted, expected);
    }

    #[test]
    fn negative_fixture_default_state_emits_only_turn_bucket() {
        let s = BattleState::default();
        let mut got = Vec::new();
        extract(&s, &mut got);
        assert_eq!(got, vec![80965]);
    }

    #[test]
    fn mirror_feature_set_is_side_flip() {
        let s = golden_state();
        let mut orig = Vec::new();
        extract(&s, &mut orig);
        let mut mirrored = Vec::new();
        extract(&mirror(&s), &mut mirrored);
        let mut flipped: Vec<u32> = orig.iter().map(|&id| flip_side(id)).collect();
        flipped.sort_unstable();
        mirrored.sort_unstable();
        assert_eq!(mirrored, flipped);
        for &id in &orig {
            assert_eq!(flip_side(flip_side(id)), id);
        }
    }

    #[test]
    fn spec_matches_sidecar_layout() {
        let groups = spec();
        assert_eq!(groups.len(), 14);
        let mut cur = 0u32;
        for g in &groups {
            assert_eq!(g.offset, cur, "group {} offset", g.name);
            cur += g.size;
        }
        assert_eq!(cur, vocab_size());
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/golden/lv0_golden_features.json")).unwrap();
        assert_eq!(vocab_size() as u64, doc["vocab_total"].as_u64().unwrap());
        assert_eq!(doc["dense_dim"].as_u64().unwrap() as usize, DENSE_DIM);
    }

    #[test]
    fn dense_golden_exact() {
        let d = extract_dense(&golden_state());
        let expected: [f32; DENSE_DIM] = [
            0.28125 + 1.0,
            1.0 + 3.0f32 / 150.0,
            2.0,
            2.0,
            (0.28125f32 + 1.0) - (1.0 + 3.0f32 / 150.0),
            0.0,
        ];
        assert_eq!(d, expected);
    }

    #[test]
    fn dense_default_state_all_zero() {
        assert_eq!(extract_dense(&BattleState::default()), [0.0f32; DENSE_DIM]);
    }

    #[test]
    fn dense_mirror_is_swap_and_negate() {
        let check = |s: &BattleState| -> [f32; DENSE_DIM] {
            let d = extract_dense(s);
            let m = extract_dense(&mirror(s));
            assert_eq!(m, [d[1], d[0], d[3], d[2], -d[4], -d[5]]);
            d
        };
        check(&golden_state());
        let mut s2 = golden_state();
        s2.sides[1].team[1].current_hp = 0;
        let d2 = check(&s2);
        assert_ne!(d2[5], 0.0, "modified clone must exercise d5 negation");
    }
}
