use pkmn_engine::state::data_bridge::{self, ItemFlag};
use pkmn_engine::state::*;
use std::path::PathBuf;

const QUICK_CLAW: u8 = 1 << 0;
const CUSTAP: u8 = 1 << 1;
const LAGGING_TAIL: u8 = 1 << 2;
const QUICK_DRAW: u8 = 1 << 3;
const LOCKED: u8 = 1 << 4;

const CATEGORIES: [(&str, u8); 5] = [
    ("quick_claw", QUICK_CLAW),
    ("custap_berry", CUSTAP),
    ("lagging_tail", LAGGING_TAIL),
    ("quick_draw", QUICK_DRAW),
    ("lock_charge_encore", LOCKED),
];

fn categories(state: &BattleState, side: usize) -> u8 {
    let mut bits = 0u8;
    let mon = state.active_mon(side);
    let item = data_bridge::item(mon.item_id);
    if item.has(ItemFlag::QUICK_CLAW) {
        bits |= QUICK_CLAW;
    }
    if item.has(ItemFlag::CUSTAP) {
        bits |= CUSTAP;
    }
    if mon.item_id == data_bridge::ITEM_LAGGING_TAIL {
        bits |= LAGGING_TAIL;
    }
    if effective_ability(state, side) == data_bridge::ABILITY_QUICK_DRAW {
        bits |= QUICK_DRAW;
    }
    let active = &state.sides[side].active;
    if active.has_volatile(VOL_CHARGING | VOL_RECHARGING | VOL_MOVE_LOCKED)
        || (active.encore_turns > 0 && active.encore_move != 0)
    {
        bits |= LOCKED;
    }
    bits
}

#[derive(Default)]
struct Census {
    records: u64,
    decider: [u64; CATEGORIES.len()],
    either: [u64; CATEGORIES.len()],
    decider_union: u64,
    either_union: u64,
}

impl Census {
    fn tally(&mut self, state: &BattleState, decider: usize) {
        let own = categories(state, decider);
        let either = own | categories(state, 1 - decider);
        for (i, (_, bit)) in CATEGORIES.iter().enumerate() {
            if own & bit != 0 {
                self.decider[i] += 1;
            }
            if either & bit != 0 {
                self.either[i] += 1;
            }
        }
        if own != 0 {
            self.decider_union += 1;
        }
        if either != 0 {
            self.either_union += 1;
        }
        self.records += 1;
    }

    fn frac(&self, count: u64) -> f64 {
        count as f64 / self.records.max(1) as f64
    }
}

fn require_train_value() {
    #[cfg(not(feature = "train_value"))]
    {
        eprintln!("speed_override_census requires a train_value build");
        std::process::exit(2);
    }
}

fn shard_files(dirs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                eprintln!("{dir}: {e}");
                std::process::exit(2);
            }
        };
        let mut here: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with(".records.bin"))
            .collect();
        here.sort();
        out.extend(here);
    }
    out
}

fn print_table(c: &Census, dirs: usize, files: usize) {
    println!("shard dirs {dirs}  shard files {files}  records {}", c.records);
    println!();
    println!(
        "{:<20}{:>12}{:>11}{:>12}{:>11}",
        "category", "decider", "of records", "either", "of records"
    );
    for (i, (name, _)) in CATEGORIES.iter().enumerate() {
        println!(
            "{:<20}{:>12}{:>10.4}%{:>12}{:>10.4}%",
            name,
            c.decider[i],
            c.frac(c.decider[i]) * 100.0,
            c.either[i],
            c.frac(c.either[i]) * 100.0
        );
    }
    println!(
        "{:<20}{:>12}{:>10.4}%{:>12}{:>10.4}%",
        "union",
        c.decider_union,
        c.frac(c.decider_union) * 100.0,
        c.either_union,
        c.frac(c.either_union) * 100.0
    );
}

fn print_json(c: &Census, dirs: usize, files: usize) {
    let reading = |counts: &[u64; CATEGORIES.len()], union: u64| {
        let mut m = serde_json::Map::new();
        for (i, (name, _)) in CATEGORIES.iter().enumerate() {
            m.insert(
                name.to_string(),
                serde_json::json!({ "count": counts[i], "fraction": c.frac(counts[i]) }),
            );
        }
        m.insert(
            "union".to_string(),
            serde_json::json!({ "count": union, "fraction": c.frac(union) }),
        );
        serde_json::Value::Object(m)
    };
    let out = serde_json::json!({
        "shard_dirs": dirs,
        "shard_files": files,
        "records": c.records,
        "decider": reading(&c.decider, c.decider_union),
        "either": reading(&c.either, c.either_union),
    });
    println!("{out}");
}

