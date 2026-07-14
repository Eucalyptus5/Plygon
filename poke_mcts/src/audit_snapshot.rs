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
}
