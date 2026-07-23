pub mod serialize;

use std::io;

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
