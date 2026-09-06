pub mod serialize;

use pkmn_engine::state::{BattleState, MonBuildData, TeamData};
use std::io;

// The single record->TeamData reconstruction shared by the serializer's EV
// story and policy_baseline: neutral nature (0), 31 IVs, EVs back-solved from
// the recorded stats, so both label and baseline carry the identical bounded
// form-change residual.
pub fn reconstruct_teams(state: &BattleState) -> TeamData {
    let mut teams = TeamData::default();
    for side in 0..2 {
        for k in 0..6 {
            let mon = &state.sides[side].team[k];
            if mon.species_id == 0 {
                continue;
            }
            teams.mons[side][k] = MonBuildData {
                ivs: [31; 6],
                evs: serialize::back_solve_evs(mon),
                nature: 0,
            };
            teams.levels[side][k] = mon.level;
        }
    }
    teams
}

// Move-name token -> decider action byte, lowercased. A duplicate token
// maps to None (ambiguous; invalidates the record at label time).
pub fn option_map(state: &BattleState, decider: usize) -> Vec<(String, Option<u8>)> {
    let side = &state.sides[decider];
    let active = &side.team[side.active_index as usize];
    let mut map: Vec<(String, Option<u8>)> = Vec::new();
    let mut insert = |map: &mut Vec<(String, Option<u8>)>, key: String, byte: u8| {
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = None,
            None => map.push((key, Some(byte))),
        }
    };
    for slot in 0..4 {
        let mid = active.moves[slot];
        if mid == 0 {
            continue;
        }
        let tok = serialize::move_token(mid).to_lowercase();
        insert(&mut map, tok.clone(), slot as u8);
        insert(&mut map, format!("{tok}-tera"), 10 + slot as u8);
    }
    for k in 0..6 {
        let sid = side.team[k].species_id;
        if sid == 0 {
            continue;
        }
        let key = format!("switch {}", serialize::species_token(sid).to_lowercase());
        insert(&mut map, key, 4 + k as u8);
    }
    map
}

pub fn fixed_record_size() -> u64 {
    let rec = crate::train_dump::TrainRecord {
        #[cfg(feature = "train_value")]
        record_version: 2,
        game_tag: 0,
        turn: 0,
        side_of_decider: 0,
        world_idx: 0,
        #[cfg(feature = "train_value")]
        root_value: 0.0,
        state: pkmn_engine::state::BattleState::default(),
    };
    bincode::serialize(&rec).unwrap().len() as u64
}

pub fn read_records_guarded(path: &str) -> io::Result<Vec<crate::train_dump::TrainRecord>> {
    let len = std::fs::metadata(path)?.len();
    let rec_size = fixed_record_size();
    if len % rec_size != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{path}: {len} bytes not divisible by {rec_size}-byte records"),
        ));
    }
    let recs = crate::train_dump::read_records(path)?;
    #[cfg(feature = "train_value")]
    for rec in &recs {
        if rec.record_version != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{path}: record_version {} != 2", rec.record_version),
            ));
        }
    }
    Ok(recs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{build_state, mon};

    #[test]
    fn reconstruct_teams_restores_recorded_stats_under_neutral_build() {
        let (state, real) = build_state(
            vec![mon(445, 24, [89, 14, 200, 328]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let teams = reconstruct_teams(&state);
        for side in 0..2 {
            for k in 0..6 {
                let m = &state.sides[side].team[k];
                if m.species_id == 0 {
                    assert_eq!(teams.levels[side][k], 0);
                    continue;
                }
                assert_eq!(teams.levels[side][k], real.levels[side][k]);
                assert_eq!(teams.mons[side][k].nature, 0);
                let sp = pkmn_engine::state::data_bridge::species(m.species_id);
                let bases = [sp.hp, sp.atk, sp.def, sp.spa, sp.spd, sp.spe];
                let e = teams.mons[side][k].evs;
                let l = m.level as u32;
                for i in 0..6usize {
                    let target = if i == 0 { m.max_hp } else { m.stats[i - 1] } as u32;
                    let raw = (2 * bases[i] as u32 + 31 + e[i] as u32 / 4) * l / 100;
                    let s = if i == 0 { raw + l + 10 } else { raw + 5 };
                    assert_eq!(s, target, "side {side} slot {k} stat {i}");
                }
            }
        }
    }

    #[test]
    fn option_map_covers_moves_tera_and_switches() {
        let (state, _) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 150, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let map = option_map(&state, 0);
        let get = |k: &str| map.iter().find(|(key, _)| key == k).map(|(_, v)| *v);
        assert_eq!(get("earthquake"), Some(Some(0)));
        assert_eq!(get("earthquake-tera"), Some(Some(10)));
        assert_eq!(get("swordsdance"), Some(Some(1)));
        assert_eq!(get("swordsdance-tera"), Some(Some(11)));
        assert_eq!(get("switch pikachu"), Some(Some(5)));
        assert!(get("switch garchomp").is_some(), "active mon still keyed by slot");
    }

    // Two-config guard proof on a real corpus shard: a train_value build reads
    // valid records (side-0 species resolve to dex names); a train_value-OFF
    // build of the same shard must hard-error on the size guard, never return
    // silently misaligned garbage.
    #[test]
    fn guarded_read_of_real_vlabel_shard() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../full_cp07_vlabel/s0");
        if !dir.exists() {
            eprintln!("skip: {} absent", dir.display());
            return;
        }
        let mut shards: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".records.bin"))
            .collect();
        shards.sort();
        let shard = shards.first().expect("s0 has shards").to_str().unwrap().to_string();
        #[cfg(feature = "train_value")]
        {
            let recs = read_records_guarded(&shard).unwrap();
            assert!(!recs.is_empty());
            for (idx, rec) in recs.iter().enumerate() {
                assert_eq!(rec.record_version, 2);
                for side in 0..2 {
                    for k in 0..6 {
                        let id = rec.state.sides[side].team[k].species_id;
                        if id == 0 {
                            continue;
                        }
                        assert!(
                            serialize::species_token(id) != "NONE",
                            "{shard}:{idx} side {side} slot {k}: species id {id} has no dex name"
                        );
                    }
                }
            }
        }
        #[cfg(not(feature = "train_value"))]
        {
            let err = read_records_guarded(&shard).unwrap_err();
            assert!(err.to_string().contains("not divisible"), "{err}");
        }
    }

    #[test]
    fn option_map_marks_duplicate_species_ambiguous() {
        let (state, _) = build_state(
            vec![mon(445, 24, [89, 14, 0, 0]), mon(25, 9, [85, 0, 0, 0]), mon(25, 9, [85, 0, 0, 0])],
            vec![mon(248, 45, [89, 242, 0, 0])],
        );
        let map = option_map(&state, 0);
        let pika = map.iter().find(|(k, _)| k == "switch pikachu").unwrap();
        assert_eq!(pika.1, None, "duplicate species must map to ambiguous, not a slot");
        assert_eq!(
            map.iter().filter(|(k, _)| k == "switch pikachu").count(),
            1,
            "one entry per token"
        );
    }
}
