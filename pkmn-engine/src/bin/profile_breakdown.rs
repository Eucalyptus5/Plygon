use pkmn_engine::state::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::zobrist::*;
use pkmn_engine::state::turn::{execute_turn, execute_switch_turn};
use pkmn_engine::data::types::Type;

use std::time::Instant;

const NUM_BATTLES: u64 = 100_000;
const MAX_TURNS: u32 = 500;

const MOVE_POOLS: [[u16; 4]; 6] = [
    [85, 89, 14, 105],
    [53, 58, 182, 261],
    [57, 200, 92, 19],
    [63, 369, 85, 53],
    [89, 105, 58, 182],
    [57, 14, 261, 200],
];

struct ProfileCounters {
    setup_ns: u64,
    legal_actions_ns: u64,
    execute_turn_ns: u64,
    execute_switch_ns: u64,
    game_over_check_ns: u64,
    rng_ns: u64,
    total_turns: u64,
    action_turns: u64,
    switch_turns: u64,
    game_overs: u64,
    legal_action_calls: u64,
}

fn main() {
    println!("pkmn-engine PROFILE BREAKDOWN: {} battles, max {} turns", NUM_BATTLES, MAX_TURNS);
    println!("========================================================================");

    let keys = ZobristKeys::new(42);

    // Warmup
    println!("\nWarming up...");
    run_profiled(&keys, 10_000);

    println!("\nProfiling...");
    let c = run_profiled(&keys, NUM_BATTLES);

    let total_ns = c.setup_ns + c.legal_actions_ns + c.execute_turn_ns
        + c.execute_switch_ns + c.game_over_check_ns + c.rng_ns;

    println!("\n========================================================================");
    println!("BREAKDOWN (measured wall time per component)");
    println!("========================================================================");
    println!("{:>30} {:>10} {:>8} {:>12}", "Component", "Time (ms)", "% Total", "Per-call (ns)");
    println!("{:-<70}", "");

    let print_row = |name: &str, ns: u64, calls: u64| {
        let ms = ns as f64 / 1_000_000.0;
        let pct = 100.0 * ns as f64 / total_ns as f64;
        let per_call = if calls > 0 { ns as f64 / calls as f64 } else { 0.0 };
        println!("{:>30} {:>10.1} {:>7.1}% {:>12.1}", name, ms, pct, per_call);
    };

    print_row("Battle setup + zobrist", c.setup_ns, NUM_BATTLES);
    print_row("is_game_over() checks", c.game_over_check_ns, c.total_turns + c.game_overs);
    print_row("legal_actions()", c.legal_actions_ns, c.legal_action_calls);
    print_row("RNG + action selection", c.rng_ns, c.total_turns);
    print_row("execute_turn()", c.execute_turn_ns, c.action_turns);
    print_row("execute_switch_turn()", c.execute_switch_ns, c.switch_turns);
    println!("{:-<70}", "");
    print_row("TOTAL measured", total_ns, NUM_BATTLES);

    println!("\n========================================================================");
    println!("SUMMARY");
    println!("========================================================================");
    println!("  Total turns: {}", c.total_turns);
    println!("  Action turns: {} ({:.1}%)", c.action_turns, 100.0 * c.action_turns as f64 / c.total_turns as f64);
    println!("  Switch turns: {} ({:.1}%)", c.switch_turns, 100.0 * c.switch_turns as f64 / c.total_turns as f64);
    println!("  Game overs: {}", c.game_overs);
    println!("  Avg turns/battle: {:.1}", c.total_turns as f64 / NUM_BATTLES as f64);
    println!("  Avg ns/turn: {:.1}", total_ns as f64 / c.total_turns as f64);
    println!("  Avg µs/battle: {:.1}", total_ns as f64 / NUM_BATTLES as f64 / 1000.0);
    println!("  Battles/sec: {:.0}", NUM_BATTLES as f64 / (total_ns as f64 / 1_000_000_000.0));

    // Estimate breakdown per turn
    println!("\n========================================================================");
    println!("PER-TURN COST BREAKDOWN");
    println!("========================================================================");
    let ns_per_turn = total_ns as f64 / c.total_turns as f64;
    let components = [
        ("legal_actions (2x)", c.legal_actions_ns),
        ("execute_turn/switch", c.execute_turn_ns + c.execute_switch_ns),
        ("is_game_over", c.game_over_check_ns),
        ("RNG + selection", c.rng_ns),
        ("setup (amortized)", c.setup_ns),
    ];
    for (name, ns) in &components {
        let per_turn = *ns as f64 / c.total_turns as f64;
        println!("  {:30} {:>6.1} ns  ({:>5.1}%)", name, per_turn, 100.0 * per_turn / ns_per_turn);
    }
}

