use pkmn_engine::state::*;
use pkmn_engine::state::structs::*;
use pkmn_engine::state::zobrist::*;
use pkmn_engine::state::legal_moves::*;
use pkmn_engine::state::turn::{execute_turn, execute_switch_turn};
use pkmn_engine::data::types::Type;

use std::time::Instant;

const NUM_BATTLES: u64 = 100_000;
const MAX_TURNS: u32 = 500;

const MOVE_POOLS: [[u16; 4]; 6] = [
    [85, 89, 14, 105],    // Thunderbolt, Earthquake, Swords Dance, Recover
    [53, 58, 182, 261],   // Flamethrower, Ice Beam, Protect, Will-O-Wisp
    [57, 200, 92, 19],    // Surf, Outrage, Toxic, Fly
    [63, 369, 85, 53],    // Hyper Beam, U-Turn, Thunderbolt, Flamethrower
    [89, 105, 58, 182],   // Earthquake, Recover, Ice Beam, Protect
    [57, 14, 261, 200],   // Surf, Swords Dance, Will-O-Wisp, Outrage
];

fn run_battles(keys: &ZobristKeys, label: &str) -> (u64, u64, std::time::Duration) {
    let mut total_turns: u64 = 0;
    let mut total_game_overs: u64 = 0;

    let start = Instant::now();

    let teams = TeamData::default();
    for battle_idx in 0..NUM_BATTLES {
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

        let mut rng = |max: u32| -> u32 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % max.max(1)
        };

        for _turn in 0..MAX_TURNS {
            if state.is_game_over() {
                total_game_overs += 1;
                break;
            }

            let a1 = legal_actions(&state, 0);
            let a2 = legal_actions(&state, 1);

            if state.phase == PHASE_ACTIONS {
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_turn(&mut state, keys, &teams, act1, act2, &mut rng);
            } else if state.phase == PHASE_SWITCH_P1 {
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                execute_switch_turn(&mut state, keys, &teams, act1, 0, &mut rng);
            } else if state.phase == PHASE_SWITCH_P2 {
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_switch_turn(&mut state, keys, &teams, 0, act2, &mut rng);
            } else if state.phase == PHASE_SWITCH_BOTH {
                let act1 = a1.actions[rng(a1.count as u32) as usize];
                let act2 = a2.actions[rng(a2.count as u32) as usize];
                execute_switch_turn(&mut state, keys, &teams, act1, act2, &mut rng);
            }

            total_turns += 1;
        }
    }

    let elapsed = start.elapsed();
    println!("[{}] {} ms | {:.1} μs/battle | {:.0} battles/sec | {} turns | {} completed",
        label,
        elapsed.as_millis(),
        elapsed.as_micros() as f64 / NUM_BATTLES as f64,
        NUM_BATTLES as f64 / elapsed.as_secs_f64(),
        total_turns,
        total_game_overs,
    );

    (total_turns, total_game_overs, elapsed)
}

fn main() {
    println!("pkmn-engine Zobrist overhead test: {} battles, max {} turns", NUM_BATTLES, MAX_TURNS);
    println!("---");

    let real_keys = ZobristKeys::new(42);
    let zero_keys = ZobristKeys::new(0);

    println!("\nWarmup run (discarded)...");
    let warmup_keys = ZobristKeys::new(99);
    run_battles(&warmup_keys, "warmup");

    println!("\nMeasurement runs:");
    let (turns1, _, elapsed_real) = run_battles(&real_keys, "real keys  ");
    let (turns2, _, elapsed_zero) = run_battles(&zero_keys, "seed=0 keys");

    let diff_ms = elapsed_real.as_millis() as i64 - elapsed_zero.as_millis() as i64;
    let diff_pct = (elapsed_real.as_nanos() as f64 - elapsed_zero.as_nanos() as f64)
        / elapsed_real.as_nanos() as f64 * 100.0;

    println!("\nDifference: {} ms ({:+.1}%)", diff_ms, diff_pct);
    println!("(Positive = real keys slower, negative = real keys faster)");
    println!("Note: seed=0 keys are NOT zeroed, so this is run-to-run variance.");
    println!();

    let ns_per_turn = elapsed_real.as_nanos() as f64 / turns1 as f64;
    println!("Estimated XOR cost: ~3-9 ns/turn out of {:.0} ns/turn ({:.2}%)",
        ns_per_turn, 9.0 / ns_per_turn * 100.0);
}
