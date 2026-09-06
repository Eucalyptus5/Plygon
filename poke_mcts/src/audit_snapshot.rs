#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct PositionSnapshot {
    pub game_id: String,
    pub turn: u32,
    pub our_side: usize,
    pub our_pick: u8,
    pub live_budget: (usize, u64, u64),
    pub request_kind: String,
    pub state: pkmn_engine::state::BattleState,
    pub teams: pkmn_engine::state::TeamData,
    pub belief: crate::belief::Belief,
    pub tags: Vec<String>,
}

impl PositionSnapshot {
    pub fn write(&self, dir: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = format!("{}/{}-t{}.json", dir, self.game_id, self.turn);
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    pub fn read(path: &str) -> std::io::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }
}

pub type SearchStatePair = (pkmn_engine::state::BattleState, pkmn_engine::state::TeamData);

fn bincode_err(e: bincode::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}

pub fn write_search_states(path: &str, pairs: &[SearchStatePair]) -> std::io::Result<()> {
    let bytes = bincode::serialize(pairs).map_err(bincode_err)?;
    std::fs::write(path, bytes)
}

pub fn read_search_states(path: &str) -> std::io::Result<Vec<SearchStatePair>> {
    let bytes = std::fs::read(path)?;
    bincode::deserialize(&bytes).map_err(bincode_err)
}

pub fn pack_search_states(dir: &str, out: &str, limit: usize) -> std::io::Result<usize> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let mut pairs: Vec<SearchStatePair> = Vec::new();
    for p in paths.iter().take(limit) {
        let snap = PositionSnapshot::read(&p.to_string_lossy())?;
        pairs.push((snap.state, snap.teams));
    }
    write_search_states(out, &pairs)?;
    Ok(pairs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_position_snapshot() {
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

        let mut belief = crate::belief::Belief::default();
        let s0 = belief.note_species(248, 80);
        belief.note_move(s0, 89);
        belief.note_move(s0, 242);
        let s1 = belief.note_species(6, 80);
        belief.note_move(s1, 53);

        let game_id = format!("test-rt-{}", std::process::id());
        let snap = PositionSnapshot {
            game_id: game_id.clone(),
            turn: 12,
            our_side: 0,
            our_pick: 7,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state: state.clone(),
            teams: teams.clone(),
            belief,
            tags: vec![],
        };

        let dir = std::env::temp_dir()
            .join(format!("poke_mcts_rt_{}", std::process::id()))
            .to_string_lossy()
            .into_owned();

        snap.write(&dir).expect("write must succeed");

        let path = format!("{}/{}-t12.json", dir, game_id);
        let back = PositionSnapshot::read(&path).expect("read must succeed");

        assert_eq!(snap.our_pick, back.our_pick);
        assert_eq!(snap.turn, back.turn);
        assert_eq!(snap.our_side, back.our_side);
        assert_eq!(snap.state, back.state);
        assert_eq!(snap.belief, back.belief);
        assert_eq!(snap.teams, back.teams);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sample_snapshot(game_id: &str, turn: u32) -> PositionSnapshot {
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
        PositionSnapshot {
            game_id: game_id.to_string(),
            turn,
            our_side: 0,
            our_pick: 1,
            live_budget: (8, 200, 1000),
            request_kind: "move".into(),
            state,
            teams,
            belief: crate::belief::Belief::default(),
            tags: vec![],
        }
    }

    #[test]
    fn packed_search_states_round_trip() {
        let dir = std::env::temp_dir()
            .join(format!("poke_mcts_pack_{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_dir_all(&dir);

        let a = sample_snapshot("game-a", 3);
        let mut b = sample_snapshot("game-b", 9);
        b.state.field.turn = 9;
        a.write(&dir).expect("write must succeed");
        b.write(&dir).expect("write must succeed");

        let out = format!("{dir}/packed.bin");
        let n = pack_search_states(&dir, &out, 8).expect("pack must succeed");
        assert_eq!(n, 2, "both snapshots must pack");

        let pairs = read_search_states(&out).expect("read must succeed");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, a.state, "name-sorted first pair is game-a");
        assert_eq!(pairs[0].1, a.teams);
        assert_eq!(pairs[1].0, b.state);
        assert_eq!(pairs[1].1, b.teams);

        let capped = pack_search_states(&dir, &out, 1).expect("pack must succeed");
        assert_eq!(capped, 1, "limit must cap the pack");
        assert_eq!(read_search_states(&out).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
