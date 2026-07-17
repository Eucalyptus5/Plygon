use std::fs::OpenOptions;
use std::io::{self, BufReader, Write};

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub struct TrainRecord {
    pub game_tag: u64,
    pub turn: u16,
    pub side_of_decider: u8,
    pub world_idx: u8,
    pub state: pkmn_engine::state::BattleState,
}

pub fn dump_worlds(
    dir: &str,
    game_tag: u64,
    worlds: &[crate::determinize::World],
    side_of_decider: usize,
    turn: u16,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = format!("{}/{}.records.bin", dir, game_tag);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    for (k, w) in worlds.iter().take(worlds.len().min(2)).enumerate() {
        let rec = TrainRecord {
            game_tag,
            turn,
            side_of_decider: side_of_decider as u8,
            world_idx: k as u8,
            state: w.state,
        };
        let bytes = bincode::serialize(&rec)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        f.write_all(&bytes)?;
    }
    Ok(())
}

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
        dump_worlds(&dir, tag, &worlds, 0, 12).expect("dump must succeed");

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
        dump_worlds(&dir, tag, &worlds, 1, 7).expect("dump must succeed");

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
        dump_worlds(&dir, tag, &worlds, 0, 3).expect("dump must succeed");

        let path = format!("{}/{}.records.bin", dir, tag);
        let recs = read_records(&path).expect("read must succeed");

        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].world_idx, 0);
        assert_eq!(recs[1].world_idx, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
