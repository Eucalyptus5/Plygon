//! Zobrist hashing for transposition table lookups.

use crate::state::structs::*;

const NUM_SPECIES: usize = 1500;
const NUM_ITEMS: usize = 1500;
const HP_BUCKETS: usize = 8;
const NUM_STATUSES: usize = 7;
const BOOST_STAGES: usize = 13;
const NUM_STATS: usize = 7;
const NUM_WEATHERS: usize = 8;
const NUM_TERRAINS: usize = 5;

struct KeyRng { s: [u64; 2] }

impl KeyRng {
    fn new(seed: u64) -> Self {
        Self { s: [seed, seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1)] }
    }
    fn next(&mut self) -> u64 {
        let s0 = self.s[0];
        let mut s1 = self.s[1];
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.s[0] = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.s[1] = s1.rotate_left(28);
        result
    }
    fn fill(&mut self, buf: &mut [u64]) {
        for v in buf.iter_mut() { *v = self.next(); }
    }
}

pub struct ZobristKeys {
    pub species: Box<[[[u64; NUM_SPECIES]; 6]; 2]>,
    pub hp_bucket: [[[u64; HP_BUCKETS]; 6]; 2],
    pub status: [[[u64; NUM_STATUSES]; 6]; 2],
    pub item: Box<[[[u64; NUM_ITEMS]; 6]; 2]>,
    pub active_index: [[u64; 6]; 2],
    pub boosts: [[[u64; BOOST_STAGES]; NUM_STATS]; 2],
    pub volatile_bit: [[u64; 32]; 2],
    pub weather: [u64; NUM_WEATHERS],
    pub terrain: [u64; NUM_TERRAINS],
    pub trick_room: u64,
    pub gravity: u64,
    pub phase: [u64; 5],
}

impl ZobristKeys {
    pub fn new(seed: u64) -> Self {
        let mut rng = KeyRng::new(seed);

        let mut species = vec![[[0u64; NUM_SPECIES]; 6]; 2].into_boxed_slice();
        for side in species.iter_mut() { for slot in side.iter_mut() { rng.fill(slot); } }
        let species: Box<[[[u64; NUM_SPECIES]; 6]; 2]> = {
            let ptr = Box::into_raw(species) as *mut [[[u64; NUM_SPECIES]; 6]; 2];
            unsafe { Box::from_raw(ptr) }
        };

        let mut hp_bucket = [[[0u64; HP_BUCKETS]; 6]; 2];
        for side in hp_bucket.iter_mut() { for slot in side.iter_mut() { rng.fill(slot); } }
        let mut status = [[[0u64; NUM_STATUSES]; 6]; 2];
        for side in status.iter_mut() { for slot in side.iter_mut() { rng.fill(slot); } }

        let mut item = vec![[[0u64; NUM_ITEMS]; 6]; 2].into_boxed_slice();
        for side in item.iter_mut() { for slot in side.iter_mut() { rng.fill(slot); } }
        let item: Box<[[[u64; NUM_ITEMS]; 6]; 2]> = {
            let ptr = Box::into_raw(item) as *mut [[[u64; NUM_ITEMS]; 6]; 2];
            unsafe { Box::from_raw(ptr) }
        };

        let mut active_index = [[0u64; 6]; 2];
        for side in active_index.iter_mut() { rng.fill(side); }
        let mut boosts = [[[0u64; BOOST_STAGES]; NUM_STATS]; 2];
        for side in boosts.iter_mut() { for stat in side.iter_mut() { rng.fill(stat); } }
        let mut volatile_bit = [[0u64; 32]; 2];
        for side in volatile_bit.iter_mut() { rng.fill(side); }
        let mut weather = [0u64; NUM_WEATHERS]; rng.fill(&mut weather);
        let mut terrain = [0u64; NUM_TERRAINS]; rng.fill(&mut terrain);
        let trick_room = rng.next();
        let gravity = rng.next();
        let mut phase = [0u64; 5]; rng.fill(&mut phase);

        Self { species, hp_bucket, status, item, active_index, boosts,
               volatile_bit, weather, terrain, trick_room, gravity, phase }
    }
}

#[inline(always)]
pub fn hp_bucket(current_hp: u16, max_hp: u16) -> usize {
    if max_hp == 0 { return 0; }
    ((current_hp as u32 * 8) / (max_hp as u32 + 1)).min(7) as usize
}

pub fn compute_full_hash(state: &BattleState, keys: &ZobristKeys) -> u64 {
    let mut h: u64 = 0;
    for side in 0..2 {
        let s = &state.sides[side];
        h ^= keys.active_index[side][s.active_index as usize];
        for slot in 0..6 {
            let mon = &s.team[slot];
            if mon.species_id != 0 {
                h ^= keys.species[side][slot][mon.species_id as usize];
                h ^= keys.hp_bucket[side][slot][hp_bucket(mon.current_hp, mon.max_hp)];
                h ^= keys.status[side][slot][mon.status as usize];
                if mon.item_id != 0 {
                    h ^= keys.item[side][slot][mon.item_id as usize];
                }
            }
        }
        let active = &s.active;
        for bit in 0..32u32 {
            if active.volatile_flags & (1 << bit) != 0 {
                h ^= keys.volatile_bit[side][bit as usize];
            }
        }
        for stat in 0..7 {
            let stage = active.boosts[stat];
            if stage != 0 { h ^= keys.boosts[side][stat][(stage + 6) as usize]; }
        }
    }
    h ^= keys.weather[state.field.weather as usize];
    h ^= keys.terrain[state.field.terrain as usize];
    if state.field.trick_room_turns > 0 { h ^= keys.trick_room; }
    if state.field.gravity_turns > 0 { h ^= keys.gravity; }
    h ^= keys.phase[state.phase as usize];
    h
}

#[inline]
pub fn validate_hash(state: &BattleState, keys: &ZobristKeys) -> bool {
    state.zobrist == compute_full_hash(state, keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_hp_buckets() {
        assert_eq!(hp_bucket(0, 100), 0);
        assert_eq!(hp_bucket(50, 100), 3);
        assert_eq!(hp_bucket(100, 100), 7);
    }
    #[test]
    fn test_xor_reversibility() {
        let keys = ZobristKeys::new(42);
        let key = keys.weather[WEATHER_RAIN as usize];
        let mut h: u64 = 0x12345678;
        h ^= key; h ^= key;
        assert_eq!(h, 0x12345678);
    }
}