fn main() {
    require_train_value();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dirs: Vec<String> = Vec::new();
    let mut json = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            _ if !a.starts_with('-') => dirs.push(a),
            _ => {
                eprintln!("unexpected argument: {a}");
                std::process::exit(2);
            }
        }
    }
    if dirs.is_empty() {
        eprintln!("usage: speed_override_census <raw shard dir>... [--json]");
        std::process::exit(2);
    }

    let files = shard_files(&dirs);
    let mut census = Census::default();
    for path in &files {
        let recs = match poke_mcts::policy_label::read_records_guarded(&path.to_string_lossy()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                std::process::exit(1);
            }
        };
        for rec in recs {
            census.tally(&rec.state, rec.side_of_decider as usize);
        }
    }

    if json {
        print_json(&census, dirs.len(), files.len());
    } else {
        print_table(&census, dirs.len(), files.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poke_mcts::testutil::{duel, mon};

    const ITEM_QUICK_CLAW: u16 = 373;
    const ITEM_CUSTAP_BERRY: u16 = 86;
    const ABILITY_QUICK_DRAW: u16 = 259;
    const MOVE_EARTHQUAKE: u16 = 89;

    fn holder(item_id: u16) -> MonBuildInput {
        let mut m = mon(445, 24, [MOVE_EARTHQUAKE, 0, 0, 0]);
        m.item_id = item_id;
        m
    }

    fn foe() -> MonBuildInput {
        mon(25, 9, [85, 0, 0, 0])
    }

    #[test]
    fn quick_claw_holder_flags_quick_claw_only() {
        let (state, _) = duel(holder(ITEM_QUICK_CLAW), foe());
        assert_eq!(categories(&state, 0), QUICK_CLAW);
        assert_eq!(categories(&state, 1), 0);
    }

    #[test]
    fn lagging_tail_holder_flags_lagging_tail_only() {
        let (state, _) = duel(holder(data_bridge::ITEM_LAGGING_TAIL), foe());
        assert_eq!(categories(&state, 0), LAGGING_TAIL);
        assert_eq!(categories(&state, 1), 0);
    }

    #[test]
    fn quick_draw_effective_ability_flags_quick_draw() {
        let (state, _) = duel(mon(445, ABILITY_QUICK_DRAW, [MOVE_EARTHQUAKE, 0, 0, 0]), foe());
        assert_eq!(categories(&state, 0), QUICK_DRAW);
        assert_eq!(categories(&state, 1), 0);
    }

    #[test]
    fn encore_active_flags_lock_category() {
        let (mut state, _) = duel(holder(0), foe());
        state.sides[0].active.encore_turns = 2;
        state.sides[0].active.encore_move = MOVE_EARTHQUAKE;
        assert_eq!(categories(&state, 0), LOCKED);
        assert_eq!(categories(&state, 1), 0);
    }

    #[test]
    fn encore_turns_without_a_move_is_not_locked() {
        let (mut state, _) = duel(holder(0), foe());
        state.sides[0].active.encore_turns = 2;
        assert_eq!(categories(&state, 0), 0);
    }

    #[test]
    fn charging_recharging_and_move_lock_flag_lock_category() {
        for vol in [VOL_CHARGING, VOL_RECHARGING, VOL_MOVE_LOCKED] {
            let (mut state, _) = duel(holder(0), foe());
            state.sides[0].active.volatile_flags |= vol;
            assert_eq!(categories(&state, 0), LOCKED, "volatile {vol:#x}");
        }
    }

    #[test]
    fn clean_position_flags_nothing() {
        let (state, _) = duel(holder(0), foe());
        assert_eq!(categories(&state, 0), 0);
        assert_eq!(categories(&state, 1), 0);
    }

    #[test]
    fn two_categories_on_one_position_count_once_in_union() {
        let (mut state, _) = duel(holder(ITEM_CUSTAP_BERRY), foe());
        state.sides[0].active.volatile_flags |= VOL_CHARGING;
        assert_eq!(categories(&state, 0), CUSTAP | LOCKED);

        let mut c = Census::default();
        c.tally(&state, 0);
        assert_eq!(c.records, 1);
        assert_eq!(c.decider[1], 1);
        assert_eq!(c.decider[4], 1);
        assert_eq!(c.decider_union, 1);
        assert_eq!(c.either_union, 1);
    }

    #[test]
    fn either_reading_sees_the_opponents_effect() {
        let (state, _) = duel(foe(), holder(ITEM_QUICK_CLAW));
        let mut c = Census::default();
        c.tally(&state, 0);
        assert_eq!(c.decider[0], 0);
        assert_eq!(c.either[0], 1);
        assert_eq!(c.decider_union, 0);
        assert_eq!(c.either_union, 1);
    }
}
