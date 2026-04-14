//! Per-turn RNG carrier for end-of-turn ability consumers (Moody, Shed Skin,
//! Effect Spore, Static, Flame Body, Poison Point). Concrete, non-generic: it
//! wraps a `&mut dyn FnMut(u32) -> u32`, so threading it through the EOT path
//! costs one monomorphization instead of one per closure shape. The hot-path
//! damage functions (calc_damage / is_crit / resolve_hits) keep their
//! `&mut impl FnMut(u32) -> u32` signatures and are deliberately NOT migrated.

pub struct BattleRng<'a> {
    closure: &'a mut dyn FnMut(u32) -> u32,
}

impl<'a> BattleRng<'a> {
    #[inline(always)]
    pub fn from_closure(closure: &'a mut dyn FnMut(u32) -> u32) -> BattleRng<'a> {
        BattleRng { closure }
    }

    /// Draw a value in `[0, max)` — mirrors the `FnMut(u32) -> u32` contract used
    /// across the engine's hot-path RNG closures.
    #[inline(always)]
    pub fn next(&mut self, max: u32) -> u32 {
        (self.closure)(max)
    }
}
