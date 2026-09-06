use std::fs::OpenOptions;
use std::io::{self, BufReader, Write};

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub struct TrainRecord {
    #[cfg(feature = "train_value")]
    pub record_version: u8,
    pub game_tag: u64,
    pub turn: u16,
    pub side_of_decider: u8,
    pub world_idx: u8,
    #[cfg(feature = "train_value")]
    pub root_value: f32,
    pub state: pkmn_engine::state::BattleState,
}

pub fn dump_worlds(
    dir: &str,
    game_tag: u64,
    worlds: &[crate::determinize::World],
    side_of_decider: usize,
    turn: u16,
    #[cfg(feature = "train_value")] root_value: f32,
) -> std::io::Result<()> {
    #[cfg(feature = "train_value")]
    assert!(
        (0.0..=1.0).contains(&root_value),
        "root_value {root_value} outside [0,1]"
    );
    std::fs::create_dir_all(dir)?;
    let path = format!("{}/{}.records.bin", dir, game_tag);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    for (k, w) in worlds.iter().take(worlds.len().min(2)).enumerate() {
        let rec = TrainRecord {
            #[cfg(feature = "train_value")]
            record_version: 2,
            game_tag,
            turn,
            side_of_decider: side_of_decider as u8,
            world_idx: k as u8,
            #[cfg(feature = "train_value")]
            root_value,
            state: w.state,
        };
        let bytes = bincode::serialize(&rec)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        f.write_all(&bytes)?;
    }
    Ok(())
}

#[cfg(not(feature = "train_value"))]
pub fn maybe_dump_worlds(
    worlds: &[crate::determinize::World],
    side_of_decider: usize,
    turn: u16,
) {
    let dir = match std::env::var("BRIDGE_TRAIN_DUMP") {
        Ok(d) => d,
        Err(_) => return,
    };
    let game_tag = std::env::var("BRIDGE_GAME_TAG")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let _ = dump_worlds(&dir, game_tag, worlds, side_of_decider, turn);
}

