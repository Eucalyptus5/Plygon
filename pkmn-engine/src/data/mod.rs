// src/data/mod.rs

pub mod types;
pub mod base_stats;
pub mod moves;

#[path = "generated/gen_moves.rs"]
pub(crate) mod gen_moves;
#[path = "generated/gen_species.rs"]
pub(crate) mod gen_species;

// Re-export generated data so the rest of the engine uses clean paths:
//   crate::data::MOVES, crate::data::MOVE_META, crate::data::SPECIES
//   crate::data::MOVE_EARTHQUAKE, crate::data::FORME_AEGISLASH_BLADE, etc.
pub use gen_moves::*;
pub use gen_species::*;