fn run_profiled(keys: &ZobristKeys, num_battles: u64) -> ProfileCounters {
    let mut c = ProfileCounters {
        setup_ns: 0, legal_actions_ns: 0, execute_turn_ns: 0,
        execute_switch_ns: 0, game_over_check_ns: 0, rng_ns: 0,
        total_turns: 0, action_turns: 0, switch_turns: 0,
        game_overs: 0, legal_action_calls: 0,
    };

    let teams = TeamData::default();
    for battle_idx in 0..num_battles {
        // --- Setup ---
        let t0 = Instant::now();
        let mut state = BattleState::default();
        let mut seed = battle_idx.wrapping_mul(6364136223846793005).wrapping_add(1);

        for side in 0..2 {
            for i in 0..6 {
                let pool_idx = ((side * 3 + i) + battle_idx as usize) % 6;
                state.sides[side].team[i] = MonSlot {
                    species_id: (i as u16 + 1) + (side as u16 * 10),
                    current_hp: 300,
                    max_hp: 300,
                    stats: [120, 100, 100, 100, 80 + (i as u16 * 5)],
                    moves: MOVE_POOLS[pool_idx],
                    pp: [24, 24, 24, 24],
                    tera_type: if i == 0 { Type::Fire as u8 } else { 0 },
                    ..Default::default()
                };
                if i == 2 && side == 0 {
                    state.sides[side].team[i].item_id = 581;
                }
            }
        }
        state.phase = PHASE_ACTIONS;
        state.zobrist = compute_full_hash(&state, keys);
        c.setup_ns += t0.elapsed().as_nanos() as u64;

        let mut rng = |max: u32| -> u32 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % max.max(1)
        };

        for _turn in 0..MAX_TURNS {
            // --- Game over check ---
            let t1 = Instant::now();
            let over = state.is_game_over();
            c.game_over_check_ns += t1.elapsed().as_nanos() as u64;

            if over {
                c.game_overs += 1;
                break;
            }

            // --- Legal actions ---
            let t2 = Instant::now();
            let a1 = legal_actions(&state, 0);
            let a2 = legal_actions(&state, 1);
            c.legal_actions_ns += t2.elapsed().as_nanos() as u64;
            c.legal_action_calls += 2;

            // --- RNG / action selection ---
            let t3 = Instant::now();
            let phase = state.phase;
            c.rng_ns += t3.elapsed().as_nanos() as u64;

            if phase == PHASE_ACTIONS {
                let t3b = Instant::now();
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                c.rng_ns += t3b.elapsed().as_nanos() as u64;

                let t4 = Instant::now();
                execute_turn(&mut state, keys, &teams, act1, act2, &mut rng);
                c.execute_turn_ns += t4.elapsed().as_nanos() as u64;
                c.action_turns += 1;
            } else if phase == PHASE_SWITCH_P1 {
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let t4 = Instant::now();
                execute_switch_turn(&mut state, keys, &teams, act1, 0, &mut rng);
                c.execute_switch_ns += t4.elapsed().as_nanos() as u64;
                c.switch_turns += 1;
            } else if phase == PHASE_SWITCH_P2 {
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                let t4 = Instant::now();
                execute_switch_turn(&mut state, keys, &teams, 0, act2, &mut rng);
                c.execute_switch_ns += t4.elapsed().as_nanos() as u64;
                c.switch_turns += 1;
            } else if phase == PHASE_SWITCH_BOTH {
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                let t4 = Instant::now();
                execute_switch_turn(&mut state, keys, &teams, act1, act2, &mut rng);
                c.execute_switch_ns += t4.elapsed().as_nanos() as u64;
                c.switch_turns += 1;
            }

            c.total_turns += 1;
        }
    }

    c
}