// v-hat = equal-weight mean of the per-world accumulator means (matches the uniform
// world weights; per-world iteration counts are unequal under a time bound, so
// equal-weight vs total-sum/total-count is pinned here for reproducibility).
#[cfg(feature = "train_value")]
pub fn decision_value(searched: &[(crate::search::SearchResult, f64)]) -> Option<f32> {
    let mut sum = 0.0f64;
    let mut n = 0u32;
    for (r, _) in searched {
        if r.value_count > 0 {
            sum += r.value_sum / r.value_count as f64;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some((sum / n as f64) as f32)
}

#[cfg(feature = "train_value")]
pub fn dump_worlds_valued(
    dir: &str,
    game_tag: u64,
    worlds: &[crate::determinize::World],
    side_of_decider: usize,
    turn: u16,
    searched: &[(crate::search::SearchResult, f64)],
) -> std::io::Result<bool> {
    match decision_value(searched) {
        Some(v) => {
            dump_worlds(dir, game_tag, worlds, side_of_decider, turn, v)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(feature = "train_value")]
pub fn maybe_dump_worlds_valued(
    worlds: &[crate::determinize::World],
    side_of_decider: usize,
    turn: u16,
    searched: &[(crate::search::SearchResult, f64)],
) {
    let dir = match std::env::var("BRIDGE_TRAIN_DUMP") {
        Ok(d) => d,
        Err(_) => return,
    };
    let game_tag = std::env::var("BRIDGE_GAME_TAG")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if let Ok(false) = dump_worlds_valued(&dir, game_tag, worlds, side_of_decider, turn, searched) {
        eprintln!("train_dump: drop game_tag={game_tag} turn={turn} (no searched leaf values)");
    }
}

pub fn read_records(path: &str) -> std::io::Result<Vec<TrainRecord>> {
    let f = std::fs::File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut out = Vec::new();
    loop {
        match bincode::deserialize_from::<_, TrainRecord>(&mut reader) {
            Ok(rec) => out.push(rec),
            Err(e) => match *e {
                bincode::ErrorKind::Io(ref io_err)
                    if io_err.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    break
                }
                _ => return Err(io::Error::new(io::ErrorKind::Other, e)),
            },
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::determinize::World;

    fn tmp_dir(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!("poke_mcts_train_{}_{}", tag, std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    fn dump(dir: &str, tag: u64, worlds: &[World], side: usize, turn: u16) -> std::io::Result<()> {
        #[cfg(feature = "train_value")]
        return dump_worlds(dir, tag, worlds, side, turn, 0.5);
        #[cfg(not(feature = "train_value"))]
        dump_worlds(dir, tag, worlds, side, turn)
    }

    #[test]
    fn round_trip_train_record() {
        let (state, teams) = crate::testutil::build_state(
            vec![
                crate::testutil::mon(445, 24, [89, 14, 200, 328]),
                crate::testutil::mon(25, 9, [85, 150, 0, 0]),
            ],
            vec![
                crate::testutil::mon(248, 45, [89, 242, 0, 0]),
                crate::testutil::mon(6, 66, [53, 394, 0, 0]),
            ],
        );

        let worlds = vec![
            World { state, teams: teams.clone(), weight: 1.0 },
            World { state, teams, weight: 1.0 },
        ];

        let dir = tmp_dir("rt");
        let tag: u64 = 0xABCD_1234;
        dump(&dir, tag, &worlds, 0, 12).expect("dump must succeed");

        let path = format!("{}/{}.records.bin", dir, tag);
        let recs = read_records(&path).expect("read must succeed");

        assert_eq!(recs.len(), 2);
        for (k, rec) in recs.iter().enumerate() {
            assert_eq!(rec.game_tag, tag);
            assert_eq!(rec.turn, 12);
            assert_eq!(rec.side_of_decider, 0);
            assert_eq!(rec.world_idx, k as u8);
            assert_eq!(rec.state, worlds[k].state);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p2_decider_side0_is_p1_team() {
        let (state, teams) = crate::testutil::build_state(
            vec![crate::testutil::mon(445, 24, [89, 0, 0, 0])],
            vec![crate::testutil::mon(248, 45, [89, 0, 0, 0])],
        );

        let worlds = vec![World { state, teams, weight: 1.0 }];

        let dir = tmp_dir("p2side");
        let tag: u64 = 0x5555;
        dump(&dir, tag, &worlds, 1, 7).expect("dump must succeed");

        let path = format!("{}/{}.records.bin", dir, tag);
        let recs = read_records(&path).expect("read must succeed");

        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].side_of_decider, 1);
        assert_eq!(recs[0].state.sides[0].team[0].species_id, 445);
        assert_eq!(recs[0].state.sides[1].team[0].species_id, 248);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caps_at_two_worlds() {
        let (state, teams) = crate::testutil::build_state(
            vec![crate::testutil::mon(445, 24, [89, 0, 0, 0])],
            vec![crate::testutil::mon(248, 45, [89, 0, 0, 0])],
        );

        let worlds: Vec<World> = (0..4)
            .map(|_| World { state, teams: teams.clone(), weight: 1.0 })
            .collect();

        let dir = tmp_dir("cap");
        let tag: u64 = 0x9999;
        dump(&dir, tag, &worlds, 0, 3).expect("dump must succeed");

        let path = format!("{}/{}.records.bin", dir, tag);
        let recs = read_records(&path).expect("read must succeed");

        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].world_idx, 0);
        assert_eq!(recs[1].world_idx, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "train_value")]
    mod valued {
        use super::*;
        use crate::search::SearchResult;

        fn sr(value_sum: f64, value_count: u64) -> SearchResult {
            SearchResult { value_sum, value_count, ..Default::default() }
        }

        fn worlds(n: usize) -> Vec<World> {
            let (state, teams) = crate::testutil::build_state(
                vec![crate::testutil::mon(445, 24, [89, 0, 0, 0])],
                vec![crate::testutil::mon(248, 45, [89, 0, 0, 0])],
            );
            (0..n).map(|_| World { state, teams: teams.clone(), weight: 1.0 }).collect()
        }

        #[test]
        fn decision_value_is_equal_weight_mean_skipping_unsearched_worlds() {
            let searched = vec![(sr(30.0, 100), 0.9), (sr(80.0, 100), 0.1), (sr(0.0, 0), 1.0)];
            let v = decision_value(&searched).expect("two searched worlds");
            assert!((v - 0.55).abs() < 1e-6, "mean of 0.3 and 0.8 ignoring world weights, got {v}");
            assert!(decision_value(&[(sr(0.0, 0), 1.0)]).is_none(), "no searched values -> drop");
        }

        #[test]
        fn p1_and_p2_deciders_write_unflipped_side0_value() {
            let worlds = worlds(1);
            let searched = vec![(sr(77.61, 100), 1.0)];
            let dir = tmp_dir("unflipped");
            for (tag, side) in [(1u64, 0usize), (2, 1)] {
                let wrote = dump_worlds_valued(&dir, tag, &worlds, side, 3, &searched)
                    .expect("dump must succeed");
                assert!(wrote);
                let recs = read_records(&format!("{}/{}.records.bin", dir, tag)).unwrap();
                assert_eq!(recs[0].side_of_decider, side as u8);
                assert!(
                    (recs[0].root_value - 0.7761).abs() < 1e-6,
                    "decider side {side} must carry the side0-absolute value unflipped"
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn sibling_records_share_the_decision_value() {
            let worlds = worlds(2);
            let searched = vec![(sr(20.0, 100), 1.0), (sr(60.0, 100), 1.0)];
            let dir = tmp_dir("sibling");
            dump_worlds_valued(&dir, 7, &worlds, 0, 9, &searched).expect("dump must succeed");
            let recs = read_records(&format!("{}/7.records.bin", dir)).unwrap();
            assert_eq!(recs.len(), 2);
            assert_eq!(recs[0].record_version, 2);
            assert_eq!(recs[1].record_version, 2);
            assert_eq!(recs[0].root_value, recs[1].root_value);
            assert!((recs[0].root_value - 0.4).abs() < 1e-6);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn empty_searched_worlds_drop_the_decision() {
            let worlds = worlds(1);
            let dir = tmp_dir("dropped");
            let wrote = dump_worlds_valued(&dir, 8, &worlds, 0, 1, &[(sr(0.0, 0), 1.0)])
                .expect("drop is not an error");
            assert!(!wrote);
            assert!(!std::path::Path::new(&format!("{}/8.records.bin", dir)).exists());
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        #[should_panic(expected = "outside [0,1]")]
        fn root_value_out_of_range_panics_at_write_time() {
            let worlds = worlds(1);
            let dir = tmp_dir("range");
            let _ = dump_worlds(&dir, 9, &worlds, 0, 1, 1.5);
        }
    }
}
